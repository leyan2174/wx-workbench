# 原生附件迁移契约审计

## 当前状态与证据（2026-09-07）

本节按当前源码校订，优先于后面的“14 项 MCP”和第 1–8 节历史审计。旧未接线结论、风险分析及测试要求原样留作阶段证据，不能据此推定当前仍未实现，也不能把当时未执行的清单改写为全部通过。

当前 [工具注册](../src/mcp/protocol.rs) 为 **17 项**，`decode_image`、`decode_voice`、`transcribe_voice` 已注册并接入；宿主权限未配置时仍会拒绝调用，注册不等于默认授权。完整公开参数、错误投影和兼容差异见 [MCP 契约](../src/mcp/PROTOCOL.md)，整体状态见 [Rust 迁移记录](rust-migration.md) 和 [系统架构](architecture.md)。

| 链路 | 当前入口、参数及行为 | 保留的限制 |
| --- | --- | --- |
| 文件/记录引用 | `decode_file_message(chat_name, local_id, create_time=0)`、`decode_record_item(chat_name, local_id, item_index, create_time=0)` 经 [mcp_attachments](../src/daemon/query/mcp_attachments.rs) → `strict_message` → `attachment_refs` | `local_id > 0`，记录直接条目零基索引；存储 1 MiB，正文解码文件 20,000 字节/记录 500,000 字节，严格 UTF-8。只返回本地引用或 text/metadata_only/missing，不下载、解密、转码或写出附件 |
| 图片写出 | `decode_image(chat_name, local_id, create_time=0)` 经宿主策略 → IPC → [mcp_image](../src/daemon/query/mcp_image.rs) → [native_image](../src/attachment/native_image.rs) | 宿主必须配置已存在可信 `--media-output-root`，可选 `--image-key-file`；不接受工具请求提交输出路径或密钥。消息/资源唯一性及账号库存检查后无覆盖发布，不自动扫描密钥或下载 |
| 语音解码 | `decode_voice(chat_name, local_id)` 经 [mcp_audio](../src/daemon/query/mcp_audio.rs) 准备有来源的音频，由 [mcp_voice](../src/cli/mcp_voice.rs) 校验后调用 `audio::publish` 发布 WAV | 此 `local_id` 是旧 `VoiceInfo` 媒体记录 ID，不是 `Msg_*` 消息 ID；重复来源拒绝。宿主输出根必需，不覆盖，不上传；不是 `voice-to-mp3` 的 MP3 编码接口 |
| 语音转录 | `transcribe_voice(chat_name, local_id)` 使用同一严格媒体关联，推理和可选 `--voice-cache-file` 持久缓存由 MCP 宿主执行 | 工具请求不能选择引擎、凭据或授权。whisper.cpp 要求显式程序/模型；云端要求显式端点、模型、凭据及 `--allow-upload`；缓存不绕过授权和来源复核 |
| Python 推理兼容 | 宿主 `--configured-local-python` 默认关闭；开启后仅接受固定账号配置 `transcription_backend="local"` 与 `local_whisper_model`（缺省 `base`） | 与 whisper.cpp 路径和云参数互斥，不自动失败回退；需要 Python/Whisper/PyTorch 和模型，命名模型可能下载权重，不上传音频，不是纯 Rust 推理或保证全离线 |
| 展示元数据与 Web 图片 | History/NewMessages 已输出 [rich_message](../src/daemon/query/rich_message.rs) 的可选 `rich`；原生 Web 另经 [automatic_image](../src/toolkit/web/automatic_image.rs) 严格身份解码和展示 | `rich` 只从有界消息文本取结构化字段，不读 CDN、不解码媒体；坏内容省略/安全回退。自动图片、目录导出、MCP 图片是不同宿主链路，不能互相代替权限或来源验证 |

文件引用仍保留无 hash 时的启发式绑定警告；MD5 是内容匹配证据，不是账号认证或恶意碰撞防护。受控媒体入口不等于把旧 `q_extract` 的所有调用都改成相同契约；daemon 也可能写账号解密缓存，不能将“附件只读”扩展为整个进程零写盘。

主线当前证据为精简后 Rust **1325 passed / 0 failed / 11 ignored**、个人 Web **61/0**；另有 **8 次可选测试执行**通过，默认 ignored 仍单列，MSVC check 通过但保留 **9 条警告**。日志出处见迁移总账。包含跨套件重复执行，不是独立功能数；这也不是本轮文档任务重跑的结果。

