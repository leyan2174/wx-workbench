# ASR 缓存独立安全审查

> 2026-09-07 文档核对：下方各次验收数字属于当时源码与日志，不是本次清理后的新结果。生产 cache/cached/receipt 已注册；`voice-cache-receipt` 的孤立 target 清理没有删除本 harness。当前 receipt 主仓入口见 [receipt README](../voice-cache-receipt/README.md)。

无过滤运行所需路径由本目录 `build.rs` 自动准备：仅从相邻真实 fixture 复制 `asr-local/fake.rs` 和五组合成 SILK/PCM 到本目录被忽略的 `tests/fixtures/`。不修改公共 `CARGO_MANIFEST_DIR`、生产源码或其他测试；复制后逐字节核验。硬链接红测试不屏蔽、不修改断言。

历史无过滤运行 137 通过、0 失败、3 忽略，见 unfiltered-path-fixed.log 和 REVIEW.md 首节。Poincare 修复后的外部硬链接原断言已独立通过；历史 cli-path-rerun.log 保留修复前失败证据，测试断言没有弱化。

审查起始仅引用真实 `src/toolkit/asr/cache.rs`；后续 harness 也直接引用下述 cached/ASR/audio/CLI 模块并保留其测试，不复制生产逻辑。fixture 本身不负责生产注册。依赖设置与主项目保持一致，尤其 `serde_json` 的 `arbitrary_precision`。全部使用临时合成文件，不加载真实模型、云服务、账号或凭据。

审查范围：失败凭据不落盘、损坏及账号封套拒绝、未知字段与高精度数值保留、读取流大小上限、打开后非协作替换、硬链接原子发布与最终重解析点拒绝。文件符号链接用例首次因 Windows 1314 失败，现以明确 ignore 原因保留，目录 junction 用例实际通过；不能把它当成文件符号链接已经通过。

边界：账号摘要是调用方给定的命名隔离，不是认证或防篡改签名。可信稳定父目录、非协作写者最终检查到 rename 的 non-CAS 竞态、目录断电持久性、转录正文自身隐私均已在 CACHE.md 声明，不作为新 bug。硬链接本身未声明禁止；测试检查原子替换不会原地改写别名目标。64 MiB 是文件字节上限，不是进程内存峰值承诺；动态错误顺序结合真实源的 `take(64 * 1024 * 1024 + 1)` 核验读取限制，不使用内存猜测。

薄适配直接引用真实 `cached.rs`、`asr/mod.rs`、`audio/mod.rs`，并为原模块自带测试补入真实 CLI 引用；不复制生产逻辑。只运行缓存相关筛选项。合成假程序只产生固定文本、失败诊断或尝试打开测试模型写句柄；无真实模型或云端请求。

完整独立 harness 的复跑入口（仓库根目录；本次未执行）：

```powershell
$env:LIBCLANG_PATH = 'C:/CodexLocal/build-tools/libclang/clang/native'
cargo test --offline --manifest-path tests/fixtures/asr-cache-security/Cargo.toml --target-dir C:/CodexLocal/build/asr-cache-security -- --nocapture
```

保留的历史分组运行器为 `powershell -NoProfile -ExecutionPolicy Bypass -File tests/fixtures/asr-cache-security/run.ps1`；它只执行四组过滤测试，不等同于上面的无过滤运行，并会写入固定的源哈希与日志文件。需要本机已安装 Rust 及离线 Cargo 依赖。保留历史证据时使用上面的 Cargo 入口并另存新输出，不覆盖旧日志。文件符号链接补验须在有权限的环境对 `final_symlink_is_rejected_without_touching_target -- --ignored --nocapture` 单独运行。
