# 通用密钥流 25 MiB 修复

> 2026-09-07 文档核对：本页保留修复当时的命令、预算测量和测试数字，不是本次清理后的新验收。生产 `video_runtime` 和相册编排现已注册；当前可在仓库根运行 `cargo test --bin wx toolkit::sns::video_runtime::tests -- --nocapture --test-threads=1`。原 harness、向量和日志均保留，复跑应另存输出，不覆盖历史证据。

## 变更边界

- `video_runtime.rs`：新增 `MAX_KEYSTREAM_BYTES = 25 * 1024 * 1024`；通用 keystream 使用该上限，视频 decode 保持 `min(VIDEO_PREFIX_BYTES)`，即前 128 KiB。
- `video_runtime_tests.rs`：边界 oracle、回调/对齐/内存范围、有限 fuel 与调用者上限、视频前缀测试。
- 本目录新增 `generate-large-vectors.cjs`、生成的 `large-vectors.json` 及此说明。
- 经追加授权，仅调整 `album_images_tests::real_runtime_stream_and_limit` 必要行：验证 128 KiB+1 加密图成功，25 MiB+1 响应被拒绝。未恢复 HTML helper 副本，未修改 mod.rs、Cargo、下载编排或其他模块。

## API 与资源预算

公开 API 签名不变；原调用者无需注册新模块。

通用 keystream 接受 `1..=25 MiB`，同时遵守调用者 `input_bytes`。长度向上按 8 字节 checked 对齐；input_bytes 约束原请求，对齐填充最多 7 字节。25 MiB 本身可被 8 整除，对齐后仍不可超过该全局上限。

回调要求长度非零、8 字节对齐、严格等于本次 expected、至多 25 MiB；checked 检查 `start + size` 与实际 guest 内存范围后才复制到宿主缓冲区。原有回调 ABI、重复回调拒绝、密钥清零和隔离 Store 不变。

`RuntimeLimits.fuel` 继续是调用者的每次调用硬上限；默认值从 100M 改为 300M。实际预算为：

```text
min(caller.fuel, 300_000_000,
    100_000_000 + 8 * max(aligned_size - 131_072, 0))
```

视频前缀仍最多获得 100M；较小的调用者上限不会被自动提升；即使传入 u64::MAX，也不能超过 300M。fuel 消耗计量始终开启。

真实已审计 WASM 的合成 key `42`：25 MiB 消耗 **227,319,547 fuel**，300M 默认上限成功；调用者限定 100M 时按预期耗尽失败，后续小请求可在新的 Store 正常成功。

未放开 guest 扩堆、未修改 WASM 二进制或 ABI 哈希。初始 guest 内存 32 MiB 足以完成已测的 25 MiB；现有默认 Store 内存限制仍为 64 MiB。

## Oracle 与验收

原 Node 包装器生成六组公开合成向量：131071、131072、131073、26214399、26214400、26214401 字节；记录完整输出的 SHA-256，避免提交数十 MiB 密钥流。
Rust 对业务上限内的五组检查长度及完整输出摘要；对最后一组在初始化 WASM 前拒绝，这是显式 25 MiB 业务边界，而非 Node 能力限制。

历史命令（当时均只定向执行，不运行全仓测试；向量生成命令会重写生成文件）：

```powershell
node tests/fixtures/sns-video-native/generate-large-vectors.cjs
cargo test --manifest-path tests/fixtures/sns-video-native/Cargo.toml --target x86_64-pc-windows-msvc --lib video_runtime::tests:: -- --nocapture --test-threads=1
cargo test --bin wx toolkit::sns::album_images::tests::real_runtime_stream_and_limit -- --exact --nocapture --test-threads=1
$env:LIBCLANG_PATH = 'C:\CodexLocal\build-tools\libclang\clang\native'
cargo check --target x86_64-pc-windows-msvc
```

结果：runtime 9/9；注册相册精确测试 1/1；MSVC check 通过，只有 dead-code 警告。
完整 stdout/stderr 已通过 Tee-Object 保存于本目录：

- `large-oracle.log`
- `runtime-25m-tests.log`（含测试阶段的 budget/used 计量，不输出 key）
- `album-runtime-integration.log`
- `runtime-25m-msvc-check.log`
- `fuel-probe.log`（最初有限高预算的单次测量；临时探针代码已删除）

本说明取代早期相册交接中“runtime 仍硬拒绝大于 128 KiB 加密图片”的限制记录；不代表整个 album CLI 编排已迁移完成。
