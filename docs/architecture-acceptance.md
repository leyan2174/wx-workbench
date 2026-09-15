# Architecture Acceptance Ledger

Status: the implementation and concentrated validation gates below have been
reconciled against the current tree. Final evidence, failed attempts, targeted
reruns, caller boundaries and deliberate limitations are recorded in
[Architecture Final Validation](architecture-final-validation.md). Git delivery
is a separate final step, not implied by a passing test. The baseline review
was `9f1f705`; the pre-audit implementation commit was `5115419`.

## Requirement Gates

The following tables preserve the acceptance questions recorded before the
concentrated run. Their remaining-work columns are historical checklists,
not the current execution status. The completed reconciliation follows below.

| Requirement | Current implementation/evidence | Remaining acceptance work |
| --- | --- | --- |
| I: protect the worktree, follow AGENTS, use synthetic data | Work is on `main`; stage commits through `5115419` are recorded in phase documents. No private account, process scanning, account restart or upload is required by validation. | Review the final diff, rerun applicable checks, commit and push verified changes. |
| II-III: business objects and adapter direction across every named domain | `business` owns narrow contracts; `adapters/wechat` owns source implementations. `architecture_contracts` checks forbidden dependencies using parsed Rust, and `business_contracts` compiles actual business modules for in-memory tests. | Reconcile every domain below with production call sites and tests. Syntax checks do not establish semantic completeness. |
| IV: public protocol ownership | Service owns operation requests, MCP Call/settings and transport contracts; CLI owns clap parsing. Existing daemon execution remains the shared host. | Confirm wire/CLI compatibility and the repaired Web query handshake using actual runtime tests. |
| V: remove duplicate implementations | Contacts, moments, messages, media, favorites, articles, emoticons, planning and archive slices replace earlier production paths. Confirmed residuals are corrected in `domain-entry-residual-fixes.md` and `sns-cache-boundary.md`. | Validate the corrected production callers and compatibility, not only file relocation. |
| VI: explicit results and stable identity | Username identities remain account scoped. Message/media evidence is opaque; legacy physical fields are explicit projection material. Typed pages distinguish source completeness from possible continuation. | Verify partial/unavailable/unsupported/ambiguity semantics and all legacy projections. Complete the residual empty-success fixes. |
| VII: one protected key store | Typed master/database/image updates use current-user DPAPI, account binding, version, atomic publication and revision checks. Initialization now owns one master+derived-key update; scanner acquisition no longer persists. | Run migration, corrupt-store, account-switch, revision/invalidation and synthetic acquisition tests on the final tree. |
| VIII: shared publication and external execution | Final artifacts use guarded publication; database snapshots have an explicit cache-output exception. Single-audio staging/encoding is inside the publisher's directory lock. Existing supervised processes retain timeouts, caps and cancellation. | Run output-alias/race/failure tests and actual synthetic process lifecycle tests; verify index failure cannot advance state. |
| IX: accurate modules and names | Shared transport re-export shims were removed; service transport is real authenticated I/O. Domain adapters and format helpers moved after callers migrated. | Record retained legacy/native names and their actual compatibility scope; do not rename the entire toolkit. |
| X: preserve reasonable differences | Query, foreground operation and persistent task lifecycles remain separate. Strict association and explicit legacy fallback are not merged. Local reads do not acquire remote authority. | Verify these distinctions in final runtime tests and document limitations. |
| XI: complete vertical slices and synthetic verification | Each slice includes models, adapter, actual caller changes, old-path cleanup and tests. Phase documents retain prior successes and failures. | Execute final root and affected standalone suites after remaining development. Ignored tests are not passes. |
| XII: code, documentation and truthful delivery | Architecture overview, slice records and this ledger describe current work separately from intended acceptance. | Publish the final dependency/caller/compatibility matrix and actual test results, including any remaining limitation. |

## Domain Gates

