# MCP 图片发布安全回归

本夹具验证图片宿主守卫、唯一消息与资源关联、路径隔离及禁止覆盖发布。所有输入使用合成数据，不读取账号密钥或私人图片。

重点检查输出目录在等待期间被替换、受保护目录变为输出、密钥文件和账号身份变化，以及缓存文件生命周期。提交前须重新验证原始守卫，不能只检查新路径的表面合法性。

图片发布与响应发送不是同一事务。测试保留“文件已发布但响应失败”的场景；它用于约束调用方重试和结果解释，不应改成承诺回滚。已有目标、链接或目录均不得覆盖。

按[测试说明](../../README.md)准备依赖，从仓库根目录运行：

```powershell
cargo test --offline --manifest-path tests/fixtures/mcp-image-security/Cargo.toml -- --nocapture
```

需要符号链接权限的项目须单列。完整边界见[附件契约](../../../docs/native-attachment-contract.md)和[MCP 协议](../../../src/mcp/PROTOCOL.md)。
