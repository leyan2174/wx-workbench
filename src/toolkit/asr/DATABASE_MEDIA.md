# ASR 只读数据库语音关联

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

`resolve_voice(&Path, MessageIdentity<'_>) -> Result<DatabaseVoice, DatabaseMediaError>` 是显式目录查询入口。输入根目录必须绝对路径且为调用方明确选择的单账号、完整、静态已解密快照。`source` 必须完整写成 `message/message_N.db`，允许 Windows 反斜杠；裸文件名、绝对 source、上级目录、URI、ADS 均拒绝。不从媒体分片编号猜消息分片；此核心不自动发现账号、不读配置/密钥、不云上传、不调用模型、不创建输出。

模块已在 `asr/mod.rs` 注册；生产依赖复用 `rusqlite`、`md5`、`same-file` 和标准库，测试用 `tempfile`。其他已接线的公共入口：

- `resolve_voice_sources(&[DecryptedSource], MessageIdentity, exact_time)` 接受调用方固定的完整来源清单，允许实际文件位于散列缓存路径；`source` 保留原始规范根相对身份。`None` 不筛时间，`Some(0)` 精确匹配零时间。
- `resolve_voice_media_id(&[DecryptedSource], username, media_local_id)` 用于 MCP 旧媒体 ID：先证明唯一媒体行，再按 username/server_id 在全部消息分片反查唯一消息，最后复用正向关联。绝不把媒体 ID 当成消息 ID。
- `source_files(root)` 供上层枚举相关源文件；清单入口拒绝重复来源、同文件别名及活动库，持有所有源的只读保护直到查询和结束复核完成。

## 当前 CLI 与宿主

`wx toolkit transcribe-database-native --decrypted-dir ABS_DIR --username USER --source message/message_0.db --local-id 7` 已接入 `cli/asr_database.rs`，还必须提供显式后端参数。默认本地 whisper.cpp，需 `--whisper-binary/--whisper-model`；语言默认 `auto`、超时 120 秒、线程自动且最多 8；云端需显式后端、端点、模型、key 文件和 `--allow-upload`。完整后端契约见 [LOCAL.md](LOCAL.md) 和 [OPENAI.md](OPENAI.md)。

可选 `--cache-file FILE --cache-account NAME` 必须成对，NAME 非空且只是调用方命名空间；缓存文件须为独立可信目录中的 JSON，不能覆盖数据库、程序、模型或凭证。后端授权检查先于数据库和音频访问。输出是 `transcription`、双侧 `evidence` 和可选 `cache` 状态；明确返回 `account_authenticated=false`、`account_provenance="caller_supplied_decrypted_snapshot"`，不序列化原始音频或凭证。

`wx toolkit transcribe-chat` 及转录导出流程已通过 `batch::prepare_snapshot` 固定账号并准备私有完整静态解密快照，再调用清单入口和字节转录；不需要用户手写媒体清单。MCP daemon 用媒体 ID 入口准备受限 `prepared_audio`，宿主验证后执行解码/转录；不是 daemon 直接运行后端。显式媒体清单的 `transcribe-chat-native` 仍作为另一入口保留。

## 关联证据

- `vendor/wechat-decrypt/transcribe_chat.py::_transcribe_local_id` 只传 username/local_id；`mcp_server.py::_fetch_voice_row` 在所有媒体分片中返回首个同 local_id。这不能保持导出 source 语义，本实现不复刻该歧义回退。
- `vendor/wechat-decrypt/export_messages.py` 实际读取 `Msg_<md5(username)>.server_id`。
- 关联核心使用 `VoiceInfo.svr_id`、`local_id`、`create_time`、`chat_name_id`，并用同媒体库 `Name2Id.rowid/user_name` 证明联系人；当前接线以本文件上方的公共入口为准，不依赖旧 CLI 私有读取函数。
- `toolkit/audio/batch.rs` 也以 `Name2Id.rowid → VoiceInfo.chat_name_id` 关联联系人，但直接遍历媒体、不读取消息来源。这些函数是私有函数或附带配置、输出职责，不适合作为此核心的无副作用公共 helper，因此未修改其可见性。
- `daemon/query/export.rs` 以完整分片 source 和 `Msg_<md5(username)>` 表导出 local_id。本实现沿用这个消息身份，不将媒体 local_id 当作消息 local_id。

