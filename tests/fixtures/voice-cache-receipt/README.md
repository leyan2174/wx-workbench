# Receipt 生产回归

旧内存规则 fixture 已撤除。真实测试位于 `src/toolkit/asr/receipt_tests.rs` 和 `src/toolkit/asr/receipt_cached_tests.rs`，分别由生产 `receipt` 和 `cached::tests::receipt_tests` 注册，主仓及既有 ASR harness 均可编译这些测试。

> 2026-09-07 文档核对：本目录没有独立 `Cargo.toml`；本次仅清理早已孤立的 `target/` 生成物，保留本说明、历史设计和证据。宿主 `src/cli/mcp.rs`、`src/cli/mcp_voice.rs` 已接入 `Pending::try_cached`。这项源码核对不是新的运行时通过声明。

在仓库根目录执行：

```powershell
$env:LIBCLANG_PATH='C:/CodexLocal/build-tools/libclang/clang/native'
cargo test --bin wx toolkit::asr::receipt::tests -- --nocapture
cargo test --bin wx toolkit::asr::cached::tests::receipt_tests -- --nocapture
```

需要复跑原独立 ASR harness 时，仍从仓库根目录使用其 manifest：

```powershell
$env:LIBCLANG_PATH='C:/CodexLocal/build-tools/libclang/clang/native'
cargo test --offline --manifest-path tests/fixtures/asr-cache-security/Cargo.toml --lib receipt
cargo test --offline --manifest-path tests/fixtures/asr-cache-security/Cargo.toml --lib
```

独立 harness 通过自己的 manifest/build.rs 定位和准备测试程序及音频资源；这不是主仓测试必须改用该 manifest 的要求。本目录不再提供第二个 Cargo crate，不应将已清理的 `target/` 当作运行入口。

生产索引在现有缓存的 `_wx_asr_receipts` 顶层扩展内，与成功记录共用既有锁和同一次原子提交，不使用 sidecar。[DESIGN.md](DESIGN.md) 仅保留历史讨论；账号沿调用方 runtime.id 契约。原日志和历史数字不代表本次清理后的新验收；复跑应另存日志，不覆盖旧证据。
