# Final Architecture Audit: Work in Progress

This record follows the phase-five baseline `5115419`. It is not a claim that
the entire architecture objective has passed acceptance. Existing successful
test results remain recorded in `architecture-phase5-progress.md`; new changes
below require their own verification.

## Compilation Record

- `wx-cli-audit-publication-check-1.log`: production compile succeeded after
  the initial image/audio publication changes.
- `wx-cli-audit-publication-check-2.log`: test compilation failed while the
  voice-catalog migration was in progress (old fixture names/module wiring).
  No tests executed; this failure is retained, not reported as a pass.
- `wx-cli-audit-integration-check-3.log`: `cargo check --tests --offline
  --locked --target x86_64-pc-windows-msvc` succeeded after the voice fixture
  migration, host-owned key persistence and checked output/index changes.
- `wx-cli-audit-integration-check-4.log`: test compilation failed on one
  reference to the removed reply host's constant. The test now declares the
  unchanged public reply limit independently; its failure assertions remain.
- `wx-cli-audit-integration-check-5.log`: production Windows-target compilation
  succeeded after strict media/reply migration and moving audio staging under
  the shared directory lock. Test execution is still pending.
- `wx-cli-audit-final-check.log`: production Windows-target compilation
  succeeded with all confirmed residual implementations frozen.
- `wx-cli-audit-final-root-tests-1.log`: test compilation failed because the
  existing SNS cache tests relied on a parent-module `BTreeMap` import that
  moved with the implementation. The test now imports it explicitly; no
  production behavior or assertions changed. No test cases ran in this attempt.
- `wx-cli-audit-final-root-tests-2.log`: concentrated root execution started
  after that import correction. Result is not yet known at this record.

All logs are in `C:/CodexLocal/`. Later edits need appropriate verification.
Compilation-only attempts do not execute cases; the final root run is tracked
separately and must reach a terminal result before it can count as evidence.

## Source Inventory

Configured ordinary/official-message candidate classification now belongs to
`adapters/wechat/messages/inventory.rs`. The daemon passes original cache keys
and a business source kind, not key material. Original sorting and permissive
candidate discovery are preserved: a malformed candidate must reach strict
adapter validation rather than disappear from an apparently complete result.
Session prewarming uses the adapter's source descriptor.

The targeted source check and inventory/query-state/runtime tests completed
successfully before this record. Logs are outside the repository:
`C:/CodexLocal/wx-cli-audit-source-check.log`,
`C:/CodexLocal/wx-cli-audit-adapters-wechat-messages-inventory-tests.log`,
`C:/CodexLocal/wx-cli-audit-daemon-query_state-tests.log`, and
`C:/CodexLocal/wx-cli-audit-query-runtime.log`.

## Local Audio Publication

The production `ToolkitOperation::VoiceToMp3` path now obtains a short-lived
`application::publication_context::PublicationContext`, supplies its protected account/input paths to the existing
`ExportTarget`, and revalidates that context before final publication. The
destination fingerprint is captured before SILK decoding or encoder startup.
The source is pinned and revalidated as before.

Missing configuration remains an explicit offline case, observed from disk;
it is not a model-supplied flag. An existing invalid configuration is rejected.
The shared context owns this distinction for image and audio publication.

The encoder still uses the existing supervised child process: 120-second
deadline, 1 MiB process-output limit, and 64 MiB staged MP3 limit. Foreground
worker cancellation/disconnection continues to be owned by its Windows Job.
This change does not turn synchronous MCP voice into a persistent task.

A subsequent review found that final-output protection alone did not protect
earlier plaintext staging against a parent-directory replacement. PCM staging
and encoding now run inside the shared publisher's callback, under its existing
directory lock. PCM permissions are restricted before writing; the encoder
writes the publisher-owned MP3 temporary. A synthetic encoder test attempts to
rename the staging directory and verifies that publication succeeds only with
that attempt rejected and no temporary files left behind. It awaits execution.

Internal codec wrappers remain useful for already-protected temporary files
and codec tests. Production final publication must use the checked host path;
batch audio already stages conversion before its own checked final publisher.

New tests cover rejection of a protected destination before decoding and host
revalidation after a successful synthetic encoder process. They have been
written but have not yet been run as of this record.

The explicit offline SNS-video command uses the same export context and a
checked final callback. Its existing create-only policy is unchanged. New
synthetic tests cover future paths within account directories and revoked host
binding before publication; no remote video or key acquisition is involved.

