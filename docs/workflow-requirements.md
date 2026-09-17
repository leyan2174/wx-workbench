# 工作流与前置条件

本页区分当前入口、输入条件与副作用。命令注册不代表目标账号一定有对应数据，也不意味着所有外部依赖已安装。

| 工作流 | 当前入口或契约 | 条件与副作用 |
| --- | --- | --- |
| 会话、联系人、历史、搜索、群成员 | 查询 CLI、MCP、Web | 已保存的目标账号密钥；历史选择普通联系人或群聊，不使用聚合会话。 |
| 收藏、统计、公众号、朋友圈通知 | 对应 CLI 与界面入口 | 本地库包含相应数据；MCP 没有独立 stats 工具。 |
| 转账、位置、引用 | 结构化查询与解码 | 完整且唯一的消息定位；不转账、不访问外链。 |
| 图片列表 | 附件/MCP 元数据 | 资源表和消息证据满足条件；缺失或歧义不能伪造大小。 |
| 文件与合并记录附件 | 只读附件契约 | 账号范围内的原始副本与可验证绑定；不自动下载。 |
| 图片发布 | 显式解码与输出参数 | 已存在的可信输出根及所需图片密钥，不覆盖已有文件。 |
| 语音准备/解码 | voice 查询、audio 模块 | 媒体 ID 与消息关联有效；发布 WAV 是写入操作。 |
| 本地识别 | whisper.cpp 或配置式 Python | 程序、模型、依赖明确；命名模型下载需另外授权。 |
| 云端识别 | 显式后端与上传许可 | 端点、模型、凭据文件和上传授权缺一不可。 |
| 缓存与回写 | ASR cache/receipt/writeback | 输入证据、账号与后端身份匹配；回写有独立确认和原子发布边界。 |
| 单聊/批量导出 | export、toolkit 导出入口 | 选择范围、格式和输出；已有产物按各命令覆盖规则处理。 |
| 增量与计划 CSV | export-delta-native、chat-plan-native | 明确的快照/计划和账号绑定；不把增量当作重写既有完整导出。 |
| 表情导出 | export-emoticons | 使用保存密钥；网络取回与本地数据处理分开授权。 |
| SNS 预览与相册 | export-sns-native、sns-album | 默认离线与显式下载分开；来源绑定与更新规则不能省略。 |
| SNS 视频 | decode-sns-video | 本地输入和新输出；MP4 头检查不证明完整可播放。 |
| 初始化和密钥 | init、setup | 区分检查、只读扫描、DPAPI 复用和显式重启捕获。 |
| 清理 | toolkit cleanup | 先预览、明确账号和逐文件选择，不清理未知目录。 |
| 本地 Web | toolkit web | 本地认证、Host/Origin/CSRF；错误与限流应可见。 |
| 后台任务 | wx tasks | 与前台操作租约不同；取消须等待 worker 回收。 |

## 失败与授权

查询超时、后台不可用、无匹配消息和资源缺失应分别处理。媒体发布或目录更新可能已经提交，响应失败不表示没有副作用，不能盲目重复提交。

需要登录、手机确认、购买服务、提供凭据、重启应用或新下载授权时，自动流程跳过该项并说明条件。不要通过切换 provider、安装模型或回退云端绕过授权。

## 详细说明

- [账号密钥](account-key-provider.md)
- [附件](native-attachment-contract.md)
- [MCP](../src/mcp/PROTOCOL.md)
- [ASR](../src/infrastructure/transcription/LOCAL.md)、[云端](../src/infrastructure/transcription/OPENAI.md)、[缓存](../src/application/transcription/CACHE.md)
- [音频](../src/infrastructure/audio/README.md)、[SNS 媒体密钥流](../src/adapters/wechat/media/SNS_KEYSTREAM.md)
- [daemon](daemon-entrypoints.md)、[任务](daemon-tasks.md)、[测试](../tests/README.md)

公开命令与参数以本项目实际注册和各子命令帮助为准。
