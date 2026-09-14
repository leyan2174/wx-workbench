# 媒体读取与发布边界

`business::media` 定义媒体种类、关联策略、阶段失败和不透明引用，不持有路径、SQL、密钥或进程。引用绑定消息快照和媒体读取实例；快照失效、来源不完整、歧义和证据冲突不等于未找到，也不能触发兼容回退。

`adapters::wechat::media::resource` 持有资源表 SQL、ChatName2Id 校验和 packed_info 摘要解析。严格 ResourceReader 要求完整消息身份、唯一资源行、真实 INTEGER 身份列及有界 BLOB。ImageSource 绑定资源行和摘要，发现阶段不解码；发布前重新验证证据。摘要扫描仅提供关联线索，不证明明文真实性。

旧 resolver 入口仅为兼容导入。其实际查询位于显式命名的 `legacy_lookup_md5`，仍保留旧时间回退与低位类型匹配行为；严格 ImageSource 不调用它。这个兼容入口仍会将部分旧查询错误当作无结果，不可用作严格关联或授权依据。

语音数据库关联实现在 `media::voice`，ASR 的旧模块仅保留导入兼容。消息定位使用真实 messages::Snapshot，读取 server_id 时拒绝非 INTEGER 值；媒体行号不冒充消息 local_id。MCP prepare/finish 继续使用原有可复核证明，不序列化快照引用、不新增长期查询租约。

## 当前发布限制

图片仍使用既有临时文件与 persist_noclobber，保留 HostOutputGuard、源 Pin 和最终 proof callback。尚未统一到 ExportTarget；后续复用共享发布核心必须保留最后核验与不覆盖已有输出的语义。此迁移不声称所有媒体发布、附件查询或目录匹配调用者已经完成统一。

## 验证接线

根与独立 fixture 通过 tests/support 的窄模块组合编译真实资源、消息读取和语音实现；不替换数据库查询或关联实现。独立读取 fixture 的 zstd 版本沿用根项目。既有 native-image 与 resolver 合成测试继续覆盖迁移后的实现。编译和测试由父任务统一执行，接线完成不等于测试已通过。
