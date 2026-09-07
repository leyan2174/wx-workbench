# 源删除后的转录缓存读取：实施前方案

> 以下为实施前历史方案，不是当前接线或待办清单。已由主线批准的简化生产实现取代：receipt 位于现有 cache JSON 顶层 `_wx_asr_receipts`，共用原子事务；账号沿 runtime.id，不新增代次。公开接口为 `receipt::lookup_success` 和 `cached::transcribe_cached_with_receipt_checked`；宿主已接入 `Pending::try_cached`。旧内存规则 fixture 和孤立 target 已撤除，现行主仓及原 ASR harness 命令见 [README.md](README.md)。下文 sidecar、独立账号代次、未接线和未完成等措辞均保留为方案当时的讨论，不据此判断当前测试结果。

历史状态（提出方案时）：设计和独立 fixture；生产 cache-only 缺口仍未完成。未修改生产文件、host/root/IPC 或既有缓存格式。

## 已核实的真实接口

- `cache::ConfigIdentity::new(backend, model_identity, language, options_identity)`：对四元组 JSON 做 SHA256；内部字符串私有，无序列化或摘要读取 API。
- `cache::CacheKey::new(username, source, local_id, audio, config)`：最终键是 `(username, source, message_local_id, lowercase_audio_sha256, config_digest)` 的 JSON SHA256。不是媒体 ID 键，也不含账号。
- `CacheKey::from_audio_sha256(...)`：无需音频即可重建同键，但仅检查摘要语法，不证明摘要来自可信音频。可信来源必须由 receipt 建立。
- `Cache::open(path, account)`：文件头 version=1、account_sha256 隔离；`lookup(&key)`、`store_success(&key,&record)` 可直接复用。禁止自动导入无账号旧缓存。
- `CachedTranscription` 只有 `text, language, create_time: Option<i64>`；记录和最终键都无法反解媒体 ID、音频 SHA、配置或关联证据。
- `cached::transcribe_cached` 仍要求 `CachedRequest.silk`、消息来源/ID/时间；先 `backend.check_authorization()`，再校验 SILK，再私有 `identity(backend, create_time)`，最后 lookup。
- 私有 `identity` 的本地配置身份含模型文件 SHA、程序文件 SHA、规范程序路径、语言、线程数、输出格式、固定管线版本和 create_time；timeout 不入键。云端含端点、模型、语言 Option、音频上限、管线版本和 create_time；不含 API key。
- `verified_digest_recovers_key_without_original_audio` 只证明键重建相等，不证明 host 已支持无源命中。
- `cli/mcp.rs` 目前 bind 后直接 send；`Pending::finish` 要求 prepared_audio，随后才调用 cached。源缺失会在缓存读取前失败。
- `RuntimeContext.id` 来自配置/数据库/密钥文件/运行根的规范路径，不是账号内容指纹；`same_runtime` 也是上下文路径比对。

## 最小持久数据

在显式转录缓存文件旁增加独立、版本化 receipt sidecar；不向 `_wx_asr_cache` 或 entries 添字段，不解析私有最终键，不缓存音频正文、凭证、显示名或转录文本副本。

逻辑索引是 `(account_binding, exact_username, media_local_id)`，只能定位候选，不能直接决定缓存命中。候选 receipt 保存：

- version、runtime_id、独立账号代次/绑定（见下文）、精确 username、正 media_local_id。
- 完整 VoiceEvidence：message_source/table/local_id、server_id、create_time、media_source/rowid/chat_name_id/local_id。
- 原始 SILK 的 SHA256、字节长度；必须对含可能 0x02 前缀的原始字节计算，与现有 CacheKey::new 一致。
- 当前算法生成的 config_identity SHA256、backend 名、身份算法版本；不保存可直接授权的标志或凭证。
- 已落盘成功 record 的规范字段摘要（text/language/create_time 的确定性编码），防止把另一个记录当成 receipt 所指结果；此摘要不是签名。

