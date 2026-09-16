# Chat export planning contract

`business::chat_plan::Plan` combines typed message, resource and voice-size
contributions from its narrow Source interface. It owns checked accumulation,
missing/error distinctions, legacy zero-timestamp aggregation, per-chat order,
and typed partial reasons. It contains no paths, SQL, configuration, threads or
CSV writer. A malformed contribution vector is an error, not missing data.
MessageStatistics and ordered source indexes describe contributions, not physical
tables or shard numbers. Semantic reasons such as MessageSourceMissing and
ConversationAbsent have no legacy wire labels in the business layer; toolkit's
projection alone maps them to the unchanged CSV status codes and sorts those codes.

`adapters::wechat::planning` owns read-only SQLite aggregation and the existing
cache inventory rules. Message naming uses the shared messages read layout.
Explicit relative database lists remain supported; they are not forced into a
strict message-identity snapshot requiring unrelated columns. Cache discovery
retains digit-only shard names and lexical order, fixed resource-file precedence,
and refusal of multiple resource shards when no fixed resource file exists.
This describes the available cache, not a complete account inventory.

The planning adapter's `scan` module owns the WeChat media-root mapping, directory
targets, missing-lane interpretation and unchanged pinned attribute-only scanner.
`application::chat_export_plan` owns scan thread execution, guard lifetimes and presentation.
It converts typed reasons to sorted legacy
`partial:...` labels only at the output boundary. The CLI CSV publisher remains
the existing shared ExportTarget implementation, unchanged by this slice.

Compatibility rules:

- Time endpoints are inclusive. SQL NULL remains unknown; the query preserves
  zero timestamps while the aggregate omits them from first/last output.
- SQLite length(TEXT) counts characters, while length(BLOB) counts bytes. The
  three stored content columns are summed without decoding or replacing the
  estimate with UTF-8 string length.
- Message shards accumulate; missing tables and failed reads stay distinct.
  Resource reads are all-or-error across selected chats. Media aggregation stops
  at its first failed shard and retains all earlier contributions.
- Scan counts entire username-hash directories under attach/file/video, not
  just the selected time interval. Hardlinks and duplicate contents count per
  directory entry. Scanned size never replaces the estimated total.
- Existing scan limits, partial bytes, attribute-only reads, ancestor pins,
  junction rejection and worker range 1..6 are retained.
- CSV columns, UTF-8 BOM, CRLF, quoting, chat indices and order stay unchanged.

Validation includes memory-source business tests, real synthetic SQLite adapter
tests, the existing Python/SQLite differential and scan security tests in
`application/chat_export_plan_tests.rs`, plus `tests/chat_plan_runtime.rs` and its fixtures.
The root business-contracts test loads the actual business module registration.
No new fixture stand-ins or private account inputs are required.
