# 旧工作流原生迁移缺口审计

## 当前状态表（2026-09-07 文档同步）

本文下半部保留早期缺口审计，不能直接当作当前未完成清单。以下表格对照当前源码说明替代入口、必要兼容路径和未验证边界；它不把旧失败、未接线或未执行记录改写为“当时已成功”。最新总账、日志及架构见 [Rust 迁移记录](rust-migration.md)、[系统架构](architecture.md)、[MCP 契约](../src/mcp/PROTOCOL.md)。

精简后自动化快照为 **1325 passed / 0 failed / 11 ignored**，个人 Web **61/0**；另有 **8 次显式可选测试执行**通过，默认 ignored 数不改写。统计包含共享套件重复执行，不是独立功能数。UI 与额外可选测试属于上一轮证据，本次文档同步未重跑。

文档同步期间集中重跑验证：`C:/CodexLocal/wx-cli-doc-sync-tests.log` 退出 0，20 套件仍为 `1325/0/11`；check 退出 0、9 类警告，18 项 EXE help 检查通过。未运行真实账号、模型或云服务；help 检查不替代业务或部署验收。

| 原条目 | 当前源码与公开入口 | 兼容或验证边界 |
| --- | --- | --- |
| G01 SNS 时间线/相册 | [sns_timeline](../src/cli/sns_timeline.rs) 接 `wx toolkit export-sns`；[sns_album](../src/cli/sns_album.rs) 接 `wx sns-album`，均使用 Rust 编排、缓存、受控媒体处理和发布 | 时间线与相册的复用/更新规则不同；旧目录认领、来源绑定和网络策略保留。只处理本地已知帖子，不证明未缓存的全部在线历史 |
| G02 Node 密钥流桥 | `decode-sns-video` 与相册使用 [video_runtime](../src/toolkit/sns/video_runtime.rs) 的 wasmi 和内嵌 WASM | 不启动 Node；WASM 仍是生产编译输入。25 MiB 图片密钥流与 128 KiB 视频前缀不同，MP4 头不证明可播放；见 [运行时说明](../src/toolkit/sns/VIDEO_RUNTIME.md) |
| G03 Web/GUI/监控 | [web_native](../src/cli/web_native.rs) 已接 `toolkit web/gui`；[web/server](../src/toolkit/web/server.rs) 提供本地 HTTP、任务/SSE/取消及查询，个人 UI 已有 61/0 合成验收 | GUI 是浏览器界面，不是重写 tkinter 窗口；默认仅 127.0.0.1，`--port 0` 分配空闲端口，`gui` 自动打开浏览器。真实账号与部署仍未据此验收 |
| G04 导出及自动 ASR | [export_all](../src/cli/export_all.rs) 接 `toolkit export-chats ... -t -- ...` 和 `toolkit run export-all -- ...`；[asr_batch](../src/cli/asr_batch.rs) 接 `toolkit transcribe-chat` 并自动关联数据库语音 | 账号、来源、缓存与批次编排已在 Rust；旧 `local` 配置（缺省亦为 local）仍使用受控 Python Whisper/PyTorch 推理桥，需模型权重；不把它写成纯 Rust 推理或保证离线 |
| G05 计划与参数组合 | 同一 `export_all::Args` 已含 `--write-plan-csv`、`--from-plan-csv`、`--plan-mode`、`--size-mode`、`--incremental`、`--delta-only`、日期、users、dry-run 和转录参数 | 写计划与读计划互斥，delta-only 要求 start；失败不伪装成功。保留独立原生命令，不要求用户手工串联才获得组合能力 |
| G06 启动与配置 | [launcher](../src/cli/launcher.rs) 的 `wx-toolbox` 白名单别名、[toolkit::cmd_run](../src/cli/toolkit.rs) 和 [setup_native](../src/cli/setup_native.rs) 已原生接线 | 无配置首启在 TTY 运行向导，仅 Applied 后继续 GUI；非 TTY 明确失败。setup 默认预览，写入需 `--apply`，非交互还需 `--yes`；`all` 不默认开启 ASR |
| G07 个人目录导出 | [export_messages](../src/cli/export_messages.rs) 接 `toolkit export-messages`，支持 CSV/HTML/JSON、`.info` 及受控媒体目录，目录依据实际 `Msg_*` 表而非 SessionTable | 未映射身份保留 unknown 标记。HTML 图片有单图 16 MiB/每页 URI 64 MiB 预算，超限回退；音视频及下载引用仍可能依赖目录文件，不是全媒体单文件 |
| G08 表情导出 | [export_emoticons](../src/cli/export_emoticons.rs) 接 `toolkit export-emoticons [output] --filter ...` 及 `run emoticons`；目录查询、下载和格式处理已原生 | `--dry-run` 不下载、不创建导出目录，但可使用账号解密缓存；未指定 dry-run 的导出命令会下载，并无额外 `--allow-network` 参数。Web rich 只输出安全元数据/链接，不自动请求表情 CDN |
| G09 企业微信 | **排除本轮目标与完成条件**；既有实现、入口和历史材料保留 | 不继续在本文审计/扩展企微；不得因排除目标删除保留实现，也不得把旧 G09 缺口重新计入个人微信未完成清单 |
| G10 独立 SNS 缓存归档 | [sns_archive](../src/cli/sns_archive.rs) → [sns/archive](../src/toolkit/sns/archive.rs)，公开 `toolkit decrypt-sns` | 不要求每个缓存文件都有时间线帖子；仍受账号、目录、格式、密钥和输出边界限制，不证明所有真实缓存均可恢复 |
| G11 诊断/清理/分发 | [monitor_native](../src/cli/monitor_native.rs) 接 `toolkit monitor/latency`；[cleanup_native](../src/cli/cleanup_native.rs) 已接清理；主仓提供 wx/wx-toolbox 两个原生入口 | 真实时延质量、已安装 EXE/发布包验收不能由合成回归推导；cleanup 的路径/授权边界不能省略。npm 启动包装仍需 Node，直接 exe 不需 Node |
| G12 MCP/媒体/查询 | 当前 17 工具，图片、语音解码及转录已接；联系人/历史选择和媒体来源链路已在原生协议/IPC 中实现，见 [附件契约当前表](native-attachment-contract.md) | 宿主未授权时仍拒绝媒体操作；图片、只读附件引用、语音推理是不同链路。History/NewMessages 的 rich 与自动图片已接，不再列为未注册；真实账号/模型/GPU/云端仍未验证 |

