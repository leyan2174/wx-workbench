# 聊天计划进程测试

所有数据库和媒体文件在临时目录创建，仅含合成数据。过程测试运行 Cargo 构建的 wx，而不是 mock 或单独编译的 CLI 替身。未注册 `wx chats plan` 时测试必须失败，不能跳过。

运行：`cargo test --target x86_64-pc-windows-msvc --test chat_plan_runtime -- --nocapture --test-threads=1`。

覆盖完整 12 列、UTF-8 BOM、CRLF、CSV 引号换行、64 位整数精度、时间双端点、跨分片、NULL、资源和语音估算、扫描逻辑长度、硬链接逐目录项计数、同名联系人筛选、线程结果顺序、缺库状态、不覆盖、源目录隔离、失败退出，以及无 Python/无真实配置环境。

## 元数据缺失行为

- 不提供聊天清单时必须明确传入 username；chat_name 回退为 username，chat_type 仅按 @chatroom 后缀推定 group，否则 single，index 按输入编号。
- JSON 清单只接受 `username`、`index`、`chat_name` 和 `chat_type`；旧 `display_name` / `kind` 字段会被明确拒绝。
- JSON 清单中缺少可选字段同样回退；不会查联系人或自动发现账号，也不会伪造昵称。
- 显式指定不存在的 JSON 文件会失败，不会回退。当前 stderr 只有底层文件错误，尚无专门的“元数据文件缺失”上下文；测试只要求非零退出和保留错误输出。
- 回退不会额外产生 metadata_missing 状态。因此无缺库状态不意味着联系人元数据完整。

## 本命令不覆盖的能力

以下限定的是 `wx chats plan` 及本测试的职责，不是整个项目的未完成清单。计划 CSV 的导出消费由 `wx chats export` 负责，见 [计划消费说明](../plan-selection/README.md)。

- 本命令只生成离线计划 CSV，不读取计划 CSV 驱动聊天导出，不实现旧脚本的黑白名单执行、聊天导出、增量合并、转录或 Web 功能。
- 不自动发现消息分片、联系人、账号、缓存、资源或媒体库；调用方必须提供明确清单。禁止从环境用户过滤变量推断名单。
- scan 统计逻辑文件长度而非磁盘分配空间，扫描整个 username 目录，不按消息时间范围裁剪；total_estimated_bytes 仍是估算合计，不改为扫描合计。
- scan 拒绝重解析点并报告状态，深度上限 128；这是比旧脚本更严格的安全边界，不声称链接行为完全相同。
- 部分数据库缺失或损坏可以生成 partial 状态 CSV 并正常退出；输入、输出或参数错误非零退出。
- JSON 清单上限 16 MiB，输出父目录须已经存在。命令参数以 `wx chats plan --help` 为准。

环境与人工审核点见[测试说明](../../README.md)。
