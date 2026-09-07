# 历史架构图快照

本目录保存更新前的图件与说明。以下“当前”“本轮”“待完成”只指当时快照，不是现行架构；当前图请从 [图件导航](../../README.md) 进入。本次只补充历史标识并修正归档后的相对链接，旧测试数字、失败和待验收记录保留。

## 当前快照

2026-09-07 六条开发线接入后的当前结构以 [architecture.md](../../../architecture.md) 的 Mermaid 和 [runtime-current.svg](runtime-current.svg) / [PNG](runtime-current.png) 为准。本轮只更新运行总图、本文及架构正文；SNS、ASR/WASM、MCP 语音专项图与 `source-evidence.json` 保留原阶段快照，不代表本轮重新核验。

运行总图保留第 01–16 行既有详细链路，更新共享双 binary 入口，并新增第 17–23 行：setup/init 共享配置快照、ASR batch 三后端、Axum 与原生 JavaScript、Web 白名单任务和受控子进程、cleanup 计划执行、企业微信批量与 monitor/latency、当前证据和实际缺口。`wx-toolbox` 的 `include!("main.rs")` 共享全部业务模块，默认 GUI 是本地浏览器。`local_python` 仍调用 Whisper/PyTorch，命名模型仍可下载权重；原生路由存在不等于纯 Rust、全 GUI 覆盖或全旧功能兼容。

| 主任务提供的本轮证据 | 状态 |
| --- | --- |
| maincheck5 | 两个 binary MSVC check 通过，22 条警告 |
| CLI 专项 | 63 passed |
| 主单元测试 | 801 total：794 PASS、0 FAIL、7 ignored，约 203 秒 |
| ASR security | 8 PASS、0 FAIL |
| 新增 setup 测试 | 9 项已添加，未运行 |
| 集中回归 | 全量测试曾因 ASR 测试缺失依赖及 chat-plan 参数问题停在 Cargo 测试编译阶段，并非已执行用例失败；现专项恢复，最终全量待跑 |
| 实际兼容缺口 | 扫描候选64..192/跨库HMAC/多PID/同salt多DB、图片批量 skipped_no_key/格式、离线 UIN-MD5 尚未全部验收；扫描修复及3项测试、图片跳过修复及4项测试已交付未运行；Main CLI offline 互斥内存模式已加，离线 helper 待完成 |
| UI 实际渲染 QA | 29 PASS、0 FAIL；实际前端资产与合成 API，覆盖1440/390视口。队列底栏 CSS 修复已完成，不代表其他功能修复或完整生产端到端验收 |

运行总图第 19、23 行的 UI 待复跑文字保留生成时快照，以本表最新状态为准。本次准确性更正仅修改两份 Markdown，不重绘 SVG/PNG；最终统一收口由主任务完成。

本轮文档不运行 cargo、不使用真实账号、不验证模型质量或发布包；不宣称全目标完成。图件色标表示源码路径或状态，不是测试通过标记。旧脚本保留不代表 `export-chats`、`transcribe-chat`、`run`、`web/gui` 仍调用 Python 脚本；仅本地 ASR 推理兼容层等实际外部依赖单独列示。

### 本轮复现

沿用项目 Python SVG 生成器和 Playwright/Edge 导出器，仅选择运行总图；不安装新绘图工具链：

```powershell
python docs\diagrams\render_architecture.py --diagram runtime-current --defer-evidence
node docs\diagrams\export_png.cjs C:\Users\leyan\.cache\codex-runtimes\codex-primary-runtime\dependencies\node\node_modules\playwright runtime-current
```

`--defer-evidence` 在并行源码仍变更时不封存旧锚点。源码最终复核与证据封存由主任务协调，本轮不声称 `--check-source` 通过。`render-report.json` 中只有 runtime-current 项属于本轮重绘，其他项仍是历史记录；图形校验与生产功能验证分开报告。

本轮图件验证：生成器 XML、markers、collisions、geometry、composition 五项通过；Playwright/Edge PNG 导出通过，文字溢出、画布文字越界、文字互撞、文字/节点、连线/文字、连线/节点碰撞均为 0。PNG 尺寸为 3280 × 13080，已回读整图；缩放预览不能替代逐节点浏览器测量。完整输出见 [SVG 校验日志](../../current-structure-render.log)、[PNG 校验日志](../../current-structure-png.log) 与 [浏览器报告](render-report.json)。本机既有 Node 依赖中未找到 Mermaid 校验器，正文 Mermaid 未执行解析器验证；没有为此安装新依赖。

## 历史阶段记录

以下原文按历史时点保留。“当前”“本轮”“最新”均指状态/表情阶段，而非上述六线接入后的结构；旧的 Python/run/GUI 未迁移描述以及 1001/1121/1142 等结果不能当作现在的路由或全量验收。

