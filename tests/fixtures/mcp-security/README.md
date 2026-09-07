# MCP 安全回归

入口：`cargo test --test mcp_security -- --nocapture`。

仅现场生成配置、无效密钥占位符和本机命名管道 mock。真实入口通过 `CARGO_BIN_EXE_wx` 启动；隔离 HOME、配置与 PATH。不读取微信数据库，不启动真实查询 daemon，不使用网络。

覆盖初始化/非法工具不触发 IPC、异常配置脱敏、未关闭 stdin 的超限退出，以及启动 ping 是否绕过 IPC 响应限额。超大响应最多为 8 MiB 合成 padding。

## 启动 Pong 判据

在 `--max-frame-bytes=1024` 下，mock 第一次返回带 8 MiB padding 的合法 Pong，后续返回正常小 Pong。测试断言完整请求序列为 `ping → ping → contacts`，并检查联系人查询成功且结果为空。若初始超大 Pong 被当作有效健康响应，序列会变成 `ping → contacts`，测试失败；仅连接失败而未完成重试及查询也不能通过。

不再用服务端 `write_all` 完成与否推断客户端接收量或内存分配：Windows 管道缓冲可使写入成功，即使客户端已经按限额拒绝响应。该测试证明初始超大 Pong 未作为有效健康响应且小 Pong 重试成功，不是内存峰值测量，也不证明客户端读取的精确字节数。保留 8 MiB 合法响应，避免用格式错误替代超限威胁。