| Domain | Authoritative implementation and test locations | Current gate |
| --- | --- | --- |
| Contacts, members, tags | `business/contacts.rs`, `adapters/wechat/contacts`, daemon contact queries; account-cache and synthetic member/label tests | Source-descriptor cleanup implemented; verify both cache spellings and failure without fallback. Retain explicitly incomplete observed membership. |
| Sessions, history, search | `business/sessions.rs`, `business/messages`, `adapters/wechat/messages`, `daemon/query/message_read.rs`; message read/source and history runtime tests | Typed page migration implemented; final continuation and legacy-field tests pending. Inventory must not depend on session summaries. |
| Structured messages, replies, calls | `business/structured_message.rs`, business message reply/call types, adapter message parsers and `reply_read.rs`; synthetic XML and real encrypted-cache reply fixtures | Reply read no longer materializes a raw host record. Verify old bounds/fields/errors and unknown call-media semantics. |
| Voice, images, attachments | Business media/voice contracts, WeChat media adapters, shared codec/publication infrastructure, thin daemon query hosts | Strict proof and voice-catalog migrations implemented; fixture wiring and final process/security validation pending. Voice messages are not call recordings. |
| Moments | `business/moments.rs`, WeChat timeline/cache adapters and existing SNS workflow | Cache-layout recovery now delegates to the adapter and a typed host writer. Missing databases return Unavailable rather than success. Verify both changes. Effective vs recorded-author compatibility remains explicit; local cache is not remote history. |
| Favorites | Business favorite listing and adapter; CLI/daemon query projection | CLI business type selection, compatibility-owned numbers, missing-result validation and stderr continuation display are implemented; verify old stdout and request fields. |
| Official articles | Business article listing over complete official-push snapshots | Revalidate unmapped-stream partial results, malformed-data failure and explicit unread selection. URL equality is not identity. |
| Emoticons | Business selection/batch outcome, adapter catalog and existing download/publication executor | Shared partial/failure outcome propagation implemented; verify continuation of batch work and retained artifacts. Legacy cache lookup must not become a claimed hash proof. |
| Full/incremental archive and plans | Business archive/chat planning, WeChat source adapters, existing document converters and shared publisher/index | Verify streaming compatibility, bound ownership and publication-before-index ordering. Index writes now carry fixed account protection too. |

## Validation Rules

Prior result details remain in `architecture-phase4-validation.md` and
`architecture-phase5-progress.md`. New compilation attempts and outstanding
work are in `architecture-final-audit-progress.md`. A previous passing test
does not prove a changed path. Conversely, unchanged suites need not be run
after each small edit: development finishes first, followed by concentrated
validation and targeted reruns for fixes.

The historical root run with two synthetic HTTP fixture failures keeps its
original nonzero exit code, even though the affected fixtures subsequently
passed targeted reruns. No future result should overwrite that history.

## Completed Reconciliation

- I: worktree changes were preserved on `main`; compilation and all executed
  tests used synthetic sources, local test services and controlled processes.
- II-III and V: every named domain is mapped to its actual business contract,
  WeChat adapter and existing host in the final validation document. The
  root architecture (3), business (43) and entry architecture (4) tests passed.
  These checks supplement, rather than replace, production caller review.
- IV and X: the real daemon/Web query-v3 test, task cross-entry tests, MCP
  authorization rejection, operation lifecycle, query generation and persistent
  history tests passed. Query, foreground and task semantics remain distinct.
- VI: typed history and voice page tests, duplicate IDs, partial sources,
  strict media ambiguity, missing SNS source, favorites projection and emoticon
  failure propagation were executed. Old physical fields remain explicit
  compatibility output, not ordinary business identity.
- VII: current root tests passed for verified acquisition without persistence,
  atomic master/database updates, corrupt-store rejection, migration, account
  binding, revision conflicts and generation invalidation.
- VIII: root and standalone output protection, source aliases, encoder-parent
  locking, timeout/cancellation and descendant-reaping tests passed. The audio
  error-context regression was fixed without weakening safety assertions.
- IX: the final caller table and slice documents distinguish removed production
  logic from retained raw/legacy projection and real transport infrastructure.
  No general framework or whole-toolkit rename was introduced.
- XI: the full root command completed with five failed test records, then the
  affected targets passed explicit reruns. The 26-fixture batch had three
  harness failures, all fixed and retested in the scopes recorded in the final
  validation document. Ignored cases are not counted as passes. Final Windows
  cargo check exited 0; warnings remain.
- XII: architecture overview, domain boundary documents, this full requirement
  ledger and final validation report now separate implemented structure,
  compatibility choices, evidence and limitations. Commit/push verification
  remains an external Git delivery action.

Expected deliberate limitations remain Windows x64/current-user protection,
explicitly marked legacy-unverified material, local-source completeness,
bounded idempotency retention, and `interrupted` rather than automatic resume
after persistent-task restart. Multi-artifact publication is not a filesystem
transaction, and cancellation does not roll back artifacts already committed.
These limitations do not excuse an unhandled implementation requirement.