Database snapshots now pass their protection list to `ExportTarget` and verify
the pinned source, configuration, and absence of target sidecars in the final
callback. The shared protection policy has one explicit distinction: database
decryption may publish into its configured decrypted cache; ordinary exports
must protect that cache too. Account inputs, configuration, encrypted/legacy
key paths and runtime files remain protected in both policies. Main-file-only
snapshot semantics (no live WAL merge) and strict/legacy batch outcomes remain
unchanged. The cache-exception test is written and awaiting execution.

The chat archive index now receives the fixed runtime's protected paths too,
including before lock/index creation, and publishes through `ExportTarget`.
Index persistence still precedes in-memory index advancement and follows the
chat artifact publication. The old unprotected `atomic_output` wrapper has no
production callers and is retained under `cfg(test)` only for publisher tests.

## Key Acquisition Boundary

Account-key acquisition no longer receives a store or writes files. It returns
the verified master material and derived database entries to initialization.
The host verifies the source/configuration guards, then submits both material
types in one store update against the observed revision. A concurrent key
update fails explicitly; failed derived-key publication cannot leave a newly
captured master key independently committed. Auto-provider reuse still verifies
the supplied master material against current database pages before returning
entries, and storage corruption is rejected by the host before acquisition.
Synthetic tests for this boundary and one-revision atomic storage are written
but not yet run. No key-acquisition algorithm or authorization was changed.

## Outstanding Acceptance Work

- Verify the completed legacy image-operation publication/context migration.
- Verify the Web query protocol correction against a real synthetic daemon;
  see `web-query-v3-fix.md`.
- Verify the completed ordinary history/search typed-page and legacy wire
  projection migration and explicit page-continuation semantics.
- Verify the completed ordinary voice catalog business result and legacy wire
  projection migration and explicit page-continuation semantics.
- Verify strict MCP image/attachment and reply read migrations; existing strict
  matching must not become heuristic and legacy fields/errors must be preserved.
- Verify the completed residuals from `phase5-domain-audit-snapshot.md`: SNS
  cache/recovery and missing-source outcomes, contact source descriptors,
  favorite type/continuation projection, and emoticon partial-result propagation.
  See `sns-cache-boundary.md` and `domain-entry-residual-fixes.md` for actual
  callers and compatibility changes.
- Consolidate the full requirement evidence in `architecture-acceptance.md`,
  and run the final compatibility validation.

## Concentrated Validation Results

The confirmed implementation residuals were frozen before concentrated testing.
Development uses necessary compile checks, not a full test run on every turn.
After the concentrated run, only affected tests are repeated for fixes.

- `C:/CodexLocal/wx-cli-audit-final-root-tests-2.log`: the full root command
  completed with exit 101 across 25 targets: 2839 passed test records, 5 failed,
  23 ignored. Repeated production modules in fixture targets are counted as
  separate test records, not unique cases.
- Four failed targets shared the same encoder error-context regression. The
  publisher still holds its directory lock throughout encoding; encoding errors
  now retain their original top-level message, while publication errors retain
  publication context. Existing failure and secret-redaction assertions remain.
- The remaining failure expected the old emoticon all-failed success exit.
  The runtime test now requires exit 1, retains the legacy count-text assertion,
  and still checks that no failed artifact is left behind. This follows the
  documented business failure contract, not a relaxed assertion.
- `C:/CodexLocal/wx-cli-audit-failure-fix-check.log`: Windows target cargo check
  exited 0, with existing warnings.
- `C:/CodexLocal/wx-cli-audit-audio-fix-tests.log`: affected `process_tests` in
  `wx`, `asr_video_security`, `mcp_runtime`, and `mcp_security` passed; exit 0.
- `C:/CodexLocal/wx-cli-audit-emoticons-fix-tests.log`: all 6 emoticon runtime
  tests passed; exit 0.

These targeted passes resolve the observed root-run failures; they do not
change the original full command's exit code.

The subsequent 26-fixture batch completed: 23 passed initially; voice-security
and standalone local ASR needed real module/dependency wiring, and the video
security CLI cases needed the explicitly built executable path. After these
harness-only corrections, voice-security passed 102 cases, local ASR passed
24 with one ignored helper, and the two affected CLI security cases passed.
Final Windows cargo check exited 0 in
`C:/CodexLocal/wx-cli-audit-delivery-check.log`. Complete per-fixture results,
failure logs, requirement reconciliation and limitations are in
`architecture-final-validation.md` and `architecture-acceptance.md`.

Earlier outstanding-work lists in this file record development-time gates;
implementation and test reconciliation are now complete. Git delivery is
verified separately through the commit and remote ref, not inferred from tests.
No ignored or unrun tests are reported as passing.
No real WeChat process, account data, keys, cloud upload, or private media was
used for this work.
