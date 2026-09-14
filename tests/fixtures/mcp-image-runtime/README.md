# MCP 图像集成测试

入口为 `tests/mcp_image_runtime.rs`，仅在 Windows 运行。复用相邻
`mcp-readonly-runtime` 的账号、命名管道、子进程生命周期与加密 SQLite
夹具，不替代生产查询、数据库解密或图像解码实现。

`images.rs` 为两个临时账号创建不同像素的 BMP，分别封装为旧式 XOR
与 V1 AES DAT。消息、资源索引与密钥均为合成数据，不读取真实微信账号。

覆盖范围：

- 无有效账号配置时，初始化与工具列举不读取密钥、不创建解密目录。
- 缺少宿主输出根时拒绝解码，工具参数不能指定路径或密钥文件。
- 经真实 daemon 解密数据库、绑定资源与解码 DAT，逐字节验证输出。
- 两账号使用同一资源 hash 时保持隔离；同输出根下不同内容共存。
- 重复输出拒绝覆盖；重复消息身份明确失败，不产生额外输出。
- 输出后账号配置、密钥、数据库与源媒体的快照保持不变。

复跑命令（仓库根目录）：

```powershell
cargo test --offline --test mcp_image_runtime --test mcp_readonly_runtime --test mcp_runtime -- --nocapture
```

该夹具不覆盖 V2 密钥探测或在线下载。运行结果以测试进程退出码为准；
编译失败不代表测试通过。

运行依赖见[测试说明](../../README.md)。
