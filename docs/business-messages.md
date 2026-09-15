# Message Read Contracts

`business::messages` distinguishes a legacy `MessageSelector`, a verified
`MessageRef`, and an `EvidenceRef`. The selector is not an ID. Every reference
currently has `SnapshotBound` stability: another read instance rejects it, and
dropping the creating snapshot expires it. Neither `rowid`, server ID, timestamp,
nor a content hash is promoted to a permanent global identity. There is no new
database, registry, or long-lived query lease.

`adapters::wechat::messages::read::Snapshot` accepts only host-prepared sources.
Logical source names distinguish ordinary and official-push databases and cannot
encode arbitrary paths. Connections retain individual SQLite read transactions;
this does not promise a simultaneous cross-database snapshot. Contacts and Name2Id
help identify actual message tables; SessionTable never determines their inventory.

The read core owns schema inspection, Name2Id, row selection, strict uniqueness,
stored scalar types and bounded decompression. `projection` produces semantic
content and compatibility previews. Raw evidence is not a business JSON payload.
Metadata-only reads do not require body columns. Missing columns, unread bodies
and actual NULL values remain distinct. The legacy ASR server-ID accessor accepts
only a nonzero SQLite INTEGER; text, missing and NULL values are not fallback keys.
Server-ID lookup checks uniqueness across times before optional time validation.

Business pagination merges a total snapshot-local order, then applies the global
offset and limit. It deduplicates only repeated reads of the same snapshot record,
never equal local IDs, seconds or message contents. History presents the selected
page chronologically; search presents newest first. Unmapped global-search results
have null username, explicit identity status and identity-completeness metadata.
Page direction changes timestamp order only; same-second ties retain source rank
and ascending snapshot record order. History ranks sources by their latest timestamp
descending, preserving the legacy stable merge before applying offset and limit.

The compatibility read policy, including packed WeChat numeric types and the
stored-text versus decoded app-message search distinction, belongs to the adapter.
Business filters use semantic kinds. Reads have explicit limits: 20,000 sources,
100,000 streams/row candidates, 1 MiB stored body, 4 MiB decoded body and 64 MiB
stored text per selected stream scan. Exceeding a budget fails, not silently truncates
evidence. These interactive limits do not replace existing bulk-export budgets.
The candidate budget applies to rows actually scanned, not the requested page
size or offset. Requests must fit `i64`; ordinary reads use a 100,001-row lookahead
and fail above 100,000 actual candidates rather than returning truncated success.

`strict_message::with_resolved` executes synchronous
callbacks while the snapshot remains alive, then check the account inventory again.
Media callbacks return existing detached proofs, never serializable Weak handles;
publication waits until the callback wrapper succeeds and existing output/proof
checks complete. Plain content decoding receives detached evidence without a live
identity promise.

`business::sessions` owns summary selection. The session adapter has separate
full-summary, timestamp and two-column unread-publisher projections. A legacy
timestamp subscription is explicitly not a complete history cursor: same-second
truncation and the existing initial-window advancement remain compatibility limits.
Call events preserve client status and duration text, with media `Unknown`; a voice
message is not a call and status text is not evidence of audio versus video.

Verification is coordinated by the parent task. Synthetic tests cover duplicate
records, expiry, metadata-only sources, server scalar types and cross-time ambiguity,
source whitelisting, unknown call media, unmapped identities and session compatibility.
This document does not claim unrun tests have passed.

## Statistics and Explicit Export

Statistics use the validated snapshot's metadata projection, without requiring
body columns. SQLite conversion errors, unavailable sender mappings and arithmetic
overflow fail the request; they are not counted as zero. The adapter converts
WeChat types and local-time hours; business statistics aggregate semantic kinds.

The adapter owns session/contact source requests. Hosts resolve their opaque
descriptors through DbCache; callers cannot supply paths through these descriptors.
The directory catalog uses validated snapshot streams, not session summaries.

Raw export is a separate projection with nullable timestamps and original SQLite
storage types. Compact, directory and delta retain distinct compatibility formats;
they do not serialize ordinary history messages. Raw export streams rows without
the interactive 100,000-candidate cap, with a separate 64 MiB per stored/decoded
body ceiling. Publication and whole-output budgets remain with existing exporters.
Only the explicit delta profile retains its old malformed-compression missing-text
behavior, while preserving original bytes; resource-limit errors never fall back.
Its inverted time window retains the legacy empty SQL result, without skipping
source or projection checks. Other profiles retain normal time-range validation.

Attachment listing has a separate legacy conversion policy. It reports skipped
invalid rows and degraded sender text; strict metadata rejects row conversion
errors. Neither policy suppresses source, schema, SQL execution or budget errors.

Legacy transfer/location diagnostics consume only explicitly selected snapshot
streams. They return detached, non-serializable raw diagnostic records, not verified
MessageRefs. Production source keys come from the account's selected catalog;
synthetic keys are constructed only by test fixtures. Timestamp zero retains its
legacy no-time-filter meaning. All candidate diagnostics are retained within the
existing bounded read budgets; ambiguity is reported before type checking, and
type checking before lazy lossy legacy content decoding. Existing transfer/location
parsers and wire fields remain unchanged. This path does not widen an explicit
source scope or replace MCP's strict evidence-resolution policy.