当前 `toolkit run` 未知命令会报错，不再调度旧 Python 业务脚本；旧 `run_main/run_script` 已删除。[legacy.rs](../src/toolkit/legacy.rs) 仍保留 ASR 解释器/诊断路径发现，`toolkit status` 在解释器为裸命令名时仍可能执行 `python --version`。它不同于原生只读业务统计的 `toolkit run status`，不能把整个 legacy 模块称为已删除或完全不运行 Python。

必要依赖分别记录：可选 Python 推理桥、npm 安装/启动包装、Frida Hook JS、浏览器 JS、内嵌 WASM、ffmpeg/whisper.cpp 和测试 oracle 的职责不同。精简已删除 SNS 四个无生产调用包装并把测试转向同参数核心；保留 `with_media/with_publication`、参考源码和模型不是“迁移未完成”的自动证据。

当前自动化不证明真实账号历史覆盖、任意媒体可播放、真实模型/GPU 识别质量、真实云服务或部署环境已经验收。这些未验证边界与“源码中缺少功能”必须分开；下列历史判断不能覆盖上表，也不能用总测试数替代逐项证据。

## 历史审计正文

初稿审计日期：2026-09-07 的早期迁移阶段。对象为当时工作树，不是已安装的 wx.exe，也不是发布版本。以下到文末的“当前”“未见”“尚未完成”“待推进”和建议顺序均指对应历史阶段，保留旧注册状态及风险要求供追溯；现在的接线与验证以上表和最新总账为准。历史 G09 及旧目标表中的企微要求已排除，不再适用。

初稿为只读审计，未执行旧 GUI、真实账号取钥/导出或网络下载。后续原生接线与合成测试进展按条目更新，最终回归记录见 `rust-migration.md`；仍未据此证明真实账号覆盖率。旧模块目录存在、独立测试通过、总测试数增加均不等于旧用户工作流已经移植。

## 历史结论

