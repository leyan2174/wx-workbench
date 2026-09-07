# MCP Voice Host Security Audit

> Historical acceptance snapshot: the numbers below belong to the recorded
> source version and log. The documentation update did not rerun the audit or
> substitute a later main-project count. Source, harness and evidence remain.

Status: checked cache-commit implementation verified. Final fresh run: 20 tests,
19 passed, 0 failed, 1 ignored, exit 0 (4.46 seconds). See `FINAL-AUDIT.md` and
`final-audit.log`. No audit-owned process remains running.

## Scope

Only this directory is owned by this audit. No production files or other shared
fixtures are edited. This is host orchestration coverage, not a rerun of the WAV
publisher's standalone security suite or a speech-recognition quality test.

The fixture compiles the actual `src/cli/mcp.rs`, `src/cli/mcp_voice.rs`, and
`src/cli/asr.rs`. `build.rs` parses those files with syn and inserts one stderr
event at each of `open_config_read_lock`, `host_path`, and `BackendArgs::build`.
It removes module documentation attributes for inclusion; it does not replace
branches, validation, return values, or file operations. Unique insertion counts
are asserted. The protocol, runtime, configuration, prepared-audio codec, actual
SILK/WAV conversion, ASR clients and cache implementation are production modules
reused from `mcp-voice-host`. The original shared file guard and WAV publisher
are compiled by path in this crate because their visibility is crate-local.

Only IPC transport is substituted with a counted synthetic Response. Host tests
run the actual CLI command in a hidden child process with cleared environment,
explicit synthetic account configuration and isolated home/temp directories.
They do not start a daemon or read a real account. Prepared audio is encoded
using the production encoder and the repository's synthetic silence SILK.
The fake ASR executable verifies the entire temporary WAV against real codec
output, records calls, and returns controlled JSON, failure, or delay. No real
model is used. Explicit cloud tests only connect to an ephemeral loopback HTTP
listener with a synthetic credential and explicit upload authorization.

## Acceptance Matrix

- Unauthorized or conflicting cloud configuration: no account-open, host-path,
  backend-build or IPC event; no configured loopback connection or file output.
- Valid prepared audio with malformed `exit_code`: string, null, booleans,
  nonzero numbers, fractional numbers, arrays and objects reject before WAV
  publication or ASR execution. Integer zero remains usable.
- Corrupted prepared audio, source evidence or wrong media ID: no WAV/backend.
- A Pending bound to one RuntimeContext rejects a changed runtime identity or
  account path/configuration before publication.
- Real protocol CallContext includes escaped text and the actual request ID in
  the external response budget; decode rejection leaves no staged or final WAV.
- Local temporary, cache, model, account and runtime paths cannot overlap.
  Hard-linked model/key cache destinations and directory junctions fail closed.
- Default temporary roots are request-unique and removed on success and failure.
- Backend nonzero exit, malformed JSON and timeout do not become success or
  leave WAV/temp/cache artifacts. Remaining host deadline limits local ASR with
  and without caching.
- Checked MCP cache commits must reject oversized responses or cancelled final
  checks before new persistence. Existing cache bytes must remain unchanged for
  both hits and misses. These are the new owner-selected acceptance requirements.
- Callback stage tests prove a miss invokes five host callbacks, a hit three,
  and uncached transcription three. The fourth miss callback observes the lock
  and staged cache before persist. Four rejection reasons remain distinguishable;
  staged files and locks are removed and old cache bytes are preserved. Replacing
  the original account directory after preflight is caught by the original host
  guard before that fourth callback and before cache persistence.

## Historical Cache Contract

The first executable audit established the previous documented nontransactional
memoization behavior: final response rejection could leave a successful backend
result in the cache (1683 bytes in the synthetic oversized-text case). This was
not a newly discovered violation of the old documented contract. Historical
failed runs remain in `host-schema-corrected.log` and
`host-boundaries-verified.log`. The owner subsequently selected stronger MCP
precommit checks while preserving ordinary CLI behavior. The original red tests
are retained as acceptance tests; they are not replaced with observational green
tests. Post-publication channel loss is still not a rollback guarantee.

## Limits

The entry probes establish ordering in the real code path, not an OS-wide file
access trace. Default-temp tests establish isolation and cleanup, not a new
Windows ACL sandbox. A symbolic-link test is explicitly ignored for the required
Windows creation privilege; junction and hard-link tests run normally. Genuine
daemon/IPC account routing is independently covered by Noether's existing
`mcp-voice-runtime` account fixtures and process tests, not counted as this run.
There is no claim of actual model accuracy, internet-provider integration,
in-flight synchronous stdio cancellation, privileged-adversary protection, or
atomic rollback following successful publication.

## Run

Run from the repository root with a new log name; preserve `final-audit.log`:

```powershell
pwsh -NoProfile -File tests/fixtures/mcp-voice-host-security/run.ps1 -Log ("rerun-" + (Get-Date -Format yyyyMMdd-HHmmss-fff) + ".log")
```

The script sets the existing local libclang path, builds offline in a dedicated
target directory, preserves merged stdout/stderr in this directory, prints the
last 55 lines, and returns Cargo's exit code. Setup failures and intermediate
runs are retained and are not counted as completed acceptance results.