文档同步期间集中重跑验证：`C:/CodexLocal/wx-cli-doc-sync-tests.log` 退出 0，20 套件仍为 `1325/0/11`；check 退出 0、9 类警告，18 项 EXE help 检查通过。未使用真实账号或私人附件，help 检查不替代真实附件或部署验收。

本轮只核对源码、入口与文档，没有执行 Cargo、网络、真实账号、私人媒体、模型/GPU 或真实云服务；合成回归不证明任意附件可播放、真实历史全覆盖或已安装发布包通过。企业微信排除目标，既有实现和入口保留，以下历史材料不能重新把企微列为完成条件。

## 历史阶段：14 项 MCP

以下保留 2026-09-07 早期制图时的更新，已被上方 17 工具状态覆盖；本节“当前”“仍未注册”等均只指当时。依据当时的 [系统架构 Mermaid](architecture.md)、[工具注册](../src/mcp/protocol.rs) 和 [server 分派](../src/daemon/server.rs)，`decode_file_message`、`decode_record_item` 已公开为只读工具；`decode_image`、`decode_voice`、`transcribe_voice` 仍未注册。`native_image`、`mcp_image`、`mcp_audio` 即使已有文件或独立测试，也不是当时公开链路。`history_selection` 在制图期间并行新增 query 调用点，当时未将其纳入图中公开链路，留待单独审阅。

真实链路是 `wx mcp → protocol → 固定账号 IPC → server → query/mcp_attachments → strict_message + toolkit/attachment_refs`。`mcp_refer` 共用严格定位；标签与语音列表走各自 query，不通过图片 Extract，也不调用 ASR。

| 边界 | 当前实现与证据 |
| --- | --- |
| 消息身份 | [strict_message](../src/daemon/query/strict_message.rs) 检查完整分片清单、唯一聊天名称及消息；保留 username/source/local_id/create_time，同表或跨分片重复拒绝。先定位唯一消息，再检查 base_type=49、XML 类型和条目索引，不取最新行 |
| 正文 | 存储字节上限 1 MiB；[mcp_attachments](../src/daemon/query/mcp_attachments.rs) 对文件解码限制 20,000 字节、记录 500,000 字节；严格 UTF-8、安全 XML。记录直接使用原始 recorditem 的直接 dataitem 零基索引，不受展示前 10/50 项裁剪影响 |
| 参数 | chat_name 与正整数 local_id；可选 create_time，0 表示不筛时间；记录要求非负 item_index。不接受任意 base、输出目录或外部 source 参数 |
| 根与查找 | 根只由当前 db.db_dir().parent() 推导；[attachment_refs](../src/toolkit/attachment_refs.rs) 扫描 msg/file 或当前聊天 hash 下 Rec，不跨账号兜底；有限目录/候选/字节扫描，常规只读句柄及祖先/reparse 路径检查 |
| 输出 | 成功为 `{exit_code:0, metadata, status, reference}`，状态 found/missing/text/metadata_only。reference 包含实际 path/size/md5、binding、warning、equivalent_copies；无 hash 单候选为 heuristic，多候选拒绝，MD5 不符失败 |
| 副作用 | 不复制、解密、转码、下载或上传附件；序列化期间保留只读引用句柄，但路径输出不是永久锁。daemon 可写账号解密缓存，不等于整个进程零写盘 |
| 安全错误 | 业务失败由 MCP 投影为 Query failed，后端失败为 Query backend unavailable，不透传内部 SQL/XML/密钥错误链；成功引用按契约返回本地路径，不能声称所有路径均隐藏 |

MD5 是内容匹配而非账号认证或恶意碰撞防护；Rec 路径不证明唯一卡片来源，无 hash 警告不能省略。注册与正常场景测试不等于完成全部安全审计；XML 标量嵌套/混合内容等对抗输入仍需专项验收，本次制图不宣称已修复。

### 该历史阶段的验收证据

[真实六工具 fixture](../tests/mcp_readonly_runtime.rs) 使用两个合成加密账号、真实 daemon/命名管道/stdin MCP，保留标签、成员、引用与语音列表覆盖，新增 [附件场景](../tests/fixtures/mcp-readonly-runtime/attachments.rs)：文件及 Rec 图片/语音/视频/文件原始字节与 MD5、同身份不同账号内容、A 不得找到仅 B 存在的文件、跨分片歧义先于类型与索引、错误类型/hash/索引、61 项且超过 20K 的记录尾项、源密文及媒体树前后不变。14 工具清单和三个未注册写工具拒绝一并验证。

