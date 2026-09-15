# Strict MCP media boundary

## Production wiring

- `mcp_image::q_decode_image_guarded` uses the shared strict message snapshot
  callback to capture `media::strict_message::Message`. Its fields are private.
  It checks a typed image predicate, asks `strict_image::AccountSources` for an
  opaque cache descriptor, and holds the host `ResourceSnapshot` while the
  adapter assembles `Proof`. The host does not decode type bits, construct cache
  directories or extract resource row IDs/digests.
- `strict_image::Proof` reuses `ImageSource::from_reference`, business media
  discovery and the existing strict native exporter. Full original type bits,
  message timestamp, logical source and username remain part of revalidation.
  The source inventory uses the existing message inventory adapter directly,
  plus the original bounded resource inventory and source/WAL identity checks.
- `mcp_attachments::q_attachment_reference` selects business `Selection::File`
  or `Selection::RecordItem`. The media adapter handles bounded decoding and
  `MessageInput` construction using the existing attachment parser. The host
  still acquires and holds `FileReference`, verifies its read handle and renders
  the existing metadata/reference/status wire result.

## Preserved Security And Compatibility

The host retains account/cache authority, output authorization, in-memory key
zeroization and resource snapshot acquisition. The existing native exporter
retains resource revalidation, read-only file/parent pins and atomic publication.
No adapter imports daemon execution. Source verification happens before export
and again at the exporter's prepublication callback, with no fallible operations
added after successful publication.

Strict image layout remains exactly `msg/attach`, sharing the account-root helper
but not the legacy DAT month-priority selection. Duplicate resource aliases, unknown source files, full-type or
timestamp mismatch and ambiguous exact resource matches still fail. Existing
host error strings and metadata serialization remain compatible. Attachment
decode-limit failures propagate while parser failures retain their bounded wire
mapping; malformed XML is not treated as a missing local file.

## Tests And Integration Status

- Existing `tests/fixtures/mcp-image/tests.rs` tests remain wired to the real
  query: full type flags, zero-time lookup, duplicate identity/resource aliases,
  unknown shards, source/WAL mutation, explicit V2 keys, output protection and
  unchanged input bytes. Inventory tests now use `AccountSources` rather than
  the removed host inventory implementation.
- Added `opaque_media_evidence_rejects_changed_kind_before_resource_proof`:
  evidence captured in one real message snapshot is rejected after changing the
  physical type before the next snapshot, with a typed stale-evidence error and
  no published output.
- Added `strict_media_rejects_unbound_attachment_root`: a DAT present only in
  another root cannot produce strict MCP output.
- Added `strict_media_rejects_cross_month_ties_that_legacy_lookup_selects`: the
  actual legacy resolver selects its preferred month, whereas strict MCP rejects
  the same equal-rank cross-month candidates with no output.
- `native-attachment-security` locally wires the real strict media adapter,
  reusing the original parser and file-reference implementation. Added high-type
  compatibility and non-attachment rejection through the real query. Existing
  SQL ambiguity, unknown/missing shard, bounded decode, account isolation and
  nested-hash rejection tests remain intact.

Cargo and tests have not been run for this change. The parent owns targeted
integration validation; no full-suite pass is claimed.

## Shared Message Compatibility Boundary

`strict_message::with_resolved<T>` remains the shared snapshot host and is not
modified here while Tesla owns the message-read work. Image and attachment hosts
do not use detached `StrictMessage`, `locate`, `bounded_decode` or a default type
parameter for `Resolution`. The parent is migrating the final refer consumer to
typed `ReplyRead` and removing those legacy shared APIs. That coordinated change
is outside this slice; no replacement production message locator is introduced.
