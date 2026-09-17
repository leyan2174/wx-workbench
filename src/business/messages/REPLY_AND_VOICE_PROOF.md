# Reply previews and table validation

## Ownership

- The business `Reply` contains text, `sender_label` and quoted summary.
  This is a quote preview, not an author identity: the label can be a display
  name, fallback text or "me". No stable identity is inferred from display text.
  The protocol still projects it as `refer_sender`.
  It has no JSON, database, XML or numeric WeChat message-kind dependency.
- `adapters/wechat/messages/reply.rs` owns strict type-57 XML decoding,
  sender precedence and raw diagnostic evidence. It reuses the existing safe
  XML parser, whitespace normalization, type labels and summary algorithm.
- `daemon/query/mcp_refer.rs` projects the typed result into the
  protocol object and renders the text output.
- `adapters/wechat/messages/reply_read.rs` performs base-kind checks,
  bounded decoding, group-prefix handling and account-directory interpretation
  while the strict snapshot is live. Its `LegacySource` is serialized only at
  the response boundary; ordinary host code does not inspect its coordinates.
  The host uses the shared live-snapshot callback and inventory checks
  without materializing adapter-owned records. Not-reply and invalid-content outcomes remain distinct and sanitized.
- `messages/read/layout.rs` owns username hashing, canonical message-table
  names and SQLite table-name validation. These checks describe physical
  layout, not source existence or account authenticity.

## Compatibility

The strict reply endpoint still bounds decoded content at 131072 bytes,
checks base kind 49, strips group sender prefixes, resolves unique source
identity before parsing and sanitizes parsing errors. Direct-root appmsg
and namespaced elements are rejected. Signed ASCII
integers with valid underscore separators are accepted.
Diagnostic timestamp and server-ID strings are not narrowed to integers.
Response projection preserves the protocol fields, status codes and sender
fallback precedence. The best-effort export preview is a separate contract
and does not use the strict endpoint parser.

## Synthetic validation

- Typed XML fixtures cover sender precedence, preserved diagnostic values,
  invalid shapes, namespaces, unsafe XML and sanitized errors.
- Layout tests cover the canonical table-name and SQLite case profiles
  against the shared hash rules.
- The readonly-security fixture explicitly registers the production reply
  adapter.
- The real encrypted-cache query fixtures cover field/rendering
  assertions and the exact seven-field response shape.
  Decode-limit tests use the live-snapshot callback and the same detached
  content implementation, without a separate host record.

Execution commands and dependencies are listed in the
[test guide](../../../tests/README.md). Synthetic checks do not establish
real-account completeness or authenticity.
