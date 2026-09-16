# 聊天目录展示适配边界

## 当前实现

聊天目录导出继续保留现有 daemon 查询和 JSON wire，以避免同时改变媒体关联、增量摘要以及 CSV/HTML/JSON 输出。微信 `local_type` 的数字到展示标签映射、名片/位置/分享 XML 摘要、系统消息摘要和未知类型回退，已统一迁到 `adapters::wechat::messages::directory_display`。

`application::chat_directory` 只消费适配器返回的 `DirectoryDisplay`，负责时间格式、发送方向、媒体准备、渲染、暂存和发布。`application::chat_directory::render` 不再解释微信数字编码或私有 XML。结构化详情开关是适配器投影中的非序列化标记，因此公开 JSON 字段保持不变。

## 兼容行为

- 目录 JSON 仍包含既有 `local_type`、`raw_content`、`native` 等兼容字段。
- 中文类型名称和友好正文保持原值，包括高 32 位带子类型的消息编码。
- 未知类型仍优先使用既有投影的 `content`，否则截取原始正文。
- 媒体定位仍使用当前严格/兼容规则，本轮未改变关联策略。

## 已验证

- Windows MSVC 全目标编译通过。
- 消息投影聚焦测试：10 通过。
- 聊天目录渲染聚焦测试：6 通过。
- 测试只使用合成消息、XML 和临时文件，没有读取真实账号或媒体。

## 尚未完成

目录导出的 daemon 查询仍生成兼容 JSON，应用仍需校验并保存其中的物理兼容字段。彻底移除应用侧 `local_type` 需要先定义类型化目录导出契约，并让 daemon 查询、媒体适配器、增量指纹和协议投影同时切换；本轮没有把局部迁移宣传为这一改造已经完成。