2026-09-07 核对 17 项 MCP 源码接线，并补齐 run status 与表情实际生产链。沿用四列布局、Python 图源和 Edge 渲染脚本；Python 仅用于文档图生成，不代表产品 runtime 依赖。已接入不等于旧 17 项全部语义迁移、真实模型质量、已安装版本或发布验收完成。

| 图 | SVG | PNG | 内容 |
| --- | --- | --- | --- |
| 当前运行总图 | [SVG](runtime-current.svg) | [PNG](runtime-current.png) | 固定账号、有界 IPC、17 工具、真实 image_metadata、run status 只读统计、表情 prepare/offline 到图片发布完整链 |
| SNS 缓存发布 | [SVG](sns-cache-publish.svg) | [PNG](sns-cache-publish.png) | 独立的显式 SNS 缓存、启发式媒体恢复、每联系人目录发布 |
| ASR / WASM 接线 | [SVG](asr-wasm-wiring.svg) | [PNG](asr-wasm-wiring.png) | 数据库严格关联、显式成功缓存、离线 WASM、已接线与未验收边界 |
| MCP 语音数据流 | [SVG](mcp-voice-flow.svg) | [PNG](mcp-voice-flow.png) | daemon 仅准备、内部 24 MiB、独立 MCP 预算、主机 WAV 发布与显式 ASR/cache |

早期 `current-runtime.svg`、PNG、JSON 和 layout 文件仅为历史快照，不是以上四图或当前权威架构。本轮只更新文档、图源与生成物，不修改生产 Rust。

此前 14/15 工具阶段、checked-cache 909 次及 receipt/image_metadata 962 次通过均为历史快照。当前 `source-evidence.json` 状态为 `status_emoticons_1001_snapshot_not_full_legacy_parity`；统一 all-targets 日志 `C:/CodexLocal/wx-cli-status-emoticons-final-tests.log` 共 1127 行、16 组，1001 次通过、0 失败、10 忽略，main 确认退出码 0。MSVC 检查与指定代码 `git diff --check` 退出码 0，日志 `C:/CodexLocal/wx-cli-status-emoticons-final-check.log`；既有 2 条 ASR 未使用代码警告不变。

`toolkit_run_prepare` 8 项单元测试与表情 runtime 6 项通过；第 6 项以测试进程名模拟 `config.wechat_process`，执行真实 run emoticons 并复用 saved keys，未扫描真实进程内存。额外显式 FFmpeg 测试 1 通过、0 失败，日志 `C:/CodexLocal/wx-cli-emoticons-ffmpeg-test.log`，单列不合并；常规基线仍为 1001/0/10，不写成 1002/0/9。合计是执行次数，不是唯一用例数。

最新全量日志含语音真实进程 7 项及图片真实进程 2 项通过；语音测试覆盖源目录移走、daemon 停止、原/重启 MCP 均相同文本、缓存不变且后端一次。main 回报生产图片夹具 75 项与冷查询/缓存刷新回归通过。909 全量通过、此前 ASR/协议/缓存夹具计数均仅为先前源码快照；图片目录替换 20 passed / 2 ignored 已修复。日志及警告分类见 [MCP 契约](../../../../src/mcp/PROTOCOL.md)，不把图形校验当作功能验收。

## 调用与边界

测试时点边界：1001/0/10、2 警告及上述 check 结果对应已收口的状态／表情阶段。随后 `toolkit_run_prepare::prepare_emoticons` 更名为共用 `prepare`，main 确认表情行为不变，本轮仅同步源码锚点。最新源码哈希可能包含刚开始、尚未测试的 run decrypt 工作；不得将旧阶段测试视为新代码已验收，也不在本轮图中新增 decrypt 迁移链。

补索引条件：无源弱缓存不能自动补造身份；有源强身份成功缓存可在真实 identity/evidence 核验通过后补 receipt 索引，不必重新识别。独立宿主日志快照为 13 lib + 19 audit passed、1 ignored，含 4 lib / 3 test warnings；当前主线全量以 1001 次通过快照为准。

运行总图第 03 行独立展示图片元数据链：内部 image_metadata 请求、全局分页后全分片身份 COUNT、共享 ResourceReader 精确行事务、每页一次 scan_candidates 与 Scan / Pin、资源 MD5 / 加密 DAT metadata.len / 状态白名单。普通 CLI 保持轻量；不读 DAT 正文、不解码或上传。

第 10 行展示 `CLI → run_status::inspect → 配置相对路径 / 元数据 / messages、chats truthiness → 文本或 JSON / warning`；不读密钥内容、不改配置或转录、不建缺失目录，旧 `toolkit status` 环境报告不变。