上一交付定向结果为 1 passed / 0 failed，完整收发见 [交付日志](../tests/fixtures/mcp-readonly-runtime/test-14-delivery.log)。这是一个含多场景的集成测试，不是 14 项全部业务的全面验收，也不替代 main 的全仓回归。当前三张静态图及检查流程见 [图说明](diagrams/README.md)，源码摘要与接线行号见 [图源证据](diagrams/source-evidence.json)。本轮只更新图与文档，不重跑 Cargo 全仓或读取真实账号。

ASR `cache/cached` 已从显式数据库 CLI 接通：授权先于缓存读取、后端身份进入缓存键、仅缓存成功转录；这不代表 MCP transcribe_voice 已注册。Delta `--append-run` 只独占新批次，不改既有 run 或全量文件。两者与附件引用是独立链路。

## 历史快照边界

以下第 1–8 节保留早期 Python 行为、旧 Rust API 可复用性和迁移要求。其“没有公共入口”“后续实施”“本轮未运行”“未修改图”等均指早期审计当时，不是当前接线状态。“应”是要求，不是已通过的验收项；旧源码行号、模块可见性及图源证据也属于当时快照，当前入口以上方源码链接和最新迁移总账为准。

审计日期：2026-09-07。范围仅为本仓库当前源码，不修改生产实现、IPC、MCP 工具列表或架构图；不扫描真实账号缓存、不下载、不创建模型。文中的“应”表示迁移要求，不表示已实现。

已先读 [系统架构](architecture.md)、[Rust 迁移记录](rust-migration.md) 和 [MCP 契约](../src/mcp/PROTOCOL.md)。前两者描述模块职责及迁移进度；MCP 文档已列出三个附件工具的提案接口。本篇补充实际 SQL/XML、身份歧义、查找路径、副作用和可复用函数，不重复声称接口已接入。

## 1. 结论与范围

- `decode_image` 是“资源定位 + 解密 + 写出新文件”；`decode_file_message` 和 `decode_record_item` 是“解析消息 + 查找现有文件 + 返回引用”。后两者不复制附件、不解密 DAT、不转码、不下载。
- Rust 已有图片字节解码、附件枚举与 Extract，以及转账/位置详细解码。没有可直接等价替换这三个旧 MCP 工具的已注册公共入口。[MCP 缺口表](../src/mcp/PROTOCOL.md) 中的 `DecodeImage`、`DecodeFileMessage`、`DecodeRecordItem` 仍为提案。
- `attachment::resolver` 是图片资源定位实现，不是通用文件/合并记录 resolver；`message::export_content` 是阅读正文生成器，不是附件元数据定位器。
- 严格迁移不能直接复用“同 local_id 取最新”“时间不命中后回退”“按目录首次命中”的策略。兼容旧参数和中文返回，不等于保留静默选错附件的行为。

主要源码入口：[旧图片工具](../vendor/wechat-decrypt/mcp_server.py)、[旧文件工具](../vendor/wechat-decrypt/mcp_server.py)、[旧记录工具](../vendor/wechat-decrypt/mcp_server.py)、[ImageResolver](../vendor/wechat-decrypt/decode_image.py)、[Rust resolver](../src/attachment/resolver.rs)、[Rust decode](../src/daemon/query/decode.rs)、[正文导出](../src/message/export_content.rs)。初审行号已随重构变化，这里的链接只定位文件，不把历史行号当作当前源码锚点；当前注册与调用关系以上方状态表为准。

## 2. 参数与返回兼容

| 旧签名 | 参数事实 | 成功返回 | 非成功与兼容边界 |
| --- | --- | --- | --- |
| `decode_image(chat_name: str, local_id: int) -> str` | 无 create_time/source/output 参数；wrapper 不做显式 `int()` 转换；chat_name 由联系人解析器转换 | 中文“解密成功!”；文件路径、格式、千分位字节数、资源 MD5 | 找不到聊天对象或“解密失败”；可附 MD5。内部 helper 的 dict 不是 MCP 返回结构 |
| `decode_file_message(chat_name, local_id, create_time=0) -> str` | 对两个数值执行 Python `int()`；0 表示不按时间过滤；非零按时间相等 | 中文“找到本地文件”；路径、实际大小、扩展名、期望大小、MD5 匹配说明或启发式警告 | 无消息/歧义/非文件/XML 异常/未下载/校验失败都返回文本，不是统一错误 enum |
| `decode_record_item(chat_name, local_id, item_index, create_time=0) -> str` | 三个数值执行 `int()`；item_index 为直接 datalist 中 dataitem 的 0-based 序号；负值/越界拒绝 | 二进制项：路径、实际/期望大小、发送者、类型/标题、绑定说明；文本项：直接返回发送者和文本 | metadata-only 类型返回“无本地 binary”；未加载与未下载分别提示；不递归定位嵌套记录 |

