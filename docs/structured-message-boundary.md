# Structured Message Boundary

## Implemented

- `business::structured_message` owns the typed, read-only preview model. It has
  no database, process, daemon, concrete adapter, or JSON-value dependency.
- `adapters::wechat::structured_message::decode` owns WeChat numeric kind
  decoding, XML shape checks, bounded field extraction, and safe URL handling.
  It reuses the existing message parsing helpers; it neither retrieves nor
  resolves media.
- The history and new-message query projections serialize the same typed model.
  CLI, HTTP, and MCP continue to consume their existing daemon query results.
- The previous `daemon/query/rich_message.rs` production implementation is
  removed, not retained as a second parser.

## Compatibility

Public `rich` keys, tags, limits, safe URL rules, and fallback summaries remain
unchanged. Unsupported kinds, oversized input, malformed XML, and an absence of
safe preview fields are distinct internal `ContentIssue` values. Existing query
projections intentionally omit `rich` on these conditions and preserve the
ordinary content summary; this does not assert that a structured preview was
successfully parsed.

Unknown application subtypes retain the explicit legacy link-preview fallback.
Nested quoted payloads cannot select the outer message kind. Preview extraction
does not authorize downloads, memory scans, writes, or uploads. A voice-message
preview is not a call recording. Call events are unsupported by this preview
decoder, and status words never infer an audio/video medium.

The migrated synthetic tests exercise bounded XML, malformed and unsafe input,
group prefixes, packed types, quotes, transfers, and all existing preview tags.
An additional typed-result test distinguishes failure reasons and verifies the
voice-message/call-event boundary. Test execution results belong in the overall
stage validation report; moving these tests alone is not proof of passage.

## Remaining Message Work

This is the structured-preview part of the architecture migration, not a claim
that the entire message domain is separated. Message inventory, conversation
selection, shard queries, stable message identity, call-event querying, raw
exports, and media resolution still require their respective business slices.
Existing format helpers under `message` remain shared dependencies of the
adapter until those callers are migrated; this document does not describe them
as storage-independent business models.