整体目标尚未完成，关键不是剩余几个 MCP 名称，而是以下真实使用链仍停留在 Python：

1. SNS 时间线更新、网络媒体与相册编排已接 Rust，验收进展见 G01；独立缓存归档仍见 G10。
2. 完整 Web/tkinter 工具箱、连续监控、任务选择/进度/取消和双模式 EXE。
3. 从聊天数据库自动批量导出并转录、旧计划 CSV 消费和一键参数组合。
4. 个人聊天含本地图片的 CSV/HTML 目录导出、表情包 CDN 导出。
5. 企业微信自动取钥到多会话多格式导出的完整编排。

Rust 已有大量可复用核心，不应重写这些核心来制造新的平行实现。本文将缺口分为：**运行时仍依赖**、**核心已实现但流程未接齐**、**原生入口未见**、**兼容变化需明确**、**验收证据不足**。安全收紧不应机械还原成旧的不安全行为。

## 历史公开边界

| 公开入口 | 当前实际调用链 | 可证明的边界 |
| --- | --- | --- |
| `wx toolkit run <command> -- ...` | `src/cli/toolkit.rs::cmd_run`：仅 decode-images 转原生；其余 `run_main` → `src/toolkit/legacy.rs::run_script` → Python main.py | decrypt/export/all/status/web 等仍是 Python 工作流；不能因 `toolkit decrypt` 是 Rust 而宣称 run decrypt 也已替换 |
| `wx toolkit export-chats ... -t -- ...` | toolkit.rs → export_all_chats.py | 包括透传计划/增量/日期/转录参数；运行依赖 Python |
| `wx toolkit export-sns` | toolkit.rs → cli/sns_timeline.rs → export_database_with_publication | 选中账号、缓存及受控下载、时间线更新已原生；生产媒体保持平铺布局，旧目录须显式认领 |
| `wx sns-album` | cli/sns_album.rs → 固定账号 Feed → toolkit/sns/album.rs → images/videos/render/publish | 原生相册编排；绑定目录可更新，显式认领旧目录，不启动 Python/Node |
| `wx toolkit transcribe-chat` | toolkit.rs → transcribe_chat.py | 配置解析、自动媒体查询、批次转录仍经 Python |
| `wx toolkit web/gui` | run_main(web) / app_gui.py | 未替换 Web 服务或 tkinter UI |
| `export-sns-native`、`decode-sns-video` | `src/cli/export_sns.rs` → toolkit/sns；sns_video.rs → wasmi | 原生数据库/缓存恢复、显式下载和单视频解密；时间线默认 fresh-only，显式 --update 绑定快照路径，旧目录还需 --adopt-existing；相册使用独立入口 |
| `export-chats-native`、`chat-plan-native`、`export-delta-native` | `src/cli/export_chats.rs`、chat_plan.rs、export_delta.rs | 已接公开原生命令，但参数不是旧脚本全集，不自动串联三者 |
| `transcribe-audio-native`、`transcribe-chat-native`、`transcribe-database-native` | `src/cli/asr.rs`、asr_database.rs → toolkit/asr | 已接字节/文件、显式清单、单条数据库身份路径；后者已有可选缓存。不等于完整导出自动 ASR |
| `wx mcp` | cli/mcp.rs → mcp/protocol.rs → IPC/server/query | 本次读取仍为 14 工具；只读工具缺口不是全部迁移范围 |

`legacy.rs` 接受 `WX_WECHAT_DECRYPT_DIR`、`WX_WECHAT_DECRYPT_PYTHON` 并定位脚本/解释器。它不是不可达的备份代码，上述实际 match 分支仍调用它。不能提前删除 vendor。

## 历史待推进工作流

### G01 网络 SNS 与完整相册

**证据。** `vendor/wechat-decrypt/export_sns.py:456` 的 `_try_download_media` 使用 urllib；`export_sns_timeline:770` 读取 `WECHAT_SNS_DOWNLOAD_MEDIA`，默认 0，显式 1 才下载。`export_sns_album.py:517` 的 export_album 组合已有图片/视频复用、缓存匹配、远端下载和 HTML；`download_sns_image:372`、`download_decrypt_video:245` 处理媒体，`sns_image_url_candidates:349` 处理 URL/token 候选。该脚本 `parse_args:701` 有 image-workers、video-workers、output-dir/output-root、since/until、no-remote/no-videos。

