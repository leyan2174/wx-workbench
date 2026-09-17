# 聊天 JSON 转录回写

模块 `writeback.rs` 已由 `application/transcription/mod.rs` 注册，复用 anyhow、serde_json、tempfile、same-file。它负责消息填充与聊天 JSON 原子发布；后端、数据库和缓存由上层编排。

## 接口

- `transcribe_json(&mut Value, FnMut(&VoiceIdentity) -> anyhow::Result<String>) -> Result<WritebackReport>`：同步逐条编排，保留未知字段及原消息，只有成功语音新增 transcription。
- `transcribe_file(&Path, &Path, callback) -> Result<WritebackReport>`：读入导出 JSON，编排后一次原子替换输出。
- `VoiceIdentity { username: String, source: String, local_id: i64 }`：根 username、消息 source、正整数 local_id，不按 local_id 单独缓存或匹配；source 原样传递，不解析成文件路径。
- `WritebackReport`：transcribed、skipped_existing、skipped_non_voice、failed，以及含零基消息下标的 errors。文件级错误直接返回 Err，不宣称结果已落盘。

## 输入规则

根必须是对象，含非空字符串 username 和 messages 数组。只接受精确 `type: "voice"`。非语音及 null 数组项不调用回调。待转录语音缺 source、缺 local_id、错误类型或消息 username 与根冲突时逐条失败。字符串 ID 不自动转换；大于 i64::MAX 的 ID 拒绝。

已有 transcription 字段一律跳过，包括 null、空字符串及非字符串值，绝不覆盖。要重试这类旧记录，由调用方明确清理字段。回调失败或空白结果不新增字段，可重试；不写错误占位文本。回调返回非空文本时原样保存。回调 panic 不捕获。

旧 Python 导出可能没有 source；本模块不会按联系人显示名解析 username，也不会猜测数据库分片，需上游补齐真实身份。

## 文件语义

仅接受 .json 输入输出。已有输出必须为同 username 的聊天 JSON，拒绝数据库内容、其他会话、目录及符号链接。source 仅为标识，不读取、不修改源库。回调必须自行遵守账户隔离及只读数据库契约。

对底层 `writeback::transcribe_file` 而言，同路径明确表示原地更新这个导出 JSON。不同路径不会修改输入；旧输出仅校验，不合并，也不会作为断点恢复来源。恢复时应把上次输出作为本次输入。硬链接目标通过替换目录项写入，不原地截断共享文件。兼容批处理有额外的恢复合并层，见下节，不能把底层规则套用到所有 CLI。

同目录 tempfile，完整序列化、flush、sync_all 后发布；失败不预删目标，临时文件随作用域清理。单条回调失败不回滚其他成功条目。本模块一次提交，崩溃可能丢失此次未提交结果，但不会留下半份 JSON。不保证断电后的目录元数据持久性。

## 并发发布保护

在读取及转录前为规范化父目录下的输出取得 `.文件名.wx-asr.lock` 排他创建锁，文件名转小写以协调 Windows 大小写别名；持有至最终发布完成。锁冲突时不调用 callback。锁文件正常退出时清理；崩溃可能留下锁，必须确认没有活动任务后人工处理，不能自动偷锁。调用者必须使用可信、稳定的父目录；不保证恶意目录重定向、短文件名别名或主动删除锁的行为。

开始时保存输出的完整字节、修改时间和持有句柄的文件身份。转录及序列化后重新打开并比对，拒绝 owner 不变的内容更新、文件身份替换、删除、新建；不是仅再次读取 username。原本不存在的路径使用 persist_noclobber，确保最后瞬间出现的文件也不被覆盖。同路径和硬链接更新仍为替换目录项，不原地截断。

Windows 最终比对句柄只允许 READ | DELETE 共享，普通文件写入在读取比对期间被拒绝；DELETE 共享仍允许非协作 rename。实测 MoveFileExW 在持有拒绝写共享的目标句柄时会拒绝同路径/硬链接更新，因此最终比对后释放检查句柄，立即 persist，全程发布锁仍持有。释放至 persist 之间，不协作的普通写入及 rename/replace 都可能发生；标准路径式替换没有文件身份 compare-and-swap 保证，不能称严格 CAS。本协议保证协作写者互斥，不声称消除所有不协作写者竞态。快照也不是历史变更审计，无法证明修改后恢复相同身份、字节和时间戳的对抗行为。需要抵抗这类写者时，上层必须用目录 ACL/独占工作目录控制写权限。检测到文件级冲突时返回 Err，保留外部版本，不报告转录结果已发布。

## 已接入的上层

- `wx chats transcribe-manifest INPUT OUTPUT --media-manifest FILE --media-root DIR` 通过 `application::transcription::transcribe_chat` 使用此层；清单按完整 username/source/local_id 匹配相对音频路径，不猜数据库分片。后端参数见 [LOCAL.md](../../infrastructure/transcription/LOCAL.md) 和 [OPENAI.md](../../infrastructure/transcription/OPENAI.md)。
- `wx chats transcribe INPUT [OUTPUT]` 通过 `BatchTranscriber::process_file` 和 `batch/files.rs` 工作；输出省略为同目录 `<输入主名>_transcribed.json`。固定账号后自动关联数据库、逐条提交成功缓存及 receipt，最终原子发布聊天 JSON。
- 批处理会从已有输出恢复同 username、规范 source/local_id 的 transcription；两侧有 timestamp 时须相同，有 account_id 时须匹配固定账号。拒绝重复语音身份、旧输出语音不在输入中、转录字段冲突，不合并旧消息集合。任何已有 transcription（包括 null/空值）仍保留并跳过，不偷偷复刻旧 Python 假值重试。
- `BatchTranscriber::process_delta` 仅向 `extras.transcription` 添加结果，保持 raw_content/UID 原始身份字段；有 source_error 或结构错误时拒绝。具体导出命令的发布与错误报告由对应上层负责。

两个文件发布入口使用同一 `.文件名.wx-asr.lock` 协作协议。普通底层回写没有逐条持久化；批处理的逐条成功缓存可在聊天 JSON 未最终提交时保留，但恢复并不意味着源库/配置可以删除。批处理检查非空转录，识别失败或持久化 warnings 会在保存成功文档和报告后通过 CLI 非零状态提醒；`engine_warnings` 仅标识 Python 推理尚存，不等于失败。

## 测试

`writeback_tests.rs` 使用合成 JSON、注入回调及临时目录，覆盖空值、缺身份、跨分片同号、逐条失败、未知字段、重复运行、同路径与异路径、硬链接、源库拒绝和并发发布冲突。测试不调用真实后端或读取账号密钥。

复跑入口见[测试说明](../../../tests/README.md)。同路径发布时，最终身份比对后须释放初始和最终两个检查句柄，发布锁仍持有到提交结束；只关闭其中一个句柄不足以允许 Windows 替换目标。