第 11–12 行用跨行连线展示 `CLI → prepare（进程检查 / saved keys 或新扫描验证后原子保存）或 offline load_saved → 账号锁 / DbCache / 隔离 cache/emoticons → catalog → filter / dry-run preview 或 HostOutputGuard → 缓存 / 受限 HTTP / AES / 可选 ffmpeg HEVC → 图片或 bin`。已有 keys 不做全库 HMAC，危险路径拒绝；预览不下载或创建导出目录，图片不覆盖、bin 可替换，逐项失败保留批次 exit 0。红色 run 节点仅指未迁移的其他 run 流程。

绿色表示已注册且存在调用；橙色为未注册或待接线；红色为保留的 Python/Node 兼容流程；蓝色表示能力限制或待验收。并列节点没有箭头时，不表示相互调用。

MCP 当前完整清单：`get_recent_sessions`、`get_contacts`、`get_chat_history`、`search_messages`、`decode_transfer`、`decode_location`、`get_new_messages`、`get_chat_images`、`get_contact_tags`、`get_tag_members`、`decode_refer`、`get_voice_messages`、`decode_file_message`、`decode_record_item`、`decode_image`、`decode_voice`、`transcribe_voice`。图片已有资源 md5、加密 DAT size 与状态投影；receipt 允许精确身份的脱源只读命中，但不认证源现存，显示名可能需 ResolveChat。新全量、真实输出及模型质量待验。轮询是会话摘要；同步 stdio 不支持在途取消。

| 能力 | 核验的源码关系与限制 |
| --- | --- |
| MCP | `main → cli/mcp → mcp/protocol → transport::send_with_limits → daemon/server`；首次查询固定显式配置与账号，持有配置读锁；业务 IPC 有剩余时限及响应字节上限，后台启动保留独立时限 |
| 标签与成员 | `server → query/mcp_contacts → toolkit/contact_metadata`；标签输出计数投影，成员选择精确名称优先且拒绝歧义 |
| 引用与附件 | `mcp_refer` 和 `mcp_attachments` 共用 `strict_message::locate`；完整分片清单前后核验，同表/跨分片唯一性先于业务类型检查；完整逻辑 source 随结果保留 |
| 附件引用 | `mcp_attachments → attachment_refs::parse_file_message/parse_record_item → find_reference`；根为当前 `db_dir.parent()`；存储正文上限 1 MiB，文件解码 20,000 字节、记录 500,000 字节；直接原 XML 零基索引，不用展示摘要 |
| 引用结果 | `metadata + status + reference`；found/missing/text/metadata_only；受限扫描、常规只读句柄、实际大小/MD5、无 hash 弱绑定警告；不解码、下载、上传或写附件。序列化路径不是永久锁或账号认证凭证 |
| 图片写出 | `cli/mcp::prepare_request → DecodeImage → server → mcp_image::q_decode_image_with_key_file → native_image::export_image_with_guard`；原守卫保持到发布前复核，输出根必须预存，密钥仅宿主给出；工具 schema 拒绝路径、密钥、覆盖和上传参数 |
| 图片发布边界 | 当前账号严格消息与资源定位、受限 DAT 候选、同目录临时文件和 `persist_noclobber`；不覆盖、不下载、不上传；V2 缺有效显式 AES 密钥失败，不扫描密钥。关联标记仍是启发式证据 |
| 图片响应边界 | 发布后 IPC/MCP 仍可因超限、超时或传输失败而失败，已发布文件不回滚；错误响应不是无副作用证明。不承诺自动重试、幂等成功或恰好一次，重试前核对输出 |
| 语音列表 | `server → query/mcp_voice`，完整媒体分片分页与真实 `length(voice_data)`/NULL；不是解码或 ASR 路由 |
| 语音准备与执行 | `try_cached → receipt::lookup_success` 在语音 IPC 前只读核验精确身份；命中直接返回，未命中才 `server → mcp_audio::q_prepare_voice` 返回 SILK/证据，内部 IPC 24 MiB；host 复核后解码或显式 ASR，公开 MCP 独立预算 |
| WAV 发布 | host 的共享守卫与发布器；before_commit 中 `check_text_result` 使用真实请求 ID 与 JSON 包装，复核后禁止覆盖发布。后续通道断开仍不回滚 |
| MCP 后端与缓存 | 默认本地，请求私有 TempDir；云端须宿主明确授权，无自动回退。IPC 后 remaining 收紧实际执行期限；成功缓存 timeout 不参与 identity，旧摘要保留但不会立即命中 |
| 缓存提交前检查 | host 通过 `transcribe_cached_checked` 预检完整文本，`store_success_checked` 暂存、sync、快照复核后再次回调；预算、原始守卫、context、账号拒绝不发布本次缓存。普通 I/O 失败不抹去成功识别，提交后通道失败不回滚 |
| Delta | `ExportDeltaNative → cli/export_delta → Request::ExportDelta → query/export_delta → chat_delta`；原始正文和逻辑分片进入旧 UID；默认新 root，`--append-run` 仅复用 root/deltas，独占新 run；同名空 run 也拒绝，原完整输出和旧 run 不改，不是恢复或全量合并 |
| 计划 CSV | `ChatPlanNative → collect_plan/collect_plan_with_scan → render_plan_csv`；12 列、BOM/CRLF、整数/空值和缺库状态；解密根祖先链在 canonicalize 前检查；只是计划，不执行导出 |
| 数据库 ASR | `TranscribeDatabaseNative → asr_database → database_media::resolve_voice`；显式静态根、完整 source/local_id，Name2Id、svr_id 和时间唯一关联；SILK 字节复用公共 ASR，不写临时 SILK，本地后端仍可用受控临时 WAV |
| ASR 缓存 | `--cache-file` 与 `--cache-account` 成对启用 `cached::transcribe_cached`；先验证授权、SILK 与后端身份，再读取缓存；键包含消息来源、音频摘要及后端配置，时间也参与身份。命中成功空文本有效；未命中才调用字节转录；仅成功结果持久化 |
| 缓存失败 | `cache::store_success` 使用协作锁与原子发布；不覆盖同键未知记录，拒绝外账号文件；读写不可用单独报告，不把成功转录改成失败。协作锁不是针对非协作写者的 CAS；显式账号标签与静态快照仍非账号认证 |
| SNS | `export_sns → build_cache_index/export_database_with_cache → recover_post_media/apply_media_references → write_export_with_cache`；同父目录 staging 后 rename，每联系人发布且拒绝已有目录；并非全批次事务，媒体归属仍为启发式 |
| 视频 | `sns_video → VideoRuntime::bundled/new`；128 KiB 前缀解码、尾部流式复制、sync_all/persist_noclobber；仅 ftyp 检查不是完整 MP4 验证；不启动 Node |
| 历史选页 | `cli/history` 与 `mcp/protocol` 将多类型/oldest_first 传入真实 History IPC，经 server 到 q_history。多类型或最早页进入 `history_selection::Selection::query_shard/page`，复用 `read_history_row/render_history_rows`；每片取 limit+offset 候选，合并后统一分页。最早/最新选页最终均升序展示，不按 local_id 去重，不承诺同时间跨快照稳定游标；默认旧分支仍保留 |
| 已接线与验收 | `mcp_audio`、`mcp_voice` 宿主、`native_image`、`mcp_image` 均已接线；源码可达不推定真实账号或旧语义完全兼容 |