查询先在指定 source 的实际消息表中唯一定位 local_id，并检查低 32 位类型为 34、server_id 非零。然后只枚举此根目录 `message/media_N.db`，精确按 username 找本库 Name2Id，再以 `svr_id = message.server_id` 查媒体。必须唯一，create_time 必须相同。媒体 local_id 即使不同也保留原值作为证据。重复消息、重复联系人映射、同库或跨库多候选均报错误，不按顺序、时间接近或字节相同擅自去重。

返回证据：username、message_source/table/local_id、server_id、create_time、media_source/rowid/chat_name_id/local_id。错误使用可匹配的 `ErrorKind` 和静态 stage，不输出绝对路径、SQL 参数或语音内容。

## 安全与限制

- SQLite 使用 `READ_ONLY + URI(mode=ro, immutable=1) + query_only`，不创建 SQLite sidecar。打开前后拒绝任何 WAL/SHM/journal；仅支持静态主库副本，不能忽略活动 WAL 当作最新数据。
- Windows 源数据库句柄在查询期间拒绝写入与删除；检查媒体分片集合及主库大小/修改时间变化。路径拒绝符号链接、重解析点和网络快照。不宣称抵御恶意目录替换或同用户主动篡改。
- 输入根目录是账号信任边界。已解密库没有可靠的统一账号证明，不能检测调用方把别的账号文件复制或硬链接进此根目录；两个独立根目录绝不相互搜索，但这不等于验证文件的来源账号。
- 没有 server_id/svr_id 的旧 schema、未发送语音的零 server_id、未知分片命名、缺必需表列都拒绝。没有采用仅 local_id、时间戳或 data_index 推断的降级关联。**这是可证明 schema 的核心，不是全部版本语音关联已完成。**
- 不知道显式快照之外是否缺少媒体分片，调用方必须保证目录或清单完整。目录枚举最多 1024 个媒体分片、4096 个目录条目；显式来源清单最多 2049 项。每条 BLOB 最大 16MiB。原始数据仅验证 SILK_V3 头，真实帧完整性留给已有 SILK 解码器。
- server_id 与 svr_id 的关联基于已核对字段及一致性约束；合成测试证明实现行为，不代替真实版本语料上的关联验证。本任务未读取真实数据库。
- 核心不生成临时音频或媒体清单。已接入 `asr::transcribe_audio_bytes`，直接消费 SILK 字节，复用原生解码/校验后分发后端；本地后端可以产生受控临时 WAV，云端在显式授权后发送 WAV，不能把核心只读边界扩大解释为整条转录链无副作用。

## 合成回归

测试文件 `database_media_tests.rs` 覆盖完整字节一致、原始前缀、同号跨消息/媒体分片、同号跨联系人/账号、精确大小写、消息和媒体重复、缺行缺库、缺证据列、时间冲突、零 server_id、路径注入、sidecar、不支持媒体内容及查询前后全部数据库字节/文件集合不变。清单入口、MCP 媒体 ID 反查和数据库 CLI 另有生产模块合成测试；公共注册已完成，统一回归由主线程执行。

历史独立结果：Windows `x86_64-pc-windows-msvc` 编译成功，18 个测试全部通过（0 failed，0 ignored）。另覆盖中文/空格/%/# 根目录的 URI 编码、rowid 用户列遮蔽、损坏库和缺媒体库不创建文件。当时日志：`C:\CodexLocal\日志\database-media-tests.log`；程序：`C:\CodexLocal\日志\database_media_worker_tests.exe --nocapture`。本次未重跑，不能据此宣称真实数据库或模型验收。

2026-09-07 文档同步期间集中重跑 check/test 均通过：check 有 9 条 warnings；全量测试退出 0，20 组 1325 passed / 0 failed / 11 ignored，日志 `C:/CodexLocal/wx-cli-doc-sync-tests.log`。18 项实际 exe `--help` 全部通过，日志 `C:/CodexLocal/wx-cli-doc-help-check.log`。UI 61 / 0 和额外 8 个原忽略项显式通过为已有验证结果，本轮未重跑；默认忽略项不计为通过。真实账号、模型质量、GPU、云端及发布验收尚未完成。
