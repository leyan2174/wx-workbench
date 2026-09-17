# Reply previews and voice proof validation

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
- `messages/read/layout.rs::valid_voice_source` is the shared physical
  source-name and username/table ownership check. Prepared-audio and receipt
  callers use this check for source naming and table hashing.

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

Prepared audio uses its own source-length policy (the outer response
budget still applies). Receipt additionally caps each source at 128 bytes,
rejects blank trimmed usernames and requires a positive media ID.
Both require exact canonical username/table binding, a positive message local
ID and a nonzero server ID. Neither check requires positive
timestamps, server IDs or other physical media fields.

These checks validate syntax and ownership, not source existence, uniqueness
or account authenticity. Existing request/evidence matching, trusted-channel
account binding, receipt conflict persistence, audio size/header/hash checks,
strict JSON deserialization and cache account isolation remain in place.
Hashes and receipts are not signatures; no authentication claim is added.

## Synthetic validation

- Typed XML fixtures cover sender precedence, preserved diagnostic values,
  invalid shapes, namespaces, unsafe XML and sanitized errors.
- Layout tests cover traversal/separator rejection, wrong source kinds,
  foreign username tables and the receipt length boundary.
- Prepared-audio and receipt tests exercise the actual shared check,
  compatibility differences, request mismatch and persistent source conflict.
- The readonly-security fixture explicitly registers the production reply
  adapter; existing audio fixtures already register the production read layout.
- The real encrypted-cache query fixtures cover field/rendering
  assertions and the exact seven-field response shape.
  Decode-limit tests use the live-snapshot callback and the same detached
  content implementation, without a separate host record.

Execution commands and dependencies are listed in the
[test guide](../../../tests/README.md). Synthetic checks do not establish
real-account completeness or authenticity.