ASR 真实模型质量、全功能兼容与全仓回归由对应任务核验，图像渲染不证明这些能力。SNS 缓存与 ASR 转录缓存独立；MCP host 已调用 ASR，但未调用 Delta。图片发布与响应仍非事务，语音预算预检不代表通道断开可回滚。

## 复现与证据

```powershell
python docs\diagrams\render_architecture.py
node docs\diagrams\export_png.cjs C:\Users\leyan\.cache\codex-runtimes\codex-primary-runtime\dependencies\node\node_modules\playwright
python docs\diagrams\render_architecture.py --check-source
```

无需安装依赖。生成器使用 Fireworks Tech Graph 的 xml、markers、collisions、geometry、composition 五项校验；`--skill-root` 可覆盖默认技能路径。PNG 使用 Edge 中文字体，宽 3280px，逻辑宽 1640px；浏览器测量文字溢出、画布越界、文字互撞及文字/连线/节点碰撞，失败非零退出。交付前逐张回读 PNG。

`source-evidence.json` 保存时间、源码与图源 SHA-256、带行号的接线锚点及精确 17 工具清单。锚点是人工审阅后的变更告警，不是 Rust 调用图解析器或编译证明；本轮新增语音预算、执行和缓存身份锚点。源码漂移先重新审阅再生成。生成期间变化会失败；本轮新增关系不使用 `--preserve-evidence`。

早期 voice17/audit 日志保留作历史材料。本轮图形输出见 `C:/CodexLocal/wx-cli-status-emoticons-diagram-render.log`、`C:/CodexLocal/wx-cli-status-emoticons-diagram-png.log`、`C:/CodexLocal/wx-cli-status-emoticons-diagram-source-check.log` 与 [render-report.json](render-report.json)；状态以实际结果为准。本轮图生成不访问真实账号、音频、密钥或网络。
