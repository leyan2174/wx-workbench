# 原生转录缓存核心

## 旧实现历史证据

以下旧 Python 文件及行号用于解释兼容差异，不是当前运行依赖；行号属于早期迁移审计快照。

- `vendor/wechat-decrypt/transcribe_chat.py` 的 `_transcribe_local_id` 直接调用 `_transcribe`，不读写独立缓存；导出 JSON 的 transcription 是其重跑状态，并且实际 pending 判断为假值（不只是字段缺失）。
- `mcp_server.py:3700` 使用 SCRIPT_DIR/voice_transcriptions.json；3707 的键是 JSON 编码的 [username, local_id]，不是账号或分片身份。
- 3712 懒加载 dict，坏文件降级空 dict；3734 临时文件 + os.replace，写失败只警告，不让成功识别消失。
- 3872 签名只含 backend/model_size，whisper_cpp 只取模型 basename。
- 4030 开始的 transcribe_voice 在数据库查询前检查缓存，旧条目缺 backend 按 local；4080 附近保存 text/language/create_time/backend/model_size，成功空文本也缓存，失败不写。

## 当前接线与核心 API

`cache/cached/receipt` 已在 `asr/mod.rs` 注册，并接入 MCP 宿主、显式数据库 CLI 和固定账号批处理。核心本身不发现账号、后端或缓存位置，也不实现过期清扫；Python 命名模型下载属于独立推理桥，见 [LOCAL.md](LOCAL.md)。

- MCP 用 `--voice-cache-file FILE` 显式启用；省略不持久化。账号取绑定的 `RuntimeContext.id`，不接受工具参数提供的账号或路径。
- `wx toolkit transcribe-database-native` 用成对的 `--cache-file FILE --cache-account NAME` 启用；缓存目录须独立于数据库/后端输入。NAME 只是调用方命名空间，不是账号认证。
- 配置式 `transcribe-chat` 等批处理使用固定账号运行目录下 `batch-transcriptions.json`；`--asr-cache-name` 只能是合法单个 JSON 文件名。成功缓存逐条提交，聊天 JSON 最后发布；缓存/receipt 持久化告警独立于逐条识别结果，批处理 CLI 会在保存报告后以非零状态提醒。

`Cache::open(path, account)` 显式账号和路径；`ConfigIdentity::new(backend, model_identity, language, options_identity)`、`CacheKey::new(username, source, local_id, audio, config)` 创建结构化身份摘要；`lookup`、`store_success`、`store_success_checked` 为核心入口。`StoreStatus` 只有 `Stored/AlreadyPresent`；不存在旧草稿中的 `store_result/FailureSkipped`。核心仅接收成功记录，后端错误由 `cached` 返回 Err、不写缓存。成功空文本可保留，返回 create_time；批处理回写另有非空文本要求，不能把 MCP 空成功规则套用于聊天 JSON。

账号仅保存 SHA-256；键包含联系人、分片、local_id、音频 SHA-256 和配置 SHA-256，不落盘原音频或配置明文。配置签名必须包含所有会影响结果的非秘密配置：模型内容摘要/不可变版本、语言、后端端点身份和推理选项；模型 basename 不足以代表内容。API 没有 api_key 字段，调用方禁止将凭据混入配置身份、错误或转录正文。正文是缓存必要内容，应保护缓存文件访问权限。

新文件使用 `_wx_asr_cache` 版本/账号封套和 entries。不能把缺账号、分片、音频身份的旧记录当成安全命中；拒绝自动认领旧格式，原文件不动。旧缓存迁移必须提供可信完整身份映射，不伪造迁移。不同于旧版，损坏文件报错且不覆盖。删除音频后恢复必须依赖事先验证并保存的身份/摘要或 receipt；不能只凭旧 username/local_id 记录补造证据。

`CacheKey::from_audio_sha256` 接受上层保存的已验证 SHA-256（64 位十六进制），与原音频入口生成相同键，支持原音频已清理后的缓存读取；不扫描数据库、不补猜摘要。

同键已有条目不覆盖；其他记录及根/条目未知字段保留，未知格式同键拒绝更新。每次写入在锁内重新加载，避免协作实例丢更新。64MiB 文件上限；同目录临时文件 flush/sync 后原子发布，新文件不覆盖。锁崩溃残留不自动偷锁。已有文件发布前比对字节、时间、身份，Windows 替换前释放句柄；非协作写者仍有最终检查到 rename 的竞态，不是严格 CAS。目录必须可信稳定；不保证权限对抗或断电目录持久性。

依赖复用 anyhow、serde(derive)、serde_json、sha2、same-file、tempfile。缓存核心不依赖具体后端；`cached` 已适配三个后端，见下文。

历史独立验证（2026-09-07，非当前全仓统计）：rustc --test Windows MSVC 编译无警告，8 passed、0 failed、0 ignored。覆盖重启读取、空成功、账号/分片/音频/配置隔离、失败不写、未知字段保留、拒绝认领旧缓存、并发冲突、跨实例合并及仅凭可信音频摘要恢复键。当时日志：C:\CodexLocal\日志\asr-cache-compile.log、asr-cache-tests.log。模块现已注册，本次文档编辑没有重跑这些测试。

## 字节转录薄适配