**当前差异。** `src/cli/sns_album.rs` 已直接调用 Rust 相册编排，并暴露 worker、output-dir/output-root、日期和禁用网络/视频参数。图片按已有文件、原图、缩略图顺序；视频按已有文件、完整缓存、远端、部分缓存顺序。固定账号来源绑定防止混用，旧无绑定目录需显式认领并持续标为来源未核验；发布保留未知旧文件，但只保证逐文件替换，不是整树事务。`toolkit export-sns` 已原生更新，显式快照入口也支持 --update。时间线按本轮结果重写汇总并保留无关旧文件，不合并历史帖子、不复用本轮未成功恢复的历史同名媒体；此契约与相册不同。相册图片密钥流上限 25 MiB，视频前缀 128 KiB。`q_sns_feed` 仍只查询本账号本地数据库，scan/limit 上限 50000；返回精确作者、扫描数和截断标记，相册摘要传播截断警告，不代表完整在线历史。

**分类。** 相册与时间线公开调用链均已原生。相册真实 CLI 12 项通过，时间线另有 7 项 CLI 测试及旧 Python AST oracle 对照核心测试。时间线 CLI 覆盖选中配置、重跑、旧目录认领、快照身份、晚联系人冲突、环境授权覆盖、本轮缓存和 loopback 下载；不是借用相册测试。当前全目标基线为 20 个入口、1142 次通过、0 失败、11 忽略，含共享测试重复执行；完整日志和页面验收状态见 rust-migration.md 当前清单。真实账号历史覆盖率、任意视频可播放性不在合成验收结论内。没有证据表明旧脚本能抓取从未缓存的全部朋友圈历史。

**验收。** 合成 HTTP 服务验证原图/缩略图候选、token/密钥处理、明文与加密媒体、无远端/无视频选项、已有文件复用、失败计数及 HTML/JSON 引用。先把网络授权、重定向、大小与输出隔离设为明确边界，再复用现有 Rust 核心。独立视频 golden 不足以通过此项。

### G02 相册 Node 桥已替换，旧兼容资产仍保留

`export_sns_album.py::WasmKeystream` 在 131 行检测 node，138 行启动 `node ... --stdio`；runner 是 vendor/sns_media_wasm/weflow_wasm_keystream.js。`src/toolkit/sns/video_runtime.rs` 已用 wasmi 并 `include_bytes!` 嵌入同目录 wasm_video_decode.wasm，`toolkit decode-sns-video` 已接线。

`wx sns-album` 已通过 `album_videos.rs` 使用原生 wasmi，图片解密也复用此核心，不再启动上述 Node 桥。视频模块和真实 CLI 均已用合成加密流核对前缀解密与尾部字节，CLI 同时验证最终相册 JSON/HTML/摘要；仍不替代真实账号加密 CDN 与播放器验收。保留 WASM 文件，因为它是生产编译输入；旧脚本及桥接资产待其他兼容调用链审计后再清理。MP4 头校验不能证明可播放性。

### G03 完整 Web/tkinter GUI 与连续监控

`monitor_web.py` 的 TOOL_TASKS（2377 附近）包含个人微信取钥解密、图片钥、全量导出、DAT、SNS、企微取钥解密/导出、MP3。`_run_tool_task:2503` 顺序运行子进程并推送 SSE；`do_POST:2720` 实现 `/api/tool`、`/api/tool/cancel`；另有 `/api/history`、`/api/tags`、`/api/sessions`。`monitor_thread:1436` 提供连续消息监听。`app_gui.py::_exec_combined` 组合消息、SNS、MP3，并管理选择/进度；GUI 的 voice 选项是 MP3 转换，不应误写为自动 ASR。

当前 cli/toolkit.rs 的 Web/Gui 仍执行这些 Python；公开 Commands/ToolkitCommands 未见 Rust Web 服务、桌面 GUI 或等价持续监控入口。Rust daemon、unread、history、MCP get_new_messages 是查询或会话摘要轮询，不替代 SSE/界面任务生命周期。

