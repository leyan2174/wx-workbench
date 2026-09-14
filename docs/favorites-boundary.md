# Favorites Boundary

The existing favorites query now uses:

`CLI favorites -> existing query RPC -> daemon query -> business::favorites::list -> WeChat Source`

The current HTTP routes and MCP query allowlist do not expose favorites. This
slice does not add routes, tools, or permissions; it migrates the existing CLI
and shared query RPC implementation. Future authorized projections can reuse the
same business capability, but that is not an implemented entry point here.

The source is assembled from the existing account-bound `DbCache` and executes
in the existing blocking query boundary. No additional cache, runtime selection,
queue, worker, configuration, or database is introduced.

The business model owns favorite identity, semantic kind, text content, author,
conversation, update time, article reference, Unicode preview, and page coverage.
Private XML is retained only in adapter provenance for the explicitly compatible
legacy preview, never returned as a business text body.
It exposes no SQL, physical table, kind code, or database row. One source is bound
to one account; a favorite's logical local identifier is not combined across
accounts or treated as a cross-shard message identity.

The adapter owns schema checks, nullable field decoding, millisecond timestamp
conversion, literal SQL LIKE escaping, and the existing numeric type filter.
Legacy `id` and `type_num` fields are resolved from opaque provenance only at the
compatibility projection. Unknown source kinds remain `Other` in business code
while their original numeric value survives the public compatibility projection.

Public query names, arguments, successful result fields, 100-character previews,
optional article URLs, and numeric filter semantics are unchanged. Newest-first
ordering now normalizes seconds/milliseconds before comparison, correcting mixed
unit ordering, and uses logical identity to make equal-time ordering stable.
The response adds `favorite_id` on each item and `has_more` on the list while
retaining all existing fields. IDs are stable only within the fixed account
context, not global identities. `has_more` explicitly identifies a limited page;
it is not a promise that the local cache contains all remote favorites.

Invalid required fields or duplicate logical identifiers now fail explicitly
instead of producing zero identifiers, silently discarded rows, or empty
success. Missing schema and unavailable sources have distinct internal errors.
Nullable optional text still projects to the original empty-string fields.

Article link extraction deliberately preserves the legacy fragment/entity rules.
The shared `adapters::wechat::legacy_text` helper is explicitly a compatibility
extractor, not strict XML parsing or authority for media resolution. It must not
be used to grant downloads or bypass a strict association check.

Tests use a pure in-memory source for the business use case and synthetic SQLite
for the real adapter. They cover typed objects, preserved provenance, unknown
kinds, literal matching, milliseconds, pagination, missing schema, duplicate
identities, and invalid required fields. Overall stage validation reports the
actual execution results separately.
