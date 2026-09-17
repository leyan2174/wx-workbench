# Moments Contracts

`business::moments` defines account-scoped moments, author provenance, opaque
evidence/media references and narrow timeline/interaction sources. Its use cases
own author selection, inclusive time endpoints, stable descending time order,
result limits and local-cache coverage. They do not parse storage formats or
authorize files, network activity, decryption, or account selection.

`adapters::wechat::moments` owns SNS database location, capability checks, SQL,
value decoding, XML and compatibility projections. Hosts continue to use the
existing fixed-account `DbCache` and blocking execution lifecycle.

## Compatibility Policies

| Entry | Author selection | Content | Search |
| --- | --- | --- | --- |
| Feed/search | Recorded identity wins; embedded identity only fills absence | DOM with historical text-only recovery | Literal encoded content/body matching, not wildcard syntax |
| Legacy database export | Explicit `RecordedCompatibility`, including grouping | Existing bounded strict XML/binary/base64 decoder | No implicit search |
| Notifications | Recorded original author wins; embedded fallback | Existing short original preview | Inclusive timestamp and unread filtering |

Conflicting recorded and embedded authors are retained as an explicit conflict;
they never switch ownership. Selected malformed export records become visible
unreadable evidence, while unselected authors remain filtered. Missing original
posts in interactions are distinct from present posts with empty previews.
Unsupported schema and invalid row data fail rather than return a successful
empty source. Duplicate source identities are rejected rather than overwritten.

All pages describe **local cache only**, never complete remote history. Scan
truncation, unreadable records, author conflicts and remaining matches are
separate fields. Existing protocol JSON is projected at the host boundary and
retains legacy field names and endpoint behavior.

The `legacy` and `query_xml` submodules are explicitly format/projection code,
not business models. They preserve the two established output representations;
toolkit compatibility modules re-export them rather than interpret XML again.
Media references expose identity/evidence only. Existing strict cache matching,
explicit download authorization, key-store reads and atomic publication remain
unchanged; recovered text never fabricates media associations.

## Verification

Pure in-memory business tests cover endpoint inclusion, ordering, recorded-author
selection, conflicts, pagination and incomplete scans. Adapter tests use only
synthetic in-memory SQLite/XML and cover schema failure, author fallback/conflict,
strict versus recovered content, literal search, media evidence, unread filters
and missing original posts. Query XML/media regression tests are registered in
`adapters/wechat/moments/query_xml_tests.rs`; existing SNS export goldens continue
to exercise compatibility projections. Execution requirements are in the
[test guide](../../tests/README.md).

## Query Completeness Projection

Feed and search responses retain their existing fields and add `meta` with
`coverage: "local_cache_only"`, `scanned`, `scan_truncated`, `has_more`,
`unreadable`, and `author_conflicts`. The last two are counts, not private source
locations. `has_more` records observed extra matches; `scan_truncated` separately
records an incomplete scan, so a false `has_more` is not a completeness claim.

Display-name author selection uses the shared contact resolver and rejects
ambiguity instead of choosing by map iteration or shortest display name. Explicit
account-scoped usernames retain their existing direct-filter behavior, including
authors not present in the local contact directory.
