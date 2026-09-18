# Voice catalog boundary

## Ownership

`business::voice::catalog` owns `Query`, `Entry`, `Page`, exact chat-label
resolution, semantic input validation and the narrow `Source` contract. It uses
only the standard library, with no SQLite, WeChat, daemon or JSON dependency.
An entry is a voice preview: account-local username, timestamp, optional byte
length and opaque source evidence. It is not an audio export or a call recording.

`SourceRef` hides adapter evidence, including from Debug output. Handle equality
only compares a preview's evidence allocation, not message identity across reads
or accounts. Neither `local_id` nor the handle is a unique message ID. Evidence
is not authorization, a capability to read audio, or proof of current row
ownership. Audio resolution retains its separate strict account/source checks.

The WeChat `media::voice_catalog::Catalog` implements `Source`. It owns schema
validation, SQL, physical attribution, ordering, paging and SQLite integer bounds.
The query algorithm has one production implementation. Its explicit
offline inventory is a caller precondition, not a claim that an arbitrary subset
proves account completeness. Missing, corrupt, aliased or ambiguous supplied
shards fail the whole read.

`daemon::query::mcp_voice::q_voice_messages` supplies the fixed account's inventory
and resolved paths through the existing DbCache. It retains pre/post disk
inventory checks, raw cache-key lookup and canonical duplicate rejection, and
returns a business Page. Authentication, account selection and query lifecycle
belong to the daemon boundary. Audio export is a separate operation.

## Response contract

Business Page carries `PageContinuation::{Exhausted, MayHaveMore}`, with the
same names and meaning as the messages page state. The two-value enum stays local
to avoid coupling the standalone voice capability to the message model. Source
completeness is a success precondition, not a statement that pagination ended.
A successful empty/short page is Exhausted; a full page is MayHaveMore even when
it happens to contain the last records. There is no definite More state and no
additional query, lookahead or larger candidate budget. This describes the
current query result, not future arrivals. Invalid state/length combinations
are rejected by business list. The public response does not include this state.

Only the VoiceMessages response boundary calls the adapter's `legacy_rows`.
Its `LegacyVoiceMessage` contains seven fields: username, source, chat_name_id,
media_rowid, local_id, create_time and voice_data_bytes. Foreign evidence or
changed preview metadata is rejected. The response contains `voices` and
`count`; each endpoint enforces its documented pagination limits.

Ordering remains create_time descending, canonical source lexically ascending,
local_id descending, rowid descending. Per-shard candidates remain offset+limit,
followed by global truncation and paging. Repeated IDs within/across shards are
not deduplicated. Both time endpoints are inclusive, including zero and negative
times. SQL NULL remains JSON null, while a zero-byte BLOB remains zero. Exact
username lookup remains case-sensitive; display labels require one exact match,
and an explicit username wins over a conflicting display label.

## Synthetic coverage

- Memory Source tests cover preserved duplicates/order, opaque evidence, NULL vs
  zero semantics, inclusive endpoints, validation before IO, unavailable sources,
  invalid returned pages and exact/ambiguous labels.
  Additional pure-memory cases distinguish complete-inventory success from
  empty/short/full pagination at zero/nonzero offsets, reject false exhaustion
  on a full last page, and assert exactly one source read (no probe).
- SQLite adapter tests traverse business list + the real Catalog before checking
  response projection. A JSON golden covers every response field, repeated local IDs,
  rowid tie-breaks, canonical source labels and offset slicing. Foreign/modified
  evidence and SQLite pagination overflow have direct tests.
- Adapter tests cover aliases, schema validation, ownership and read-only access.
  The security fixture retains its independent cross-shard ordering/paging
  oracle, Name2Id differences, ambiguous media and incomplete inventory tests.
- Both independent fixtures use the real business/adapter/query modules. Their
  cache substitute tests only the public cache boundary, not decryption or live
  accounts. Inventory tests cover unavailable/extra/canonical-duplicate shards
  and a shard added during cache resolution.

Run the root checks and mcp-voice / mcp-voice-security fixtures according to
[test instructions](../tests/README.md). Inventory checks do not guarantee a
cross-database atomic snapshot.
