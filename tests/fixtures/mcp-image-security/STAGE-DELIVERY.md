# Image15 Audit Stage Delivery

> Superseded by [GUARD-FIX-VERIFIED.md](GUARD-FIX-VERIFIED.md): the original directory-swap red test is
> now green after the owner's production fix. Historical evidence below remains.

## Status At Stage Delivery

No audit command/session remains running. No production fix was applied.
Tests, probes and logs are under this dedicated directory. Old safe21 evidence
was retained. MAIN's reported 830 passed / 9 ignored is separate from this audit;
this auditor did not independently rerun or claim that full-suite result.

## P2: Protected Directory Can Become The Output During Await

References: src/attachment/native_image.rs:30 and :41 (HostOutputGuard),
src/daemon/query/mcp_image.rs (guard creation before awaiting q_decode_image).

The held output-directory handle does not prevent rename on this Windows host.
After initial protection checks, the test pauses at the cache get boundary,
renames the original output directory away, then renames a protected directory
to the configured output pathname. A same-file handle confirms that this is
the original protected directory, not a new directory with the same name.
The actual wrapper resumes and reports status=published, writing the image
into that formerly protected directory. The protected sentinel remains intact;
this is a write-boundary bypass, not an overwrite finding.

Reproducer: guard_directory_swap_must_not_publish_into_previously_protected_root.
Evidence: guard-swap.log. This remains a genuine failing security assertion.
It uses actual wrapper/guard/native publication, with synthetic cache metadata
and a deterministic pause. No production cache or native implementation is copied.

The publication path needs to retain and verify the originally approved output
and protected-directory identities before publication, or use directory locks
whose rename exclusion is actually demonstrated on Windows. Reopening a new
guard from the same pathname alone accepts the replacement directory.
Concurrent local directory mutation is the prerequisite; no remote path
injection or privilege escalation is claimed.

## P2: Publication Can Succeed Without A Deliverable Receipt

MCP layer: response_limit_records_real_publication_before_delivery_failure
executes a real query/publication and production Protocol::serve. With 1024 bytes,
only initialize is delivered; tool serialization fails, file remains, retry fails.

IPC layer: actual_ipc_reader_failure_after_publication_has_no_receipt_or_rollback
publishes a real image with original/high/thumbnail candidates. The real result
serializes to 1264 bytes. Production read_response at limit=1024 returns an
oversize error after publication; the file remains and repeat export fails.
The private reader is extracted from production via a Rust AST at build time,
without altering its body. No live named-pipe daemon run is claimed.

Evidence: final-audit.log and ipc-post-publication-verified.log. These are
observational tests that pass by proving the failure semantics, not fixes.
References: src/cli/transport.rs:354 and src/mcp/protocol.rs::Protocol::serve.

## Normal Cache Export Is Usable

real_cache_cold_warm_and_redecrypt_first_exports_remain_usable uses actual
DbCache, full_decrypt, query and publication with synthetic encrypted SQLite
pages. Cold, warm and full-redecrypt paths all publish the expected image.
Cold creates _mtimes.json; redecrypt overwrites stale cache bytes; both message
and resource entries persist current source timestamps. Encrypted sources stay
unchanged. Evidence: lifecycle-real-cache-verified.log.

HostOutputGuard protects ordinary files through their parents, not immutable
file-read handles. Only explicit image key files receive pinned read handles.
The tested guard does not block normal first export or mtime/cache updates.
WAL-only cache refresh was not separately exercised in this audit stage.

## JSON And Key Lifecycle

The matrix passes: wrong root types, unknown/legacy field names, duplicate fields
(including null then value), invalid AES/XOR types and values, trailing JSON,
BOM, non-ASCII AES, and oversized files reject without cache reads or secret echo.
Empty object and null options retain defaults; exactly 4096 bytes including
whitespace is accepted; 4097 rejects. The shared AES parser accepts at least
16 ASCII bytes and uses the first 16, rather than enforcing exact length.

Across query await, key writes through original/hardlink paths are blocked;
original key deletion and key-parent rename are blocked. Unlinking a different
hardlink alias is permitted but does not change the pinned original or bytes.
Success, decode failure and cancellation before native dispatch release handles.
Evidence: guard-lifecycle.log. Cancellation after spawn_blocking starts and
memory-forensic zeroization were not tested. Output-directory rename is NOT
blocked, as captured in the separate red finding above.

## Test Accounting

The fixture now defines 20 tests. Latest targeted runs confirm the IPC reader
and key lifecycle tests pass, and directory swap fails. The preceding 19-test
run recorded 15 passed, 2 failed, 2 ignored; its two failures were subsequently
resolved as test assumptions and rerun individually. Do not present these as
one fresh all-green aggregate run. No unrelated tests were started after the
stage-delivery request. Fixture-only dead-code/lint warnings remain.

Two Windows file/directory symlink setup attempts failed with OS error 1314;
they remain explicitly ignored. Actual directory-junction coverage passed.
Earlier cold-cache setup failure was VACUUM renumbering a synthetic mapping
rowid, corrected before the successful real-cache run; not a production defect.

All complete stdout/stderr and historical failures remain in the named logs.
