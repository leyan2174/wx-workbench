# 语音真实进程回归

入口：`cargo test --test voice_runtime -- --nocapture`。仅使用现场生成的 SQLCipher 加密 SQLite、固定虚构密钥和临时配置。直接启动真实 wx daemon，通过账号专属 named pipe 发送 JSON IPC，不定义额外的 CLI 或 MCP 工具。

覆盖原始反斜杠键、大小写键、两个 media 分片交错全局分页、默认 limit=20/offset=0、闭区间时间过滤、NULL 与零字节大小、缺片和未知片失败且无部分结果、共享运行根下双账号相同 ID 隔离、分页拒绝、源数据库/配置/密钥逐字节及目录清单不变。

测试持有 daemon 子进程句柄，成功或 panic 均 kill/wait 回收；IPC 设有八秒超时和一 MiB 读取上限。每次请求、响应和完整 daemon stdout/stderr 随 `--nocapture` 输出。未覆盖真实微信版本差异、真实音频解码或音频内容读取；本接口只查询元数据。
