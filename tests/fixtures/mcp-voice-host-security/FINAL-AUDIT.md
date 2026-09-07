# Voice Host Security: Final Verification

> Historical final run for this audit's source snapshot, not a current whole-repo
> result. The documentation cleanup did not rerun tests or alter the figures.
> Current rerun instructions in [README.md](README.md) use a separate log name.

## Result

The owner-selected checked MCP cache boundary passes this independent host
audit. No unresolved failure remains in the executed cases. The symbolic-link
case remains a coverage gap, not a passing security claim.

- Fresh complete run: **19 passed, 0 failed, 1 ignored**, 20 tests total.
- Exit code: **0**. Test duration: **4.46 seconds**.
- Full merged stdout/stderr: [final-audit.log](final-audit.log).
- Source snapshot: [verified-source-hashes.json](verified-source-hashes.json).
- Only this fixture directory was edited during the voice-host audit. Production
  fixes are the owner's work. No audit-owned test process remains active.

## Checked Cache Boundary

Both retained red tests now pass without weakening their no-new-cache assertions:
`response_budget_rejection_must_precede_transcript_cache_commit` and
`final_commit_denial_must_precede_transcript_cache_commit`.

The real CLI and protocol reject oversized escaped transcription text or a
large JSON-RPC request ID using the actual CallContext budget. Cache-hit and
cache-miss rejection leave an existing cache byte-identical. A cancelled new
result also leaves existing bytes unchanged.

Callback execution was independently measured:

| Case | Host callback count | Verified state |
| --- | ---: | --- |
| Cache miss | 5 | Before audio, after backend setup, transcription preflight, cache pre-persist, final return |
| Cache hit | 3 | No cache mutation; final text budget still enforced |
| Uncached transcription | 3 | No cache publication path |

On a miss, callback 3 observes no cache artifacts; callback 4 observes the lock
and staged cache but no new published cache; callback 5 observes the committed
cache and no staging artifacts. Rejecting callback 4 with each of `Cancelled`,
`TimedOut`, `ResultLimit`, and `Unavailable` returns the exact reason, rather
than swallowing it as a degradable cache-write failure. This was tested with
both absent and existing cache files. All lock/temp artifacts are cleaned.

The original account database directory was renamed and recreated at its former
path during callback 3. The next real store callback rechecks the original
host guard, detects the changed file identity and returns `Unavailable` before
calling user callback 4. No new cache is committed; an existing cache remains
byte-identical. This tests the second check rather than merely initial preflight.

## Other Host Boundaries

- Real CLI authorization rejection precedes all instrumented account-open,
  host-path, backend-build and IPC entries. The configured loopback listener
  receives no connection. Positive cases reach those entry points.
- A valid prepared payload plus malformed `exit_code` (including string, null,
  booleans, floating zero, nonzero/fractional/out-of-range integers, arrays and
  objects) returns QueryFailed with no WAV and no backend execution. Integer
  zero is accepted. Corrupted payloads, evidence and wrong media IDs fail closed.
- Changes to bound runtime/account identity or paths are rejected. Genuine
  daemon A/B routing is Noether's separate process coverage, not included here.
- Account/model/temp/cache overlaps, hard-linked cache targets and directory
  junctions are rejected without overwriting protected bytes or launching ASR.
- Default temporary roots are unique per request and removed after success,
  backend failure and malformed output. Explicit-temp failure and timeout paths
  also leave no WAV/temp/cache residue and never report success.
- The local backend uses remaining host time, with and without cache. The initial
  300ms remaining-window test was too short to guarantee the synthetic process
  had reached its call marker; the final test uses a 2200ms total deadline,
  500ms simulated IPC wait and a 10-second backend setting. It asserts TimedOut,
  actual backend entry, bounded elapsed time and no cache/temp residue.
- Explicitly authorized cloud execution is tested only against loopback HTTP,
  with synthetic credentials: HTTP 200 succeeds, HTTP 401 fails safely. The real
  WAV bytes are sent; no real account, model or internet provider is used.

## Evidence And Limits

The old memoization behavior was already documented, not a newly discovered
violation of its former contract. Historical red logs remain intact. After the
owner selected the stronger MCP boundary, the original red tests were retained
and turned green through production changes, not observational assertions.
Ordinary CLI compatibility is covered by the owner's separate regression; this
fixture does not claim to independently rerun the full CLI or application suite.

The ignored symbolic-link test requires Windows symbolic-link creation privilege;
it was not executed in this final run. Junction/hard-link tests did execute.
Default-temp coverage proves request isolation and cleanup, not a new ACL sandbox.
Entry probes are AST-inserted events in actual production code, not an OS-wide
file access monitor. Their exact instrumentation and substitutions are described
in [README.md](README.md). The final build reports four dead-code warnings for
image-only helpers in the shared file-guard module; these are fixture-specific,
not a claim about main's warning count.

No test asserts rollback after a successful persist followed by channel loss.
There is no claim of actual model accuracy, in-flight synchronous stdio
cancellation, privilege-resistant atomicity, or complete product acceptance.
