# 媒体读取与发布边界

## 阶段 5 媒体内容与缓存布局

`AttachmentKind` 保持原有 opaque ID 的 JSON 字符串形状。未被生产调用的 `from_local_type` 位掩码方法已删除；resolver 实际使用的资源类型数字映射迁至 `media::attachment_kind`，对应 fixture 使用同一个协议枚举，不复制编码规则。

- `business::attachment_content` 拥有解码后的附件内容、容器种类和具名媒体结果；不依赖 XML、JSON、微信来源路径、消息分库或原始 datatype。媒体引用和严格关联仍复用 `business::media` 与既有 ImageSource，不创建替代关联实现。
- `adapters::wechat::media::attachment_content` 复用原有文件/合并记录 XML 算法，拥有消息来源校验、私有字段映射、记录缓存命名和文件候选命名。业务内容与旧 evidence 投影分开；Identity/datatype 仅留在明确的兼容元数据包装中。
- `application::attachment_references` 负责有界遍历、目录/文件 Pin、句柄复核、累计 hash 预算和只读引用；不解析 XML 或拼接微信缓存目录。微信内容解析与缓存布局只由 `adapters::wechat::media::attachment_content` 提供，应用层不再重导出适配器类型。旧序列化字段继续在协议边界平铺，未增加 content 嵌套。
- `directory_layout` 拥有聊天目录图片/视频缓存规则；目录宿主消费 typed 内容，继续负责启用开关、账号根约束、受限读取、解码、暂存输出与错误投影。具名媒体拒绝重复节点、命名空间伪装、缺失/无效 MD5、未知节点和超限 XML，不回退为文件名猜测。
- `legacy_dat` 仅供显式历史 DAT 入口，文件系统能力由 resolver 宿主提供。保留本地时区的“前 31 天、当前、后 31 天”顺序，再按目录排序兜底；每个目录内 full > HD > thumbnail。这不是严格图片关联失败后的回退，也不新增账号或输出授权。
- 聊天目录图片依旧 HD > full > wide > thumbnail > wide-thumbnail，同级候选拒绝歧义；其词法月份识别和旧 MsgAttach 布局不改成 legacy DAT 的策略。无 MD5 的附件仍仅允许单候选弱绑定，带警告；多个候选拒绝。相同 MD5 的多份副本仍明确计数，不冒充唯一物理副本。

新增合成测试覆盖 typed 解码、未知/重复/无效 XML、严格拒绝与 legacy 分类的分离、平铺 wire 兼容、两种图片优先级和 legacy 月份兜底。attachment-refs、native-image、mcp-image-listing-parity 与共享 image fixture 接入真实适配器；保留原有安全/歧义/预算测试。本切片按协作要求未运行 Cargo、未提交，编译及统一测试由父任务执行，不能据此宣称通过。

`business::media` 定义媒体种类、关联策略、阶段失败和不透明引用，不持有路径、SQL、密钥或进程。引用绑定消息快照和媒体读取实例；快照失效、来源不完整、歧义和证据冲突不等于未找到，也不能触发兼容回退。

`adapters::wechat::media::resource` 持有资源表 SQL、ChatName2Id 校验和 packed_info 摘要解析。严格 ResourceReader 要求完整消息身份、唯一资源行、真实 INTEGER 身份列及有界 BLOB。ImageSource 绑定资源行和摘要，发现阶段不解码；发布前重新验证证据。摘要扫描仅提供关联线索，不证明明文真实性。

旧 resolver 入口仅为兼容导入。其实际查询位于显式命名的 `legacy_lookup_md5`，仍保留旧时间回退与低位类型匹配行为；严格 ImageSource 不调用它。这个兼容入口仍会将部分旧查询错误当作无结果，不可用作严格关联或授权依据。

语音数据库关联实现在 `media::voice`，ASR 的旧模块仅保留导入兼容。消息定位使用真实 messages::Snapshot，读取 server_id 时拒绝非 INTEGER 值；媒体行号不冒充消息 local_id。MCP prepare/finish 继续使用原有可复核证明，不序列化快照引用、不新增长期查询租约。

## 当前发布限制

严格 native image 已使用 ExportTarget::new_file 与 write_bytes_checked，删除独立临时文件发布实现。共享核心同步内容后调用图片核验回调，回调保留 HostOutputGuard、源 Pin、资源 sidecar、目录清单和最终 proof 检查；随后共享核心再次检查临时文件、父目录和目标再发布。已有输出不能覆盖，回调失败清理临时文件。

普通附件查询使用共享 read_attachment_page。LegacyList 仅跳过旧整数列转换失败，skipped_rows/degraded_content 非零时在原 meta 中附 partial 标记；StrictMetadata、来源、schema、预算错误仍失败。图片元数据请求先有界预检是否存在返回页，空页不访问资源缓存；预检不返回引用，消息文件 Pin 跨异步保持不变，正式读取与关联引用均在同一有效 Snapshot 中使用。

目录图片使用完整来源 Snapshot 与 ImageSource 判定关联，不以导出页内计数证明唯一；缓存键包含来源、local_id、时间和原始类型。目录媒体仍写入归档 staging，服从归档整体发布编排，不单独发布最终文件。

目录表情只扫描既有本地根、核验消息 MD5，不自动下载。显式来源清单包含 emoticon/emoticon.db 时才读取 CatalogSource；私有 URL/AES/Emoji 留在 adapter。关联失败不回退；没有指定 catalog 时保留 message_md5 兼容模式。materialized_catalog_message_md5 仅说明内存 catalog 条目与当前有界本地读取匹配；另有源 Pin/sidecar 核验，不宣称持续磁盘代际证明，也不伪造 MessageRef。旧显式提取入口不属于严格发布统一声明。

## 验证接线

图片密钥样本保留 prepare/stage/publish 两阶段处理：stage 在调用方保存密钥前解码并验证受控内部暂存，不创建最终文件；最终 publish 通过 ExportTarget::write_with_checked 从固定只读句柄流式复制，保留 64 MiB 上限、零化缓冲区、单链接、SHA-256、路径身份和 guard 核验。同步后再次核验源，再由共享核心执行 noclobber 发布。调用方的新密钥保存顺序、既有密钥验证及显式 no_save 分支均未改变；内部暂存不是最终外部产物。

根与独立 fixture 通过 tests/support 的窄模块组合编译真实资源、消息读取和语音实现；不替换数据库查询或关联实现。独立读取 fixture 的 zstd 版本沿用根项目。既有 native-image 与 resolver 合成测试继续覆盖迁移后的实现。编译和测试由父任务统一执行，接线完成不等于测试已通过。

图片 fixture 接入真实 files/config/runtime/setup/private_file。MCP 图片竞争探针在受检发布回调开头注入，仍覆盖同步后、最终核验前的源或输出变化；新增最后证据拒绝时临时文件清理及旧输出保留测试。
