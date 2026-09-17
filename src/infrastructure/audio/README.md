# 原生单文件音频转换

## 入口

模块注册于 `infrastructure::audio`，由 `Operation::ExportAudio` 和 `Operation::ConvertAudio` 分派到 [音频执行模块](../../daemon/operations/audio_export.rs)调用。
SILK 解码使用 Rust 与静态 SILK C SDK，MP3 编码调用原生 ffmpeg；此转换路径不启动 Python 或 Node。
批量数据库导出见 [批量语音导出](../../../docs/voice-batch-export.md)，ASR 是独立流程，不能把 MP3 转换当作语音识别。

命令：

```powershell
wx audio convert input.silk output.mp3
wx audio convert input.silk
```

输入为必需位置参数，输出为可选位置参数；省略输出时，将输入路径扩展名改为 `.mp3`。
CLI 从 PATH 查找 ffmpeg，没有单文件 `--ffmpeg` 参数；指定编码器路径需使用下方库接口。
单文件成功时允许原子替换已有输出，失败时保留旧文件；这与批量及 MCP WAV 的不覆盖发布不同。

根 [Cargo.toml](../../../Cargo.toml) 声明以下依赖：

```toml
silk-codec = { version = "=0.3.1", default-features = false }
tempfile = "3"
same-file = "1"
```

复用已有 anyhow、serde（derive）；模块注册见 [infrastructure/mod.rs](../mod.rs)，实现见 [mod.rs](mod.rs)。


接口：

```rust
pub fn normalize_silk(data: &[u8]) -> anyhow::Result<Vec<u8>>;
pub fn decode_silk_to_pcm(data: &[u8]) -> anyhow::Result<Vec<u8>>;
pub fn convert_silk_to_mp3(input: &Path, output: &Path) -> anyhow::Result<Conversion>;
pub fn convert_silk_to_mp3_with_ffmpeg(
    input: &Path, output: &Path, ffmpeg: &Path,
) -> anyhow::Result<Conversion>;
```

Conversion 可序列化，字段为 input: PathBuf、output: PathBuf、size: u64。
output 是绝对路径；扩展名由调用者决定，编码格式显式固定为 MP3。
默认从 PATH 查找 ffmpeg，也可通过带 with_ffmpeg 的接口指定绝对路径。

## 实现与边界

- SILK V3 头必需；可选单个 02 前缀；完整封包可省略 ffff 尾标记。
- 以封包边界识别尾标记，不误判压缩载荷末尾的 ffff。
- 固定解码为 24000 Hz、单声道、s16le；SDK 负责实际编解码，包含单包内多帧循环。
- 主动拒绝空文件、仅头、截断封包、非正包长、超过 1024 字节的包、尾标记后额外数据。
- 为防止无界内存增长，单输入最多 16 MiB、6000 个包。这是比旧脚本严格的输入契约。
- 非流式：一次读取 SILK、解码整段 PCM，再写临时 PCM 文件。
- 使用 tempfile 在输出同目录创建排他临时文件；只有 ffmpeg 成功且结果非空、sync_all 成功后才 persist 原子替换。
- 任一步失败，既有输出保留；临时文件自动清理。拒绝源文件同路径及硬链接别名。
- ffmpeg 直接 Command 调用，不经过 shell；Windows 使用 CREATE_NO_WINDOW，并禁用 stdin。
- 未加入 ffmpeg 超时机制；不把任意可执行文件视为不可信沙箱。
- 本文件描述单文件核心；数据库读取、联系人目录、已有项跳过及批量统计由 [应用层批次导出](../../../docs/voice-batch-export.md) 实现。ASR 不在此核心内。

## Codec 调查

选用 [silk-codec 0.3.1](https://github.com/Redmomn/silk-codec)，其默认功能不依赖 FFmpeg 库；
构建期由 cc 编译内置 SILK SDK FIX v1.0.9，bindgen 生成绑定，需要 MSVC C/C++ 工具链与 libclang。
未自动发现 libclang 时，将 `LIBCLANG_PATH` 设为本机实际 DLL 所在目录。
crate 声明 MIT；其内置 SDK 保留 Skype 的许可头，分发时应保留依赖许可证。

对比的 [silk-rs 0.2.0](https://github.com/lz1998/silk-rs) 高层 API 没有单包多帧循环，
且未正确消费 ffff 尾标记，因此未采用；也没有自行实现音频算法。

## 测试

音频夹具使用合成 SILK 与参考 PCM，覆盖静音、音调、扫频及多帧包，不含用户录音。普通测试不需要 Python 或 FFmpeg；重新生成参考数据才需要对应生成依赖。MP3 端到端测试需要 FFmpeg/ffprobe，确认依赖后定向执行，不把默认忽略算作通过。

命令与人工审核点见[测试说明](../../../tests/README.md)。