旧 `resolve_username` 先接受已知 username、`wxid_` 或含 `@chatroom` 的输入，再按显示名忽略大小写精确/包含查找，返回首个匹配；不是同名联系人唯一性检查。[源码](../vendor/wechat-decrypt/mcp_server.py)

迁移要求：

1. 外部兼容层保留旧参数名、默认值及记录序号意义；内部绑定固定账号和精确 username。显示名多匹配应报歧义，不能任取。Python `int()` 的字符串/布尔/小数宽松转换与严格 JSON 整数规则不同，应明确记录有意收紧。
2. 图片旧参数不足以区分同号消息，应提供可选时间/完整 source 扩展，或对旧调用的多匹配明确报歧义；不默认为最新图片。source 扩展不应伪称旧接口已有。
3. 成功内容可以按旧中文模板呈现；原生内部宜区分 missing、ambiguous、wrong_type、invalid_schema、unsafe_path、hash_mismatch、limit、key_missing、write_conflict。此分类是建议，不是新建的生产模型。
4. 如沿用现有原生 MCP 的 JSON-text 和安全错误投影，应显式标注“部分返回兼容”，不能称为旧字符串逐字一致。旧错误包含绝对路径和候选详情；新 MCP 不宜直接透传内部 SQL、密钥、路径错误链。
5. 文件引用的成功字段应说明内容匹配强度、是否新写文件以及来源身份；不把大小、扩展名或 MD5 当成账号认证。不要混淆资源 MD5、候选实际 MD5 和输出文件 MD5。

## 3. 所需 Schema 与定位身份

### 3.1 图片资源

旧图片工具不先查询 message_N.db，它直接调用 `ImageResolver.get_image_md5(username, local_id)`：

| 数据库/表 | 必需字段与行为 |
| --- | --- |
| `message/message_resource.db` / `ChatName2Id` | `user_name` 精确匹配 username，取 SQLite `rowid` 作为 chat_id；旧实现只取一行，不检测重复 username |
| `MessageResourceInfo` | `chat_id`、`message_local_id`、`message_local_type`、`message_create_time`、`packed_info`；type=3 或 `% 2^32 = 3`，按时间降序 LIMIT 1 |
| `packed_info` | 搜 `12 22 0a 20` 后的 32 字节 ASCII hex；失败再扫连续 hex。不是完整 protobuf schema 解码，也不是文件内容校验 |

严格入口还需 message 分片中的 `Msg_<md5(username)>` 真实行，至少 `local_id/local_type/create_time`，验证低 32 位 type=3；固定账号 + 完整逻辑来源如 `message/message_12.db` + 表名 + local_id，时间作额外一致性证据。相同身份多行、相同时间多分片仍是歧义，不应把时间当绝对唯一键。

资源库没有已核实的 message 分片关联字段。即使消息行唯一，资源行 `(chat_id, message_local_id, low32_type, message_create_time)` 仍须唯一；若仍有多行，不得用 rowid/最新时间猜测。不能因为有 source 就声称已解决资源关联。

### 3.2 外层文件与合并记录

两个旧工具都通过 `_find_msg_tables_for_user` 枚举配置密钥清单内的 `message/message_N.db`，验证表名 `Msg_[0-9a-f]{32}`。查询 `sqlite_master`、`MAX(create_time)` 后收集表；未解密/查询异常会跳过，故“扫描所有分片”实际只覆盖已知且成功打开的分片。[源码](../vendor/wechat-decrypt/mcp_server.py)

消息 SQL 必需列：`local_id`、`local_type`、`create_time`、`message_content`、`WCDB_CT_message_content`。条件是 local_id，create_time 非零才加时间相等；先收集匹配，再判类型。每个分片 `fetchone()`，只检测跨分片多匹配，可能漏掉同表重复行。记录工具的歧义详情只保留 table_name，不保留分片文件路径，诊断信息也较弱。