**验收缺口。** 需原生后端任务编排加实际 GUI 的启动、选择联系人、最近活跃筛选、任务日志、取消及子进程回收、失败状态、退出重入、账号隔离测试。旧 Web 绑定 0.0.0.0（monitor_web.py:2901）；迁移不应为“兼容”默认复制无授权暴露。本轮未做 UI 验收。旧 Web 的个人聊天 formats 被 `_build_export_steps:2353` 明确忽略，不能宣称其按钮已支持所有格式。

### G04 批量导出与自动转录尚未组合

`export_all_chats.py` 的 `--with-transcriptions` 在主循环传入单聊/增量导出；`transcribe_chat.py::transcribe_export` 使用 username（缺失才解析 chat），自动 `_fetch_voice_row`，对 type=voice 且 transcription 为假值的消息逐条转录、每条后写 JSON。旧本地 backend 在 mcp_server.py:3884 加载 Python whisper，并可能首次下载模型；旧 OpenAI/whisper.cpp 配置由 `_resolve_active_backend:3842` 共享。

Rust export-chats-native 的参数没有转录选项；transcribe-chat-native 强制 media-manifest/media-root，transcribe-database-native 只接单条精确消息身份。已有 ASR/cache/database_media 不能证明“自动导出全部聊天并关联每条语音”已实现。`BackendKind::Local` 实际为显式 whisper.cpp 可执行文件和模型，不是旧 Python Whisper/PyTorch 后端；没有自动模型下载或旧环境配置发现。

**兼容边界。** 新授权前置、分片/server_id 身份、更严格缓存键应保留，不能恢复旧 local_id 首条猜测。需明确旧逐条保存与新批末原子回写之间的崩溃恢复策略；验证失败标记、空成功重跑规则及断点续跑。缓存命中测试不是识别质量测试，尚无本轮真实模型/真实云端验收证据。

### G05 计划 CSV、增量与旧参数组合

旧 export_all_chats.py 主参数有 `--write-plan-csv`、`--from-plan-csv`、`--plan-mode blacklist|whitelist`、`--size-mode`、`--delta-only`、`--incremental`、日期、users、dry-run、-t；计划选择在 1615 附近改变 sessions。Rust chat-plan-native 负责显式离线统计/生成 CSV，需 --decrypted-dir 和 --user/--chats-json；没有旧 from-plan-csv/plan-mode 消费入口。export-chats-native 只接 users、incremental、start/end、dry-run；export-delta-native 是另一公开入口。

**分类。** 原生统计、CSV、delta 和增量核心已接；旧计划选择到批量导出（含转录）的组合流程仍依赖 Python。`tests/chat_plan_runtime.rs` 验证独立真实命令，`tests/delta_runtime.rs` 验证合成加密账号/IPC/delta/append，不覆盖读取用户编辑 CSV 后驱动全流程。应补黑/白名单、空 CSV/重复身份/无交集、users 组合、日期与 delta/-i/-t 组合及失败 manifest 的实际 CLI 测试，不能用手动运行三条命令冒充参数兼容。

### G06 一键启动器、配置和参数语义

`main.py:276` 默认 web；decrypt/export/all 会检查微信进程并 ensure_keys。export/all 先全量解密，export 透传后续参数、all 则传空数组；`except SystemExit: pass` 会吞掉导出退出异常。all 结尾仅提示转录配置，**不自动执行 ASR**。`wechat_decrypt_launcher.py` 还支持无参启动 Web、export-all 别名及白名单 script.py 调用形态。`toolkit run` 只有 decode-images 使用 NativeInvocation，其他仍 run_main。

`wx init` 已有原生个人微信取钥/账号选择；`toolkit decrypt` 使用保存密钥，不能当作 main.py decrypt 的完整替代。缺少统一原生一键编排及这些参数/返回码的明确映射。安全退出码不应复制旧吞错误，应记录为有意修正并测试部分失败。setup.py 的交互式配置、后端检测与 --check 不等同于只提供 wx init/toolkit status；仍未见完整原生配置向导。

### G07 个人聊天 CSV/HTML 及媒体目录导出

`export_messages.py` 不是 export_all_chats 的别名：620 后按联系人和数据库组织消息，写 `.info`，按配置解密联系人图片，建立 local_id → 相对媒体路径，再输出 CSV（BOM、server_id、图片路径）、JSON、含图 HTML。格式来自 `WECHAT_EXPORT_FORMATS`；GUI 子任务可走这条旧路径。

