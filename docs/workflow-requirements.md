# 工作流与前置条件

本页区分当前入口、输入条件与副作用。命令注册不代表目标账号一定有对应数据，也不意味着所有外部依赖已安装。

| 工作流 | 当前入口或契约 | 条件与副作用 |
| --- | --- | --- |
| 会话、联系人、历史、搜索、群成员 | 查询 CLI、MCP、Web | 已保存的目标账号密钥；历史选择普通联系人或群聊，不使用聚合会话。 |
| 收藏、统计、公众号、朋友圈通知 | CLI、MCP 查询、HTTP 与 Web 查询面板 | 本地库包含相应数据；统计工具为 `get_chat_stats`，不能用当前页计数代替。 |
| 标签与标签成员 | `tags` / `tag-members`、MCP、HTTP、Web 标签入口 | 唯一匹配标签；标签目录计数不等于已返回全部成员。 |
| 只读语音目录 | `voice-messages`、`get_voice_messages`、HTTP 与 Web 查询面板 | 读取媒体元数据，不读取音频正文，不写出 SILK；未知大小保持未知。 |
| 转账、位置、引用 | 结构化查询与解码 | 完整且唯一的消息定位；不转账、不访问外链。 |
| 图片列表 | 附件/MCP 元数据 | 资源表和消息证据满足条件；缺失或歧义不能伪造大小。 |
| 文件与合并记录附件 | 只读附件契约 | 账号范围内的原始副本与可验证绑定；不自动下载。 |
| 图片发布 | 显式解码与输出参数 | 已存在的可信输出根及所需图片密钥，不覆盖已有文件。 |
| 原始语音导出 | `voices`、聊天导出 | 保留 SILK 与消息引用，通过关联 manifest 交给下游处理；缺失或歧义不伪造关联。 |
| 单聊/批量导出 | `wx export`、`wx chats export`、`wx chats export-all`、`wx chats export-messages` | 选择范围、格式和输出；已有产物按各命令覆盖规则处理。 |
| 增量与计划 CSV | `wx chats export-delta`、`wx chats plan` | 明确的快照/计划和账号绑定；不把增量当作重写既有完整导出。 |
| 表情导出 | `wx emoticons export` | 使用保存密钥；网络取回与本地数据处理分开授权。 |
| SNS 预览与相册 | `wx moments export-snapshot`、`wx sns-album` | 默认离线与显式下载分开；来源绑定与更新规则不能省略。 |
| SNS 视频 | `wx media video decode` | 本地输入和新输出；MP4 头检查不证明完整可播放。 |
| 初始化和密钥 | init、setup | 区分检查、只读扫描、DPAPI 复用和显式重启捕获。 |
| 清理 | `wx cleanup` | 先预览、明确账号和逐文件选择，不清理未知目录。 |
| 本地 Web | `wx web` | 本地认证、Host/Origin/CSRF；错误与限流应可见。 |
| 后台任务 | wx tasks | 与前台操作租约不同；取消须等待 worker 回收。 |

## 失败与授权

查询参数与协议差异见[查询协议](query-protocol.md)、[HTTP API](http-api.md)；入口接线和验收状态见[能力矩阵](capability-matrix.md)。Web 已接查询面板、历史范围和消息详情，不据此宣称浏览器验收通过。

查询超时、后台不可用、无匹配消息和资源缺失应分别处理。媒体发布或目录更新可能已经提交，响应失败不表示没有副作用，不能盲目重复提交。

需要登录、手机确认、购买服务、提供凭据、重启应用或新下载授权时，自动流程跳过该项并说明条件。不要通过切换 provider 绕过授权。

## 详细说明

- [账号密钥](account-key-provider.md)
- [附件](native-attachment-contract.md)
- [MCP](../src/mcp/PROTOCOL.md)
- [SNS 媒体密钥流](../src/adapters/wechat/media/SNS_KEYSTREAM.md)
- [daemon](daemon-entrypoints.md)、[任务](daemon-tasks.md)、[测试](../tests/README.md)

公开命令与参数以本项目实际注册和各子命令帮助为准。