正文处理：CT=4 且正文为 bytes 时 zstd 解压，UTF-8 使用 replacement；解压失败返回 None。仅群聊剥离 `sender:\n` 或受限账号字符后的 `:<msg` / `:<?xml` 等前缀。默认 XML 上限 20,000 字符，拒绝 DOCTYPE/ENTITY；外层含精确 `<type>19</type>` 时允许重试至 500,000 字符，内层 recorditem 同为 500,000 字符。[helpers](../vendor/wechat-decrypt/mcp_server.py)

| 类型 | 严格旧判定与实际读取的 XML |
| --- | --- |
| 外层文件 | low32 base_type=49，appmsg 直接子 `type` 解析为 6，并要求直接子 `appattach` 存在；直接子 `title` 非空且 safe basename；`fileext/totallen` 是 appmsg 后代查找；`md5` 是 appmsg 直接子，不是 appattach 子节点 |
| 合并记录 | low32 base_type=49，appmsg 直接子 type=19；直接子 `recorditem` 的文本作为一层内嵌 XML，读取根直接子 `datalist` 的直接 `dataitem`；不把嵌套 record 的 dataitem 混入序号 |
| dataitem | `datatype` 属性；直接子 `datatitle/datasize/datafmt/sourcename/fullmd5`；文本项再读 `datadesc`。`datafmt` 虽读取，但当前查找和成功输出并未使用它决定路径/扩展名 |

`totallen/datasize` 无法解析时为 0；旧代码用 truthy 判断是否执行大小过滤，并未统一拒绝负数。MD5 仅在非空且长度=32 时启用校验，没有先验证完整 hex；其他长度会进入启发式分支。迁移应把格式错误与字段缺失分开，不能默默降低匹配强度。

## 4. 旧路径查找的真实行为

本节只描述源码规则，未访问这些缓存路径；平台注释不是本机实测证据。

### 4.1 普通图片

`<WECHAT_BASE_DIR>/msg/attach/<md5(username)>/*/Img/<resource_md5>*.dat`，glob 收集后排序。不是仅精确 `<md5>.dat/_h.dat/_t.dat`。

实际选择先找非 `<md5>_` 前缀文件，然后又执行独立循环，遇到 `_h.dat` 会覆盖之前的原图选择。因此“原图 > 高清 > 缩略图”的注释与实际行为不同。Rust 则在每个月内确实 full > h > t，但跨月优先级由月份搜索顺序决定。兼容测试必须固定选择规则，不能拿注释作 oracle。

写出：`<DECODED_IMAGE_DIR>/<md5>.tmp`，解码后改名为 `<md5>.<format>`；旧 final 存在则先 unlink，再 rename。跨账号若共用输出目录、同资源 hash 并发或失败中断，都不能当成隔离/原子不覆盖保证。

### 4.2 外层文件

1. 在 `msg/file/YYYY-MM` 查消息时间当月与前后 31 天对应月份，月份来自集合，无稳定遍历顺序；“前后31天”不总等于相邻历月。
2. 月份快路径使用精确 title，以及 `stem*ext` 宽匹配；有大小时先过滤。快路径并非只接受 `(N)` 副本，可能接纳任意同前缀文件。
3. 仅当快路径没有候选时，才 `os.walk(msg/file)`。慢路径跳过点文件，仅接受精确文件名或 `stem ?(数字)ext` 副本，拒绝单纯子串匹配。
4. 候选存在但 MD5 全不匹配时，不再启动全树兜底。因此快路径坏候选可能挡住其他月份的正确副本。
5. 有有效长度的 expected_md5 时，先确认 realpath 在 `msg` 下，再流式计算 hash，首个匹配即停止；多个相同内容副本不算歧义。无 hash 时，多候选报歧义，单候选返回 filename/size 启发式警告。

根边界是账号 `msg`，不是联系人专属目录；文件名与大小不证明属于当前聊天。无已知大小时匹配证据更弱，不能仍描述为“已核对 filename+size”。

### 4.3 合并记录

基目录 `<base>/msg/attach/<去掉 Msg_ 的表 hash>/*/Rec/*/`，未把 Rec 的通配目录与外层消息 server ID/时间/dataid 绑定；在同一聊天的所有月份和记录缓存中寻找：