Rust `wx export` 只列 markdown/txt/json/yaml 且默认 limit=500；export-chat / export-chats-native 是另一种完整 JSON。独立 `decode-images` 或 `extract` 的存在不代表媒体相对引用、.info 和 HTML 目录编排完成。当前未见等价个人聊天 CSV/HTML+图片目录公开原生入口。需同一合成账号产物级 oracle，覆盖同名联系人、分片同 local_id、文件名与资源引用，不能仅比较消息文本。

### G08 表情包 CDN 导出与在线展示

`export_emoticons.py::main` 支持输出目录、dry-run、filter，调用 `emoticons.py::build_emoji_lookup` 和 `download_emoji:208`；后者 urllib 下载并处理格式，包含 HEVC 转换辅助函数。`monitor_web.py::_download_emoji` 也使用该能力。公开可从 `toolkit run emoticons` 经 main.py 调入 Python；不能把消息中 emoji 摘要解析或本地 DAT 解码当 CDN 表情包导出。

公开 Rust CLI/toolkit 枚举未见等价 emoticons 下载器。应补只读列表/预览、显式网络下载、过滤、格式验证、重试/已存在处理和本地输出安全测试；目前网络表情包链未原生化。

### G09 企业微信完整工作流

`find_wxwork_keys.py` 是 WXWork 进程内存 raw key 提取，不等于个人微信 KeyProvider；GUI TOOL_TASKS 将它与 decrypt_wxwork_db.py 顺序执行。`export_wxwork_messages.py:630` 可批量选会话，按 WXWORK_EXPORT_FORMATS 或 --formats 同时写 CSV/HTML/JSON。Rust已接 `toolkit decrypt-enterprise`（显式 key-file、单库）和 `toolkit enterprise <snapshot>` contacts/conversations/messages/export（单会话、单格式）。

差距是 WXWork 自动取钥/多库解密和多会话多格式输出编排，不是 wxsqlite3 解密或 CSV/HTML writer 缺失。未见个人 init 扫描器调用 WXWork raw-key 提取。原生企业快照限制、WAL/日志拒绝须写入迁移契约，不能静默把活动库当完整快照。已有 toolkit/enterprise 的页级 golden 和查询测试只能证明离线子功能。

### G10 SNS 缓存批量相册与 standalone decrypt_sns

`decrypt_sns.py` 从两个配置缓存根批量处理图片、跳过缩略图，输出 `<output_base_dir>/朋友圈图片/<YYYY-MM>`；GUI sns_decrypt 执行该脚本后再 export_sns.py。原生 export-sns-native 可在读取 sns.db 时恢复已知动态的显式本地缓存，`toolkit decode-images/batch-decrypt-images` 也有解码能力，但公开入口未见完整旧缓存根到该目录布局的独立一键对应。

属于编排/布局兼容缺口；底层 DAT 与缓存匹配不必重写。须区分“数据库中有帖子的媒体恢复”和“缓存目录中全部可解码图像归档”，用只存在缓存、无时间线对应项的 fixture 验证，避免静默遗漏后者。

### G11 诊断、清理和分发

`monitor.py` 为终端持续会话监控；`latency_test.py` 观察解密/WAL 延迟，当前公开 CLI 未见等价连续监控/基准入口。不能用手动重复 sessions 或 daemon status 声称已覆盖。setup.py、config.py 管理旧路径和后端配置；两套 config 的发现/默认值不完全相同。

`cleanup.py:71` 枚举明文库、decoded_voices、decoded_images、exported_chats、exports 和 all_keys*.json，支持 status/交互/--dry-run。Rust daemon reload 等不是此清理流程；未见原生等价。迁移时必须增加账号/路径/输出所有权约束，不复制任意配置路径递归删除。

`build.bat` 调 PyInstaller + WeChatDecrypt.spec，launcher 无参启动 Web，冻结模式支持子脚本分发。Cargo release 产出 CLI 并不能替代这一双模式用户体验。当前旧 GUI 分发仍依赖 Python 打包链；只有重建完整 UI/启动器并验证分发后才能移除它。

