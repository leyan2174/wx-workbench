# Raw voice export

`cmd_voices` consumes `business::voice_export` selection and the real
`adapters::wechat::media::voice_export::Catalog`. Selection and database adaptation
own name resolution and Name2Id/VoiceInfo queries, rather than the CLI.
The daemon supplies protected database material through the existing worker
broker. Production workers do not open the material Store directly; direct
synthetic Store access is confined to test assembly.

Directory selection alone is not proof of a strict message association.
It accepts the historical `message/media_*.db` key inventory (including textual
suffixes). Exact username wins over case-insensitive substring matches; ambiguous
matches fail. The host separately attempts a strict message/media join against
the complete account source inventory; it does not invent missing message sources.
The exact, descending metadata-only voice catalog is a separate contract.

Time endpoints remain inclusive. Ascending time/local-id ordering and offset/limit
are applied once across all shards, not separately per shard.
Stable ties retain sorted shard order and then SQLite rowid order. A zero limit
selects nothing. Unknown Name2Id rows retain the historical skip policy, now
reported as `unmapped_rows`; unavailable cached shards are `missing_shards`.
These conditions contribute to `incomplete_items` and the summary `partial`
flag. Invalid available schemas fail the operation; missing or invalid selected
audio is recorded in the manifest, rather than becoming empty success.

The adapter holds read-only SQLite transactions while selecting and reading
selected BLOBs. It does not preload every audio payload. Coordinates and raw bytes
stay in the adapter; the business layer contains metadata and pure selection only.
Metadata memory still scales with the legacy inventory; this is not a streaming
metadata cursor. Snapshots are per shard, not a cross-database atomic snapshot.

Both the synchronous CLI and the persistent task use the daemon's
`prepare_voice_snapshot` before catalog and strict association reads. The host
prepares an account-scoped private decryption snapshot; each source is then
copied with SQLite Backup into a `ResourceSnapshot` held by `VoiceSnapshot`.
Backup includes committed WAL data. Only the private copy is switched to
`journal_mode=DELETE`; source journal modes and sidecars are not changed or
deleted. Strict readers still reject WAL/SHM/journal files. This preparation
does not establish cross-shard atomicity or compatibility with every live schema.

Each export prepares its private decryption snapshot, which can take substantial
time for large databases. If a source changes while it is being read, the
operation refuses that attempt; retry after source writes have settled. Do not
delete WAL/SHM/journal files to bypass this check. Success is not guaranteed
while WeChat continues writing.

The original SILK bytes are preserved, including an existing `0x02` prefix.
No prefix normalization, WAV/MP3 conversion, decoding or upload is performed.
Complete chat exports retain voice references; raw SILK and an association
manifest are the handoff to downstream tools. A media-directory entry alone
does not prove message association. Missing or ambiguous evidence must not be
replaced with a guessed message identity.

The command fixes one RuntimeContext, checks the worker's expected account and
holds the existing ConfigPin. Store, cache directories and publication protection
all use that context without rediscovering configuration. All audio, evidence and
summary files use ExportTarget. Audio/evidence use no-clobber when overwrite is
off, otherwise captured replacement; both are captured before audio publication.
Summary retains replacement semantics and is captured before database reads.
The output root and per-chat directory are checked against protected account
resources before directory creation. This is deliberately not a multi-file
transaction: a failed evidence commit returns an error and leaves already
published audio intact; previously published evidence/summary is not truncated.

## Task Publication

The persistent `export_voices` task uses the same selector and raw-byte writer,
but publishes only into a fresh host-controlled task directory. Audio and
sidecar must both pass registration before their group becomes visible in the
task artifact index; a half-published group is not counted as exported. Earlier
complete groups survive later failure or cancellation. This index atomicity
does not make the two filesystem writes a single transaction. Task manifests
omit output absolute paths. The synchronous CLI retains its existing overwrite
and summary replacement behavior.

## Manifest 字段

条目结构以 [ManifestItem](voice_export.rs) 为准。所有字段均序列化；可选字段未知时为 JSON `null`，不以空字符串、零或猜测值补齐。未知 sender、duration_ms 均为 null；已知的零时长与未知时长不同。

本表指 `summary.manifest[]`，不是所有对象的同名字段。任务 `summary.items[].timestamp` 和 sidecar 顶层 `timestamp` 保持媒体时间，`timestamp_source=media`；sidecar 独立的 `message_timestamp` / `message_timestamp_source` 只记录已证实的消息时间，没有证据时为空。当前严格反向关联要求消息与媒体时间一致；冲突仍为未证实，不为投影测试放宽规则。选集、分页和文件名始终使用媒体侧时间，补证不会反向改写它们。

