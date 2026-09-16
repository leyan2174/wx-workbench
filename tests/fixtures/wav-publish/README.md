# 原生 WAV 无覆盖发布

直接编译 `src/infrastructure/audio` 和 `attachment/local_files.rs`；不复制 Scan/Pin、WAV 解析或 SILK 解码。fixture 使用合成临时目录和已有静音 SILK，不读取真实账号、不调用网络转录或外部转换器。

## 接口

`publish_wav_noclobber(wav, &HostOutputGuard, before_commit) -> Result<PublishedWav>` 为 crate 内接口。描述字段：`path: PathBuf`、`size: u64`、`pcm_bytes: u64`、`sample_rate: u32`。PCM16 单声道时长应由 `pcm_bytes / (2.0 * sample_rate)` 计算。

完整 WAV 通过共享音频 chunk parser 和 32 MiB 上限后，以完整字节的 SHA256 小写十六进制摘要加 `.wav` 为名。文件名不含 username。附加 chunk 不计入 PCM 字节数，不假设 44 字节头。

同目录临时文件写入、`sync_all`、`reopen` 身份/完整字节验证后，调用 `FnOnce(&PublishedWav) -> Result<()>`。宿主在此预构造并限制响应，检查身份、取消和其他上下文；回调前后以及无覆盖提交前复核共享 guard。成功提交后只返回内存描述，无后续可失败 I/O。

## 验证

```powershell
cargo test --manifest-path tests/fixtures/wav-publish/Cargo.toml --test publisher -- --nocapture
```

单个 integration test 执行完整场景组并逐组报告：完整字节/摘要名、提交前响应借用、重复拒绝、附加和填充 chunk、16kHz 元数据、真实 SILK 转换、回调拒绝、已有文件/目录/硬链接、回调抢占目标、暂存内容改变、坏格式/重复 data/超限、共享保护输入及输出目录替换。符号链接创建权限不足时明确输出 `UNVERIFIED`，不将其算作已验证。

## 边界

输出目录必须是宿主信任的现有本地目录。`protect` 负责路径隔离；需要固定的静态源由宿主 `pin_input`。Windows 下持有目录句柄不保证阻止全部重命名，路径复核与按路径提交之间仍有 TOCTOU 窗口，本实现不声称抵抗同权限恶意进程的所有竞争。目录被外部移走时，按原路径清理临时文件也可能留下被移动的暂存文件；不会将其报告为已发布 WAV。

提交后响应传输仍可失败，已发布文件不自动回滚；重试不得覆盖已有文件。本 fixture 不代替完整构建、原 ASR 单元测试和真实 MCP 宿主验证。

依赖及跳过规则见[测试说明](../../README.md)。
