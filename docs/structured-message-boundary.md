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

The synthetic tests exercise bounded XML, malformed and unsafe input,
group prefixes, packed types, quotes, transfers, and all existing preview tags.
A typed-result test distinguishes failure reasons and verifies the
voice-message/call-event boundary. See [test instructions](../tests/README.md) for execution.

## Related contracts

Message inventory, conversation selection, shard queries, identity and raw export
are described in [message read contracts](business-messages.md). Media association
uses the separate [media boundary](media-boundaries.md). Format helpers are adapter
dependencies, not storage-independent business models.