| datatype | 子目录与匹配 |
| --- | --- |
| `1` 文本 | 返回 datadesc；不查本地附件 |
| `2` 图片 | `Img/<index>_t`、`Img/<index>`、`Img/<index>.*`、`Img/<index>_*`；扁平文件，不是 index 子目录；有 datasize 时过滤 |
| `4` 语音 | `A/<index>/<escaped datatitle>` |
| `5` 视频 | `V/<index>/<escaped datatitle>` |
| `8` 文件 | `F/<index>/<escaped datatitle>` |
| 其他 | metadata-only，含链接、位置、名片、小程序、视频号及嵌套记录；不使用任意 binary 通配兜底 |

非图片且缺 datatitle、已知非零 datasize 时，允许 `<subdir>/<index>/*` 按大小查找。已有 title 但未命中不退化为 size-only。之后按 fullmd5 校验再判断歧义，与外层文件规则相同；无 fullmd5 的单候选也可能来自另一张卡片，必须保留警告。

“未下载”返回中的通用期望路径使用 `<subdir>/<index>/<title>`，与图片实际扁平路径不一致；提示文字不能当成图片路径模板。序号 i 的提示让用户点击第 i+1 项，不是工具自行下载。

## 5. 文件写入与只读引用

| 路径 | 真实副作用 | 迁移约束 |
| --- | --- | --- |
| 旧 decode_image | 读资源库/DAT、读配置图片密钥、创建明文临时文件、删除旧同名输出并改名 | 应绑定显式受控输出根，预检身份/大小/路径/密钥后写出；默认独占/不覆盖，失败保留旧输出；不得从消息 XML 或工具参数任意指定系统路径 |
| 旧 decode_file_message / decode_record_item | 读消息、glob/walk/stat，必要时读候选计算 MD5，返回现有绝对路径 | 不复制、不写回、不解密、不转码、不下载；返回引用不是内容已经读取、可播放或永久有效的保证 |
| 三个旧工具的公共 DBCache | cache.get 可能解密主库、应用 WAL、写临时缓存 DB 与 `_mtimes.json`；模块导入就创建缓存目录 | “只读附件引用”不等于整个旧进程零写盘。新入口应复用受控账号缓存或显式静态快照，而非复制全局缓存机制 |
| Rust q_extract | 创建输出父目录、读取整个 DAT、调用 default image-key provider、普通 `fs::write` | 不能直接暴露为受控 MCP 写入 API。exists 检查与 write 分离，有竞态；没有原子发布/输出根限制/源输出分离的完整保证 |
| Rust decoder::dispatch | 仅输入/输出内存字节，不访问数据库/文件/账号 | 最适合复用的解码核心；输入限额、密钥授权、输出发布由调用方负责 |

旧 `_safe_basename` 拒绝绝对路径、斜杠、反斜杠、NUL、`.`/`..`，但不是完整 Windows 文件名策略：设备名、ADS 冒号、末尾点/空格、大小写碰撞需补充。旧 `_path_under_root` 的 realpath 前缀检查并未固定后续打开的句柄，也不保证常规文件；glob/stat 可跟随链接。目录 reparse/祖先 junction、文件替换竞态、同一文件硬链接及超大目录扫描都需另行限制。

`_md5_file_chunked` 使用 64 KiB 分块，初始 stat 超过 500 MiB 拒绝；读循环没有累计字节硬上限，增长中文件仍可能超过上限。MD5 比较是沿用旧内容匹配协议，不是签名、账号认证或对抗恶意碰撞的消息来源证明。

## 6. Rust 精确 API 复用表

“直接”只指已有函数的职责可复用，不承诺旧 MCP 整体兼容；可见性以实际代码为准。

