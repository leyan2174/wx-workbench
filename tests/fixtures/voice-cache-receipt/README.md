# 转录成功记录索引回归

真实测试位于 src/toolkit/asr/receipt_tests.rs 和 receipt_cached_tests.rs，分别由生产 receipt 和 cached 测试模块注册。本目录没有独立 Cargo.toml。

按[测试说明](../../README.md)准备依赖后，从仓库根目录执行：

```powershell
cargo test --bin wx toolkit::asr::receipt::tests -- --nocapture
cargo test --bin wx toolkit::asr::cached::tests::receipt_tests -- --nocapture
```

索引位于缓存的 _wx_asr_receipts 顶层扩展，与成功记录共用锁和同一次原子提交，不使用独立 sidecar。MCP 账号取固定 RuntimeContext.id，daemon 内的宿主执行器负责授权、守卫与请求预算。

回归覆盖精确 username 和媒体 ID 命中、账号和配置隔离、记录摘要冲突、源删除后读取、命中不修改缓存，以及提交前拒绝。已有强身份记录补写索引须验证真实 evidence，不能凭弱旧记录补造来源。命中可以不读取源语音，但 MCP 仍需要有效 daemon 会话。

索引不是签名，不证明当前消息仍存在；程序、模型及身份计算所需依赖不能任意删除。完整限制见[缓存契约](../../../src/toolkit/asr/CACHE.md)。
