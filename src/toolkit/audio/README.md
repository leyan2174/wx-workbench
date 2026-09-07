# 原生单文件音频转换

## 当前接入状态（2026-09-07）

已接入主仓 `toolkit::audio` 和 [CLI 分派](../../cli/toolkit.rs)，不再是待注册的独立核心。
SILK 解码使用 Rust 与静态 SILK C SDK，MP3 编码调用原生 ffmpeg；此转换路径不启动 Python 或 Node。
批量数据库导出见 [BATCH.md](BATCH.md)，ASR 是独立流程，不能把 MP3 转换当作语音识别。

当前命令（下列示例未在本轮文档校订中执行）：

```powershell
wx toolkit voice-to-mp3 input.silk output.mp3
wx toolkit voice-to-mp3 input.silk
```

输入为必需位置参数，输出为可选位置参数；省略输出时，将输入路径扩展名改为 `.mp3`。
CLI 从 PATH 查找 ffmpeg，没有单文件 `--ffmpeg` 参数；指定编码器路径需使用下方库接口。
单文件成功时允许原子替换已有输出，失败时保留旧文件；这与批量及 MCP WAV 的不覆盖发布不同。

主仓 [Cargo.toml](../../../Cargo.toml) 已声明这些依赖，无需再添加：

```toml
silk-codec = { version = "=0.3.1", default-features = false }
tempfile = "3"
same-file = "1"
```

复用已有 anyhow、serde（derive）；模块注册见 [toolkit/mod.rs](../mod.rs)，实现见 [mod.rs](mod.rs)。

| 状态 | 证据与边界 |
| --- | --- |
| 当前主线自动化 | 精简后 Rust 全量 `1325 passed / 0 failed / 11 ignored`，个人 Web `61/0`；另有 8 次显式可选测试执行通过，默认 ignored 数不改写；MSVC check 通过且仍有 9 条警告 |
| 本文校订 | 只核对源码与主线提供的记录，没有运行 Cargo、ffmpeg、真实录音或账号数据；上述总数不是本模块独立用例数 |
| 未验证边界 | 合成 PCM/MP3 差分不证明任意用户音频兼容性；真实账号、模型/GPU、真实云服务和已安装发布包不在本轮验证内；企业微信排除目标，既有实现和入口保留 |

最新总账及日志出处见 [Rust 迁移记录](../../../docs/rust-migration.md) 和 [系统架构](../../../docs/architecture.md)。

主线本轮文档同步验证另已完成：`C:/CodexLocal/wx-cli-doc-sync-tests.log` 终态退出 0，20 套件合计 `1325/0/11`；check 退出 0、9 警告，18 项 EXE help 检查通过。该证据由主线执行，help 通过不代表真实业务或部署环境验收。

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
- 本文件描述单文件核心；数据库读取、联系人目录、已有项跳过及批量统计已由 [batch.rs](batch.rs) 实现。ASR 不在此核心内，见 [ASR 模块](../asr/mod.rs)。

## Codec 调查

选用 [silk-codec 0.3.1](https://github.com/Redmomn/silk-codec)，其默认功能不依赖 FFmpeg 库；
构建期由 cc 编译内置 SILK SDK FIX v1.0.9，bindgen 生成绑定，需要 MSVC C/C++ 工具链与 libclang。
当前机器验证时设置 LIBCLANG_PATH=C:/CodexLocal/build-tools/libclang/clang/native。
crate 声明 MIT；其内置 SDK 保留 Skype 的许可头，分发时应保留依赖许可证。

对比的 [silk-rs 0.2.0](https://github.com/lz1998/silk-rs) 高层 API 没有单包多帧循环，
且未正确消费 ffff 尾标记，因此未采用；也没有自行实现音频算法。

## 历史独立测试证据（2026-09-07）

以下保留主仓接线前的独立验证记录和当时命令，不是本轮文档任务重新执行的结果；当前集成状态以上节和迁移总账为准。

独立验证项目：C:/CodexLocal/audio-validation-native/Cargo.toml。
其 lib.path 直接指向当前 mod.rs，不复制实现、不运行全仓 Cargo、不使用主仓 target。

```powershell
cargo check --manifest-path C:\CodexLocal\audio-validation-native\Cargo.toml --target x86_64-pc-windows-msvc --config 'env.LIBCLANG_PATH="C:/CodexLocal/build-tools/libclang/clang/native"'
cargo test --manifest-path C:\CodexLocal\audio-validation-native\Cargo.toml --target x86_64-pc-windows-msvc --config 'env.LIBCLANG_PATH="C:/CodexLocal/build-tools/libclang/clang/native"' --config 'env.WX_AUDIO_FIXTURES="C:/CodexLocal/src/wx-cli/tests/fixtures/audio"' -- --include-ignored --nocapture
```

最终测试完整输出的业务部分：

```text
running 7 tests
test tests::payload_ffff_is_not_a_terminator ... ok
test tests::normalizes_prefix_and_tail_without_mutating_source ... ok
test tests::rejects_malformed_containers ... ok
test tests::rejects_source_alias_and_preserves_outputs_on_failure ... ok
test tests::synthetic_sdk_pcm_matches_pilk ... ok
test tests::missing_or_failed_encoder_preserves_target_and_cleans_temporary_files ... ok
tone: PCM byte parity, MP3 byte parity, 4461 bytes, 24000 Hz mono
silence: PCM byte parity, MP3 byte parity, 4461 bytes, 24000 Hz mono
sweep: PCM byte parity, MP3 byte parity, 4461 bytes, 24000 Hz mono
multi40: PCM byte parity, MP3 byte parity, 4461 bytes, 24000 Hz mono
multi100: PCM byte parity, MP3 byte parity, 4461 bytes, 24000 Hz mono
test tests::mp3_matches_legacy_pcm_pipeline_and_has_expected_format ... ok
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.38s
```

5 组 fixture 完全合成，不含用户录音，覆盖静音、440Hz、扫频与 40/100ms 多帧包。
pilk 0.2.4 只用于生成测试 SILK 和参考 PCM；日常 Rust 测试不需要 Python。
每组参考 PCM 为 48000 字节；Rust 输出逐字节相同。
MP3 差分在同一台机器使用同一个 ffmpeg 8.1.2，参考路径采用旧脚本参数和 pilk PCM，输出逐字节相同。
ffprobe 另验证 codec_name=mp3、sample_rate=24000、channels=1。
端到端测试默认 ignore，需显式 --include-ignored 且 PATH 有 ffmpeg/ffprobe；普通测试不依赖 ffmpeg。
当时的独立验证不等同于全仓验收；后续主线集成结果已列于本文开头，不能用这里的 7 项旧记录覆盖当前全量计数或未验证边界。