| 现有 API | 可复用部分 | 不可直接复用部分 |
| --- | --- | --- |
| `attachment::decoder::dispatch(&[u8], V2KeyMaterial<'_>) -> Result<DecodedImage>` | 直接复用 XOR/V1/V2 分派，结果 `data/format/decoder`；V1 固定 AES，V2 显式密钥，wxgf 为 hevc | 不是文件/记录路径查找，不校验消息归属，不做输入体积或输出路径管理 |
| `V2KeyMaterial { aes_key: Option<&[u8;16]>, xor_key: u8 }` | 显式传入账号密钥材料；`with_aes` 将 XOR 设为 0x88 | 派生 `Default` 的 XOR 实际是 0，而非注释中的 0x88；不得用 Default 冒充旧默认值。配置中的 XOR 必须保留 |
| `message::split_group_content(&str) -> (&str,&str)` | 直接复用旧允许的群发送者前缀处理；群聊条件由调用方控制 | 对非群内容不能无条件调用后丢弃前缀 |
| `message::export_content::extract_with_context(i64, Option<&str>, &ExportContext) -> Result<ExportContent, UnsupportedAppType>` | 继续负责展示正文及 extras；文件标题和记录摘要已有语义 | 不返回 title/size/hash/path 的严格对象；type=6 只是 `[文件]` 文本，未要求 appattach；允许高位 subtype 回退，与旧 decoder 要求 XML type 不同；图片正文会省略 |
| `message::export_content::extract(...)` | 仅测试辅助 | 标记 `#[cfg(test)]`，不是生产可调用 API |
| `message::xml::parse/collapse` | 现有安全 XML 边界可作为同模块实现基础 | `xml` 私有且函数 pub(super)，daemon 不能直接调用；record/app/record_item 也都是私有摘要函数 |
| `attachment::AttachmentId::{encode,decode}` | 已有 v1 base64url JSON 传输格式 | 不是签名/授权凭证；仅校验版本和反序列化，未验证完整身份、正数/尺寸；db:Option<u8> 不是完整 source；枚举输出 db=None，resolver 忽略 db |
| `AttachmentKind::from_local_type(i64)` | 顶层类型分类 | 49 一概 File，未验证 appmsg type=6；resolver 实际没有补这项验证，不能把链接/记录当独立文件 |
| `resolver::lookup_md5_blocking(&Path,&str,i64,i64,i64) -> Result<Option<AttachmentMetadata>>` | schema 和 packed_info 提取思路 | 精确时间也 LIMIT 1；未命中/查询失败后取同 local_id/type 最新行；`.ok()` 将 schema/SQL 失败折叠为缺失；不能承担严格身份 |
| `resolver::extract_md5_from_packed_info(&[u8]) -> Option<String>` | 资源 hash 候选解析，可作为有来源标记的低层 helper | marker/hex 扫描可能误命中，不能作为经过 schema 证明的绑定；Rust 接受大小写并规范小写，旧 fallback 仅小写 |
| `resolver::find_dat_file(&Path,&str,&str,i64) -> Option<PathBuf>` | 已有账号聊天 hash 路径及月份规则 | 仅 Img DAT；没有 fullmd5/file/Rec 查找；月份优先会压过清晰度优先，忽略错误、不做完整 reparse 防护；不验证传入 file_md5 是否安全 basename |
| `resolver::resolve_blocking(&AttachmentId,&Path,&Path) -> Result<ResolvedAttachment>` | 组合上面资源/DAT 逻辑，字段 id/md5/dat_path/size | 所有 kind 最终走 Img；不验证消息分片；不是 generic attachment resolver，也不能把 dat_size 当解密后大小 |
| `daemon::query::q_decode(&DbCache,&Names,&str,i64,i64,DecodeKind)` | 已有只读多分片逐行收集、0/1/2 状态和来源诊断可作为扩展位置 | DecodeKind 只有 Transfer/Location；严格定位核心 `lookup_with_sources` 私有且返回最终解码 JSON，不是可直接借用的通用行查询 API；当前也无 source 入参 |
| `daemon::query::find_msg_shards` / `decompress_message` / `strip_group_prefix` | query 内既有分片上下文和前处理 | 均为私有 helper；解压失败会回退损坏原字节 UTF-8，而旧 Python 返回 None；zstd::decode_all 无解压后限额，不宜原样承诺严格错误契约 |
| `cli::decode::cmd_decode(Request,bool) -> Result<()>` | 已有文本/JSON 输出，一般失败退出1、歧义退出2 | 是 CLI 终点，会 process::exit；不是 MCP 库回调。必须先有真实 Request/daemon 路由，不能传提案 Request |
| `q_attachments` / `q_extract` / `cli::extract::cmd_extract` | 现有附件枚举和显式句柄 Extract 工作流 | 枚举不是按 local_id 严格取单条；Extract 返回 kind/md5/dat_path/dat_size/output/output_size/format/decoder，不是旧 path/size/text 契约，且写入风险见上 |
| `toolkit::images::decode_image(String,Option<String>)` | CLI 已复用字节解码和原子输出 | 读取 raw_config、打印 JSON，默认输出靠近输入；不是账号固定的 MCP 服务 API；`atomic_output` 与不覆盖发布须分别核验，不能因“原子”推导“不覆盖” |

