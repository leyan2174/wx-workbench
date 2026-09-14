# Article Pushes

## Scope

This slice reads locally received official-account pushes. It does not fetch
article bodies, download covers, or claim complete remote publication history.

The business operation owns publisher substring filtering, inclusive receive-time
boundaries, descending publication-time ordering and the existing unread policy:
intersect with unread official-account publishers, then keep one latest article
per publisher before applying the result limit. Publication time falls back to
receive time when the encoded value is absent or invalid. Equal URLs are not
article identity and must not collapse separate pushes or items.

## Adapter Boundary

The article adapter expands WeChat multi-article XML after the shared message
adapter has discovered official-push shards, resolved publishers and decoded
content. It must not independently implement inventory SQL, Name2Id mapping,
message table discovery or decompression. Session inventory is used only for
the explicit unread policy, never as the article-source inventory.

Legacy fragment extraction remains available for malformed XML; recovered
articles carry an invalid-content issue rather than claiming complete evidence.
Unknown publishers are reported and omitted, not replaced by display names or
empty usernames. Result pagination is separate from incomplete source reads.

## Current Integration State

The production query uses the shared host inventory preparation and message
snapshot, then the article source and business operation. Old query SQL, content
decompression and XML parsing were removed. Article evidence reuses the shared
snapshot-bound message reference plus item ordinal; it is not a cross-snapshot
identifier. The source read and expanded article inventory are bounded at
100,000 candidates; a reached bound is reported conservatively as unfinished.

Existing article fields are retained. The response adds `partial`, `has_more`,
`source_unfinished` and fixed issue codes. Missing inventory or unread-source
errors fail the request instead of producing an empty success. Known non-article
items without title or URL remain omitted. CLI projection writes one fixed
warning to stderr for partial or unfinished source reads, retaining its existing
article-array stdout output in text and JSON modes. Ordinary result pagination
does not trigger this warning, and raw issue details are never echoed.

Tests use memory sources, synthetic XML and real shared snapshots over synthetic
SQLite, including compressed pushes, repeated local IDs across shards, unknown
publishers and bad unread rows. Parent-coordinated Cargo verification is pending.