### G12 尚未接线的新增核心与 MCP 兼容

本次末轮读取：`src/mcp/mod.rs` 只注册 protocol；protocol/CLI 测试仍拒绝 decode_image、decode_voice、transcribe_voice；`src/attachment/native_image.rs`、`src/daemon/query/mcp_audio.rs`、contact_rows.rs 已存在，公共 root 未见相应注册/调用。归类为正在实现/接线待验收，不归类为算法不存在。文件消息和记录条目已经进入 14 工具及真实命名管道 golden，不再列缺口。

联系人旧 nick_name/remark 搜索与范围已有 contact_rows helper；主计划 legacy_view 分流，但本次读取未见其 IPC/route 落地。原 CLI private/display 排序与旧 MCP 全范围应分别验收。History 的 CLI 已新增多类型/oldest_first；不能凭 CLI 参数证明 MCP 已映射，应对当前 protocol schema、IPC和真实查询一起核验。只读列表 get_chat_images 仍在协议返回 partial_legacy_compatibility 及 md5/size 不可用说明，需要完整媒体列表契约另验。

## 历史依赖分类

| 文件/依赖 | 真实用途 | 能否直接视为待删业务运行时 |
| --- | --- | --- |
| `src/scanner/windows/account_hook.js` | account.rs:311 用 Frida session.create_script 注入目标进程 | 必要的 Frida JS 执行接口；没有 Node 子进程。可审计脚本范围，但不能用 Rust 文件数量指标强删 |
| vendor/sns_media_wasm/*.js | 保留的旧 Python 相册脚本及 Node golden 使用 WASM bridge | `wx sns-album` 已解除 Node 生产依赖，见 G02；旧兼容资产仍需完整调用链审计后清理，wasm 本体是 Rust 编译输入，保留 |
| `npm/wx-cli/bin/wx.js`、install.js | npm 平台包装/安装；wx.js execFileSync 原生 exe | npm 分发路径依赖 Node，直接 wx.exe 不依赖。应单列分发选择，不误报为核心业务仍由 Node 实现 |
| tests/generate_*.py、migration_parity.py、fixture oracle.py | 从旧代码生成/验证 golden | 开发测试依赖，不是原生命令运行依赖；删掉会失去迁移证据 |
| docs/diagrams/render_architecture.py、export_png.cjs | 架构图生成/渲染 | 文档工具依赖，不是 GUI 后端；可后续简化但不优先于缺失业务 |
| ffmpeg、whisper.cpp | 原生 SILK 后续 MP3 编码、ASR 推理外部进程 | 不是 Python/Node。需继续验证进程/超时/模型部署，不以“纯 Rust”误报它们不存在 |

## 历史入口覆盖盘点

下表是本次人工检查的工作流归属，不是按脚本数计算完成率。测试、库 helper 与可执行入口不混为同一功能。

| 旧脚本/文件组 | 已见原生对应与剩余去向 |
| --- | --- |
| main.py、wechat_decrypt_launcher.py、setup.py、config.py、build.bat、WeChatDecrypt.spec | G06/G11；个人 init 与部分独立命令已原生，完整启动/配置/分发未替代 |
| app_gui.py、monitor_web.py、monitor.py、latency_test.py | G03/G11；持续 UI/诊断链仍 Python |
| find_all_keys.py、find_all_keys_windows.py、key_scan_common.py、key_utils.py、decrypt_db.py | 原生 scanner/decrypt/cache 已存在；一键参数与旧配置编排见 G06，不因同名 run decrypt 误判；跨平台 README 不作为已验证平台承诺 |
| find_image_key.py、find_image_key_monitor.py、decode_image.py、batch_decrypt_images.py | 原生解码/提取已有；单独旧图片钥监控的用户流程未在公开 CLI 枚举见对应，需与 init 当前图片钥路径另做运行验收，不用模块历史注释判定 stub |
| export_chat.py、export_all_chats.py、chat_export_helpers.py | 原生完整 JSON/增量/delta/计划组件已有；G04/G05 参数组合和自动转录仍依赖 |
| export_messages.py | G07 独立多格式+图像目录，不可合并计为 JSON 导出完成 |
| transcribe_chat.py、voice_to_mp3.py、wx_toolkit_voice_to_mp3.py | 原生文件/批量 MP3、显式 ASR已接；G04 自动数据库批次及旧 backend 语义未齐 |
| export_sns.py、export_sns_album.py、decrypt_sns.py、sns_media_wasm 文件组 | G01/G02 时间线与相册已原生；G10 独立缓存归档仍有缺口，旧脚本仍可能被 GUI/main 编排使用，WASM 为编译输入 |
| emoticons.py、export_emoticons.py | G08 在线表情包导出/展示缺口 |
| wxwork_crypto.py、find_wxwork_keys.py、decrypt_wxwork_db.py、export_wxwork_messages.py | G09；离线加密/查询/单格式已有，取钥与批量流程未齐 |
| mcp_server.py、decode_transfer.py | 14 工具及转账等已接；其他公开/兼容缺口见 G12，不把整个大文件列为全未实现 |
| cleanup.py | G11；不可提前运行或删除用户产物 |
| vendor README.md、USAGE.md、EXE_USAGE.md、docs/chat_export_format.md、tests | 用户工作流/格式证据和 oracle；需在新入口等价验收后更新/归档，不能以“旧”标记批量删除 |

## 历史目标约束

| 用户目标 | 当前证据与剩余工作 |
| --- | --- |
| Python/Node 尽可能 Rust 化，wechat-decrypt 全功能移植 | G01–G12 有实际入口差距，不能宣布完成；必要 Frida JS 与开发工具单列 |
| Rust 架构合理、模块清晰；原代码重构/注释 | 新 toolkit、query 叶模块已形成边界；仍有两套联系人/消息/SNS解析及 GUI脚本编排。全局合并需保持上述不同输出契约，不能凭文件数减少宣称优化 |
| 功能检验、回归且不遗失 | 已有离线/合成测试；还缺所列网络、GUI、完整一键工作流级验收。某次全仓通过也不覆盖不存在的测试 |
| 尽量中文注释 | 新模块多为中文；`src/attachment/mod.rs` 仍有历史“stub/后续接线”说明和模块级 allow，实际 resolver/decoder 已公开使用，说明注释审校尚未全局闭环。不能照旧注释给功能判缺失 |
| 详细架构图 | docs/diagrams 已有运行时、ASR/WASM、SNS图；证明有图，不证明覆盖 G01–G12 或与最终接线一致。所有批次结束需校验源图/布局/PNG与最终调用链 |
| 完成后清理不必要文件，再全块优化/合并 | 运行时 Python桥、WASM嵌入路径、AST oracle还引用 vendor，删除条件尚未满足。先形成逐入口替代证据，再做可达性、测试、构建和产物引用复核 |
| 尽可能多子代理并行 | 属执行要求，不是功能覆盖证据；并行模块在 root 接线/集成验收前只能标在途，不能把独立交付求和为完成 |

## 历史测试证据与推进顺序

本轮阅读的真实测试入口包括 tests/chat_plan_runtime.rs、delta_runtime.rs、voice_runtime.rs、mcp_runtime.rs、native_migration_security.rs，以及 toolkit 的 SNS/ASR/enterprise golden。它们证明已有对应测试代码，不代表本轮执行通过；新增在途 fixture 同理。此前本任务留存的 mcp-fourteen-protocol-test.log、mcp-fourteen-runtime-test.log、mcp-fourteen-cli-test.log、contact-rows-tests.log 是特定快照结果，不替代当前全仓验证。

建议下一批按用户链收口：先 G05/G04（已有核心，补计划选择→精确媒体关联→导出转录编排）；并行 G01/G02（复用离线缓存和 WASM，补受控网络相册）；再 G07/G08/G09；最后以这些原生任务服务重建 G03/G06/G11 的 GUI/启动器。每一链均以“真实公开入口 → 合成账号/服务 → 输出与错误 → 重跑/取消/隔离”的证据验收。此顺序不是缩小全部目标，G01–G12 未验收的项都继续保留。

禁止提前清理：仍被 run_script 调用的脚本、生产 include_bytes! 的 WASM、测试 AST/golden 原始来源、未迁移 GUI 的打包文件。旧脚本中吞异常、猜账号、任意路径删除等行为应明确修正，不以兼容为名恢复。