| 字段 | JSON 类型 | 含义 |
| --- | --- | --- |
| `account_id` | string | 固定账号的 runtime 标识；用于限定身份作用域，不是访问媒体的凭据。 |
| `message_id` | string / null | 精确会话标识与非零服务端消息 ID 组成的身份，不依赖物理分片或 rowid；仅在当前账号范围内使用，account_id 单独提供。其值是二元素数组的 JSON 字符串，元素为会话标识及十进制服务端 ID 字符串；应作为不透明标识处理。 |
| `conversation` | string / null | 精确会话 username，不是显示名。 |
| `sender` | string / null | 可由消息证据确定的发送者标识；不能确定时为 null。 |
| `timestamp` | integer / null | 消息时间采用 Unix 秒。严格关联成功后使用消息时间；未关联时仅有媒体侧时间证据，不能将其解释为已证明的消息时间。结合 `evidence` 判断来源，不根据文件名补时间。 |
| `duration_ms` | integer / null | 消息元数据中可验证的时长，单位毫秒；不解码音频估算。缺失或不能可靠解析时为 null。 |
| `encoding` | string / null | 确认原始 SILK 时为 `silk`；未知为 null。格式标记不保证下游解码成功。 |
| `relative_path` | string / null | 相对本次导出根目录的已发布媒体路径；没有可交付路径时为 null，不是绝对源库路径。 |
| `status` | string | `success`、`missing`、`failed` 或 `disabled`：写出成功、媒体缺失、处理失败或显式禁用。各入口的适用范围见下文。 |
| `association` | string | 关联证据级别；与文件处理成功与否分别判断。 |
| `evidence` | object | 来源坐标与关联证据；媒体分片、rowid、媒体 local_id 等仅为证据，不是稳定消息身份，也不是访问授权。`voices` 以 `timestamp_source` 区分 `message` 与 `media` 时间来源。 |
| `failure` | string / null | 当前条目的失败说明；无文件处理失败时可为 null，即使关联仍未证明。 |

`message_id` 不使用 SQLite rowid、媒体 local_id、消息 local_id 或时间戳替代非零服务端 ID。消费方以 `account_id + message_id` 区分账号；非空 `message_id` 也不能单独证明媒体已关联或已发布。

## voices 汇总与退出

`wx voices` 在导出根写入 `_voice_export_summary.json`，并输出同一汇总。可交付条目位于 `items`，选中媒体记录的完整处理结果位于 `manifest`，后者包含失败与缺失项。

- `manifest[].status` 为 `success`、`missing` 或 `failed`，分别表示写出成功、媒体缺失或处理失败。
- `manifest[].association` 为 `exact_message_media_join` 或 `unproven`。前者需要严格消息/媒体关联及原始字节核对；目录匹配本身不构成该证明。
- 未证明关联的条目不补造 `message_id`、发送者或时长。`evidence.timestamp_source = "media"` 时，时间仅为媒体侧证据；严格 join 后 `timestamp` 使用消息时间，标记改为 `"message"`，证据另含消息侧关联信息。
- `success + unproven` 表示原始文件已写出但关联未证明；即使 `failure` 为 null，也不是完整成功。
- `incomplete_items` 累加未映射媒体行、缺失媒体分片、缺失消息分片，以及状态非 success 或关联 unproven 的 manifest 条目。它不是单纯的失败文件数。
- `partial` 等于 `incomplete_items > 0`。`exported` 统计已写出条目，`associated` 统计严格关联条目；关联成功不保证文件写出成功。

汇总先发布和输出，再根据 `exported` 与 `incomplete_items` 判断业务结果。有未完成项时以非完整成功的非零状态退出；已成功发布的文件保留，不因部分失败回滚。来源无法读取、汇总无法发布等操作级错误也不能解释为空结果成功。

## 聊天目录清单

完整聊天目录导出在目录根生成 **`_voice_manifest.json`**（有前导下划线），外层结构为 `{"version": 1, "items": [...]}`，条目使用同一 `ManifestItem`。它与 `_media_manifest.json` 并存，聊天内容保留语音引用，`relative_path` 相对该聊天导出目录。

这里的身份和发送者来自对应聊天消息；非零服务端 ID 缺失时 `message_id` 为 null。直接语音的严格媒体恢复使用 `exact_message_media_join`；转发记录内语音沿用附件引用的绑定证据，不能一律宣称为严格消息/媒体关联，其消息身份属于外层聊天记录。

目录清单将媒体 `available` 映射为 `success`，同时保留缺失、失败及显式禁用媒体时的 `disabled` 状态。未提供绑定证据时为 `unproven`；附件引用可携带自己的绑定标记。因此不要将 `voices` summary 的状态和关联取值集合直接作为聊天目录清单的全量枚举。

原始文件、聊天引用和清单一起交给下游工具；本产品不产生识别文本、不管理识别模型，也不回写识别结果。

## Verification

Tests use in-memory business sources and synthetic SQLite files only: global
pagination, inclusive endpoints, ties, fuzzy ambiguity, legacy source names,
raw-byte preservation, lazy BLOB reading, read snapshots and schema failures.
Execution requirements are in the [test guide](../../tests/README.md).
