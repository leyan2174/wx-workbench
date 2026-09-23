# 只读数据库语音关联

## 接口

```rust,ignore
let voice = database_media::resolve_voice(
    decrypted_root,
    database_media::MessageIdentity {
        username: "alice",
        source: "message/message_0.db",
        local_id: 7,
    },
)?;
// voice.silk 原始字节不去掉 0x02 前缀；voice.evidence 为双侧关联证据。
```

`resolve_voice(&Path, MessageIdentity<'_>) -> Result<DatabaseVoice, DatabaseMediaError>` 是显式目录查询入口。输入根目录必须绝对路径且为调用方明确选择的单账号、完整、静态已解密快照。`source` 必须完整写成 `message/message_N.db`，允许 Windows 反斜杠；裸文件名、绝对 source、上级目录、URI、ADS 均拒绝。不从媒体分片编号猜消息分片；此核心不自动发现账号、不读配置/密钥、不创建输出。

实现位于 `adapters::wechat::media::voice`；生产依赖复用 `rusqlite`、`md5`、`same-file` 和标准库，测试用 `tempfile`。其他公共入口：

- `resolve_voice_sources(&[DecryptedSource], MessageIdentity, exact_time)` 接受调用方固定的完整来源清单，允许实际文件位于散列缓存路径；`source` 保留原始规范根相对身份。`None` 不筛时间，`Some(0)` 精确匹配零时间。
- `resolve_voice_media_id(&[DecryptedSource], username, media_local_id)` 用于 MCP 旧媒体 ID：先证明唯一媒体行，再按 username/server_id 在全部消息分片反查唯一消息，最后复用正向关联。绝不把媒体 ID 当成消息 ID。
- `source_files(root)` 供上层枚举相关源文件；清单入口拒绝重复来源、同文件别名及活动库，持有所有源的只读保护直到查询和结束复核完成。

## 原始语音与宿主

此适配器提供只读消息与媒体关联证据。`wx voices` 的媒体目录导出与严格消息关联是不同契约；完整聊天导出保留语音引用，原始 SILK 及关联 manifest 供下游工具消费。宿主负责固定账号、准备来源、路径保护和文件发布，不把媒体 local_id 当作消息 local_id。

CLI 原始语音导出与 `export_voices` 任务统一使用宿主 `prepare_voice_snapshot` 准备独立来源。`VoiceSnapshot` 持有通过 SQLite Backup 生成的 `ResourceSnapshot`，将单库已提交数据（包括 WAL 中已提交的数据）纳入私有副本，仅对副本设置 `journal_mode=DELETE`。这不放宽下述严格读取边界：读取器仍拒绝侧车，宿主不删除源侧车或修改源库日志模式，逐库副本不保证跨库原子一致性。

## 关联证据

- 消息定位必须包含完整 source；不以 username/local_id 在媒体分片中选取首个同 local_id 记录，歧义必须报错。
- wechat-decrypt 导出参考实现提供 `Msg_<md5(username)>.server_id` 字段语义；本适配器的关联规则由测试和类型化证据独立固定。
- 关联核心使用 `VoiceInfo.svr_id`、`local_id`、`create_time`、`chat_name_id`，并用同媒体库 `Name2Id.rowid/user_name` 证明联系人；调用方通过本文件上方的公共入口访问，不依赖 CLI 私有读取函数。
- [原始语音导出工作流](../../../daemon/operations/voices.rs) 使用媒体导出目录 `voice_export::Catalog` 遍历媒体，并调用本适配器的 `resolve_voice_media_row` 核对消息关联；固定账号、来源准备、输出发布与计数由 daemon 操作宿主负责，不提升为微信适配器的公共 helper。
- [聊天查询导出](../../../daemon/query/export.rs) 以完整分片 source 和 `Msg_<md5(username)>` 表导出 local_id。本实现沿用这个消息身份，不将媒体 local_id 当作消息 local_id。

查询先在指定 source 的实际消息表中唯一定位 local_id，并检查低 32 位类型为 34、server_id 非零。然后只枚举此根目录 `message/media_N.db`，精确按 username 找本库 Name2Id，再以 `svr_id = message.server_id` 查媒体。必须唯一，create_time 必须相同。媒体 local_id 即使不同也保留原值作为证据。重复消息、重复联系人映射、同库或跨库多候选均报错误，不按顺序、时间接近或字节相同擅自去重。

返回证据：username、message_source/table/local_id、server_id、create_time、media_source/rowid/chat_name_id/local_id。错误使用可匹配的 `ErrorKind` 和静态 stage，不输出绝对路径、SQL 参数或语音内容。

## 安全与限制

- SQLite 使用 `READ_ONLY + URI(mode=ro, immutable=1) + query_only`，不创建 SQLite sidecar。打开前后拒绝任何 WAL/SHM/journal；仅支持静态主库副本，不能忽略活动 WAL 当作最新数据。
- Windows 源数据库句柄在查询期间拒绝写入与删除；检查媒体分片集合及主库大小/修改时间变化。路径拒绝符号链接、重解析点和网络快照。不宣称抵御恶意目录替换或同用户主动篡改。
- 输入根目录是账号信任边界。已解密库没有可靠的统一账号证明，不能检测调用方把别的账号文件复制或硬链接进此根目录；两个独立根目录绝不相互搜索，但这不等于验证文件的来源账号。
- 没有 server_id/svr_id 的旧 schema、未发送语音的零 server_id、未知分片命名、缺必需表列都拒绝。没有采用仅 local_id、时间戳或 data_index 推断的降级关联。**这是可证明 schema 的核心，不是全部版本语音关联已完成。**
- 不知道显式快照之外是否缺少媒体分片，调用方必须保证目录或清单完整。目录枚举最多 1024 个媒体分片、4096 个目录条目；显式来源清单最多 2049 项。每条 BLOB 最大 16MiB。原始数据仅验证 SILK_V3 头，真实帧解码与可播放性由下游工具验证。
- server_id 与 svr_id 的关联基于已核对字段及一致性约束；合成测试证明实现行为，不代替真实版本语料上的关联验证。
- 核心不生成临时音频或媒体清单，不执行解码或识别；关联证据不能替代文件发布结果。

## 合成回归

测试文件 `voice_tests.rs` 覆盖完整字节一致、原始前缀、同号跨消息/媒体分片、同号跨联系人/账号、精确大小写、消息和媒体重复、缺行缺库、缺证据列、时间冲突、零 server_id、路径注入、sidecar、不支持媒体内容及查询前后全部数据库字节/文件集合不变。清单入口、MCP 媒体 ID 反查和数据库 CLI 另有生产模块合成测试。


URI 编码、用户列遮蔽 rowid、损坏库及缺库不创建文件也属于回归范围。命令与依赖见[测试说明](../../../../tests/README.md)。