同一逻辑索引可以有多个配置的 receipt；先检查历史音频/关联证据是否冲突，再按当前配置精确选定。配置不同本身不是身份歧义；同一媒体 ID 出现不同强证据则保留冲突，禁止最新覆盖、首条优先或按“哪个还能命中”挑选。重复的相同 receipt 幂等。

receipt 在可信本地缓存目录内具有与现有转录缓存相同的信任边界。JSON 自带 SHA 不防可写该目录的恶意本地用户；不能把未经验证的导入文件包装成“strong proof”。若要求防本地伪造，应另行设计受保护密钥/MAC，不在本次最小方案中假称具备。

账号绑定不能从工具传入的 chat 猜。建议同时绑定既有 runtime.id 与主线持有的显式账号代次标识；同路径更换账号或重新导入另一账号时必须更换代次。现有代码没有此强账号代次 API，需 main 决定提供方式。仅复用 runtime.id 可以满足既有不同路径账号隔离，但不能宣称防同路径账号替换。不得自动生成新代次后收养旧 receipts。源记录删除不得改变已确认的账号代次。

## 待批准的最小 API

以下均为建议接口，不是已存在函数：

1. `cached::prepare_cache_identity(&Backend, create_time) -> PreparedCacheIdentity`：从现有私有 identity 提取共享实现，保留授权前置、文件句柄、现有字节编码及字段顺序。对象持有 ConfigIdentity 和本地文件句柄，可供现有 transcribe_cached 与无音频 lookup 共用；不执行 ASR 或下载模型。
2. `ConfigIdentity::fingerprint(&self) -> &str`：只读现有摘要，用于 receipt 精确匹配，不新增从任意摘要构造身份的公开入口，也不改变缓存格式/键算法。
3. `receipt::lookup_success(store, AccountBinding, ExactUsername, media_id, &Backend) -> LookupOutcome`：结果分 Hit、Miss、Conflict、Unavailable；授权先于 receipt/cache 读取，逐候选使用其已验证 create_time 计算当前配置身份、匹配 config SHA，再调用真实 from_audio_sha256/open/lookup；复核 record 时间和摘要。不调用 transcribe_cached，不要求 SILK。
4. `receipt::record_success(... verified voice, prepared identity ...) -> ReceiptStatus`：只接受本轮已验证的 DatabaseVoice 和明确账号绑定；重查已落盘缓存 record 并校验时间及内容后记录 receipt。不能仅凭 CachedOutcome::AlreadyPresent 推断磁盘记录等于本轮识别结果。
5. host 内部 `Pending::try_cached(exact_username, account_binding, context, before_commit)`，在 send 之前调用；Hit 直接生成旧文本响应，Miss 才去准备音频。保持正常路径与 cache-only 使用相同响应检查。

当前 CachedOutcome 不携带 config identity 或成功键。建议把 prepared identity 的生命周期移到共享调用层，避免先算一次、ASR 内又算一次造成配置变化竞态。不要复制 identity 的私有算法，也不要从 receipt 直接恢复一个不经当前配置核验的 ConfigIdentity。

## Host 与 IPC 顺序