函数所属文件：[ID](../src/attachment/attachment_id.rs)、[字节解码](../src/attachment/decoder/mod.rs)、[Extract](../src/daemon/query.rs)、[CLI decode](../src/cli/decode.rs)、[图片 CLI](../src/toolkit/images.rs)。

Rust query 自身的 `parse_appmsg/format_file_appmsg/format_record_appmsg` 也是展示函数：记录展示只取前10项且没有旧 `[N]` 序号；export_content 的记录摘要取前50项、每项截断200字符。两者都不能从已格式化文本反向恢复完整 dataitem，后续入口必须从原 XML 按原索引读取。

## 7. 最小迁移边界与风险排序

1. **先修身份契约再接入**：账号固定；完整逻辑 source 与消息类型复核；多行 fail-closed；资源库精确条件不命中不得取最新。时间相同仍歧义，source 与资源表无法唯一关联时明确失败。
2. **拆开引用与写出**：文件/记录只返回受控根内常规文件引用；普通图片另有受控写入权限、尺寸限额和独占发布。禁止把三个工具统一路由到 Extract。
3. **保留原 XML 解析边界**：直接 appattach/type/fullmd5 字段、双层上限、DOCTYPE/ENTITY 拒绝、未加载与 malformed 分开。展示层不增加附件 I/O，不从摘要重建元数据。
4. **查找与验证分阶段**：有限目录候选、固定常规文件句柄、大小/hash 校验、来源标记、再返回。无 hash 的启发式单候选保留显著弱绑定说明；是否允许这种降级是明确产品契约，不得悄悄称为确定匹配。
5. **兼容差异必须公开**：整数收紧、图片旧最新行语义、h 覆盖原图、月份宽匹配、MD5 格式错误、同表重复、旧错误路径泄漏以及默认覆盖，不应为了逐字兼容而移植危险行为。

这里没有新建模型或抽象。后续如实施，优先在既有 query/decode 的边界内补通用严格定位，再调用纯解码核心；准确的函数签名与模块可见性应在生产变更时单独审查。

## 8. 验收清单与证据

以下是后续合成验收要求，本轮未声称已执行：

- 同联系人跨分片同 local_id；同时间冲突；同表重复；resource 同键多行；精确资源缺失不得回退最新；联系人同名；账号隔离与伪造 AttachmentId。
- TEXT/BLOB/NULL、CT=4 正常/损坏/解压炸弹、高位类型、群前缀两种形态、XML边界值、DTD/ENTITY、缺 appattach、type=5/6/19 混淆。
- 记录0-based索引、负值/越界、超过展示50项仍能精确定位、嵌套记录不展开、datatype=1/2/4/5/8及metadata-only、datafmt不作为真实格式证明。
- 图片 full/h/t 与跨月排序；文件快慢路径差异；缺标题 size-only；多个候选有/无 hash；MD5不符/格式错误；Rec跨卡片同索引同名同大小。
- Windows ADS/设备名/尾点空格、根与祖先junction、文件链接/目录冒充文件、检查后替换、文件增长超限、受控输出冲突和失败保留原文件。
- 引用工具零附件写入/下载；图片不覆盖源或已有输出；无隐式密钥扫描；缺V2 key 明确失败；MCP错误不泄漏路径/SQL/密钥，成功不伪造大小/hash。

已有 [test_record_decoders.py](../vendor/wechat-decrypt/tests/test_record_decoders.py) 只覆盖 helper，文件头明确两个工具 wrapper 不在该套测试中；其直接 `import mcp_server` 会触发全局缓存初始化，本轮没有运行。Rust resolver 的精确时间用例不覆盖“精确缺失后最新回退”；decode 测试覆盖跨分片和同时间冲突，但不等于附件工具端到端验收。

本轮验证为源码与文档静态审计；未运行 Cargo 全仓测试、未调用任何账号工具。未修改 `docs/diagrams`。读取完整输出保存在 [审计日志](native-attachment-audit.log)；日志中保留重复读取内容，部分界面输出有截断，不表示源文件缺失。两次探索错误（误猜 image_resolver.py 文件名、Windows rg 文件通配参数）已在对话完整报告，实际类定义随后从 decode_image.py 核实。
