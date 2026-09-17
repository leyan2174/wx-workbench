# Legacy voice export

`cmd_voices` consumes `business::voice_export` selection and the real
`adapters::wechat::media::voice_export::Catalog`. The command no longer discovers
media keys, resolves fuzzy names or interprets Name2Id/VoiceInfo with its own SQL.
Database credentials are read through the existing encrypted Store API.

This contract is a legacy VoiceInfo directory, not a strict message association.
It accepts the historical `message/media_*.db` key inventory (including textual
suffixes). Exact username wins over case-insensitive substring matches; ambiguous
matches fail. No message database or MessageRef is invented. The strict voice
adapter and the exact, descending metadata-only voice catalog are separate contracts.

Time endpoints remain inclusive. Ascending time/local-id ordering and offset/limit
are applied once across all shards, not separately per shard.
Stable ties retain sorted shard order and then SQLite rowid order. A zero limit
selects nothing. Unknown Name2Id rows retain the historical skip policy, now
reported as `unmapped_rows`; unavailable cached shards are `missing_shards`.
Either condition sets the additive summary `partial` flag. Invalid available
schemas and invalid selected audio fail, rather than becoming empty success.

The adapter holds read-only SQLite transactions while selecting and reading
selected BLOBs. It does not preload every audio payload. Coordinates and raw bytes
stay in the adapter; the business layer contains metadata and pure selection only.
Metadata memory still scales with the legacy inventory; this is not a streaming
metadata cursor. Snapshots are per shard, not a cross-database atomic snapshot.

Existing SILK prefix normalization, evidence fields, output names, overwrite
rules and host lifecycle remain unchanged. This command has no optional decoder, worker, upload or remote authorization.

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

Tests use in-memory business sources and synthetic SQLite files only: global
pagination, inclusive endpoints, ties, fuzzy ambiguity, legacy source names,
raw-byte preservation, lazy BLOB reading, read snapshots and schema failures.
Execution requirements are in the [test guide](../../tests/README.md).