`cached::transcribe_cached(&CachedRequest, &Backend) -> anyhow::Result<CachedOutcome>`。
请求显式含 cache_path/account/username/source/local_id/create_time/silk；返回
transcription/create_time/cache_state。缓存状态为 Hit、Stored、AlreadyPresent、
ReadUnavailable、WriteUnavailable。读取失败（包括账号不符）不使用旧记录且
不覆盖该文件，仍允许对当前明确音频执行后端；落盘失败不丢成功识别结果。
后端失败直接 Err，不缓存；成功空文本允许 Hit。错误状态不包含缓存正文或凭据。

本地身份包含模型完整 SHA-256、程序完整 SHA-256 与规范程序路径、语言、线程、
输出格式、no-fallback、字节流水线版本和 create_time；不是模型 basename。
timeout 仅是执行预算，不再参与成功缓存键；更改预算不使成功条目失效。
旧含 timeout 的摘要记录不删除，但不能直接命中新摘要。
每次哈希流式读文件，Windows 文件句柄保持到返回并拒绝写/删除；程序依赖 DLL、
驱动、系统环境及目录重定向不在此内容绑定范围，必须使用可信、固定部署目录。
命中前用已有 normalize_silk 校验容器，未命中只调用真实 transcribe_audio_bytes，
不复制解码/进程/HTTP逻辑。缓存目录须可信且账号隔离，无严格 CAS 或目录沙箱保证。

云端使用后端所有者提供的非秘密 `cache_identity` 访问器，绑定最终请求端点、
模型、语言 Option、音频上限、字节流水线版本和 create_time。API key 不参与
身份，因此仅轮换凭据仍可命中；所有命中均先通过父模块授权检查。
自动语言与显式语言独立。配置仅以摘要落盘，不保存原始端点、模型或凭据。
云端模型若使用可变别名，服务端更新无法从本地配置推断；需要调用方指定版本
或换用新缓存文件，本模块不伪造不可变模型保证。
32MiB 输入限制先于容器校验、哈希、模型和缓存访问。CacheState 支持 snake_case
JSON 序列化，公开 create_time 保留供上层报告使用。

历史适配实跑：Windows MSVC 独立 harness 引用真实 ASR mod/audio/cache 和 CLI 叶模块，
编译无警告，cached_tests 6 passed、0 failed、0 ignored。日志为
C:\CodexLocal\日志\asr-cached-compile.log 与 asr-cached-tests.log。
本地命中次数、空成功、错账号/音频/同名模型/程序内容变更、后端失败和持久化
失败均验证；本地模型只是合成 fixture，未运行真实模型。

主工程已注册 cache/cached/receipt；公共模块及 CLI 接线由主线程统一维护。
云端回环测试现在实际传输 WAV multipart，验证首次上传、重复命中、凭据轮换命中、
端点/模型/语言/音频上限隔离，以及命中前拒绝未授权请求。仅连接本机合成服务，
不调用真实云端模型。此前独立测试数字是历史记录，不代表最新整仓回归。

## Python 身份与 receipt

`LegacyPythonLocal` 身份绑定 Python 程序摘要/路径、模型根、桥源码及桥返回的引擎摘要。后者包含模型文件摘要或命名模型描述、Whisper/PyTorch/Python 版本、引擎源码摘要、CPU/CUDA 设备、线程、语言等。取得身份可能启动 Python 并导入第三方包；模型加载和识别延迟到实际请求。因此不能把缓存命中统称为“完全不启动进程”，也不能把版本/环境摘要视为对全部依赖和驱动的不可变认证。

`cached::transcribe_cached_with_receipt_checked` 用 `VoiceEvidence` 将成功记录与媒体 ID 索引同次发布；`receipt::lookup_success` 可在 MCP 语音 IPC 前按精确 username + media ID 查历史成功，校验账号、配置、证据和记录摘要。结果为 Hit/Miss/Conflict/Unavailable；索引最多 65536 个身份，每个身份最多 64 个配置。精确 username 命中不要求源语音或 daemon 仍存在；显示名必要时仍需 ResolveChat。whisper.cpp 命中仍需程序/模型文件计算身份，Python 命中可能需启动引擎身份 worker；不承诺后端依赖可一并删除。

receipt 命中不修改缓存；源仍在时，强身份成功缓存可经过真实 evidence 校验补写缺失 receipt，而不是重新识别或认领弱旧记录。receipt 不是签名，不能证明当前消息仍存在、账号数据未被同路径替换，或抵御同用户恶意篡改。普通批处理仍先准备快照并关联源语音，不等同 MCP 的无源 receipt 快速路径。

checked 接口在识别后预检，并在暂存、sync、快照复核后且实际发布前允许再次拒绝。MCP 宿主检查真实请求 ID 的完整文本预算、账号、context 和路径守卫，并保留拒绝的 DispatchError；普通缓存 I/O 失败以独立状态报告。成功发布后的取消或 stdout/IPC 断开不回滚缓存，不保证恰好一次。

## 当前验证范围

2026-09-07 文档同步期间集中重跑 check/test 均通过：check 有 9 条 warnings；全量测试退出 0，20 组 1325 passed / 0 failed / 11 ignored，日志 `C:/CodexLocal/wx-cli-doc-sync-tests.log`。18 项实际 exe `--help` 全部通过，日志 `C:/CodexLocal/wx-cli-doc-help-check.log`。UI 61 / 0 和额外 8 个原忽略项显式通过为已有验证结果，本轮未重跑；默认忽略项不计为通过。真实账号、模型质量、GPU、云端及发布验收尚未完成。