1. 校验 schema、媒体 ID、预算和显式 backend/upload 授权；绑定并复核当前账号。cache-only 不自动切换后端、不下载模型、不启动识别进程、不上传。
2. 获取精确 username：请求字符串与已验证 receipt 的 username 完全相等时可走直接候选路径，不做大小写折叠/模糊匹配。显示名称需要主线新增内部“仅解析精确联系人”的 IPC，返回当前账号内唯一 username；不依赖语音消息或媒体 BLOB。歧义拒绝；联系人已删除且只给显示名时不得利用历史别名猜测，要求显式 username。
3. Transcribe 且配置了缓存时先 try_cached。Hit 不发送音频准备 IPC，不要求 daemon 成功读取消息。复核账号绑定、路径守卫、预算、取消状态和完整响应大小，再返回 `[本地时间] (language)\ntext`；空文本仍为成功。
4. Miss 才使用已有 TranscribeVoice -> q_prepare_voice 音频 IPC。返回后校验媒体 ID、精确 username 与请求绑定、全部 prepared evidence 和音频 SHA，再走已有 ASR 缓存/转录流程。不能只校验 media ID。
5. 成功结果按现有缓存格式存储，确认磁盘成功记录后才发布 receipt；两文件间崩溃允许留下无索引缓存，不允许先发布指向尚未存储结果的 receipt。receipt 写入失败不改变本次成功识别，但此条未来无源命中仍不可用，需内部状态标明。
6. Conflict 不挑一个旧结果返回；可在 main 明确的正常源流程中继续获取当前证据，但不能静默消除历史冲突。Unavailable 不读弱缓存；有源时可继续普通转录，无源时给安全失败。任意 IPC 错误后都不启动“全局弱 ID”兜底。

精确 username 的 cache hit 不需要新音频 IPC 类型；显示名称路径需要窄联系人解析 IPC。host/root 接线必须由 main 批准后实施。不得为了 cache hit 先调用会拉起 daemon、依赖完整消息清单的 transport。

## 文件与配置边界

- sidecar 与 cache 使用相同显式受保护目录，并互相列入输入/输出保护，拒绝源数据库、模型、凭证路径及别名；校验祖先重解析点、常规文件、硬链接别名、字节/记录/候选上限、版本、重复字段和范围。
- sidecar 发布需独立协作锁、同目录临时文件、sync、快照复核和无覆盖首次提交；现有文件受控更新，坏文件原样保留。不能把 fixture 的 fs::write 当生产发布算法。
- lookup 全程只读，不修复、不创建目录、不迁移、不回写命中次数。提交前复核句柄和账号状态；不刷新调用预算。
- 本地现有 identity 仍需模型和程序文件存在，读取摘要不等于启动后端。源消息/音频删除但这些文件仍存在是本方案直接覆盖的最低范围。
- 如果要求模型/程序文件也删除后仍命中，不能直接相信 receipt 的旧 config SHA。需 main 批准独立、显式固定模型/程序摘要的配置身份输入，并定义路径和选项一致性；不能自动下载恢复。此项不在当前真实 API 能力内，不能宣称全量旧行为已完成。
- 云端 hit 必须再次满足当前显式 upload 授权和当前 backend 配置验证；receipt 不携带可绕过授权的历史许可。API key 不入键不代表可跳过授权。沿用当前 BackendArgs::build 的凭证要求，除非另行明确修改策略。
- 既有强缓存没有 receipt 时，不能从最终 hash 逆推或扫描弱 ID：有源时验证后补建，无源时无法安全自动恢复。旧无账号 Python cache 同样不自动迁移，需有独立可信映射的显式迁移方案。

## 验收与未完成项

独立 lib.rs 只验证真实缓存 API 加最小内存选择规则：持久 receipt 后删除合成源仍读成功空文本、账号/username/media ID 隔离、source/message ID/audio SHA/config 改变拒绝命中、历史强证据冲突拒绝、缺记录及时间不符拒绝、未授权在缓存 IO 前停止。config 为明确合成值，不复制 cached::identity；授权布尔只测试顺序，不冒称真实 cloud 集成覆盖。

生产验收还必须覆盖：真实 identity 各字段变化与 timeout 不变性、真实未授权云端 hit 拒绝、账号代次/切换、联系人 IPC 精确与歧义、配置文件缺失策略、同 ID 重用、多配置候选、崩溃恢复/并发写/坏文件保留、路径别名、取消/超时/响应超限、不下载不运行后端的调用计数，以及真正 host 源消息删除端到端测试。

本设计落地并通过上述主线验收之前，“源删除后 transcribe cache hit”保持未完成；fixture 成功不改变该状态。
