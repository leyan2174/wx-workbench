# Image15 Independent Read-Only Audit

> Historical first-stage report. See [STAGE-DELIVERY.md](STAGE-DELIVERY.md) for
> the later audit stage, then [GUARD-FIX-VERIFIED.md](GUARD-FIX-VERIFIED.md) for
> the guard-fix verification. Their counts are separate historical runs.
> The missing production interfaces noted below have since landed; that old
> compile failure is retained only as chronological evidence.

The repeat command below still targets the retained standalone audit harness;
this documentation update did not rerun it. Save new output separately from the
historical logs and source hashes.

## Finding: P2 Publication Survives A Lost Tool Response

Production references: `src/mcp/protocol.rs:791` (bounded response serialization),
`src/attachment/native_image.rs:554` (publication), and `src/cli/mcp.rs:72`
(IPC response cap inherited from the same host frame limit).

At the supported 1024-byte limit, a normal synthetic image query publishes its
decoded file before the protocol discovers that the result cannot fit. The
client receives only the initialize response, no tool receipt or output path.
Repeating the same query fails because the output already exists. This is a
delivery/recovery bug, not a source-overwrite or secret-exposure finding.

Reproduction: `response_limit_records_real_publication_before_delivery_failure`
in `audit.rs`. It executes the real SQLite query, native decoder and publication,
then returns the real result through production `Protocol::serve`. It asserts
the limit error, one output line, the exact published bytes, and retry failure.
The test passes by recording the faulty behavior; a passing audit suite does
not mean the behavior has been fixed.

Observed in `final-audit.log`:

```text
LIMIT RESULT: Err(... "MCP response exceeds limit" ...)
PUBLISHED: true
RETRY AFTER LOST RESPONSE: Err(output already exists or publication failed ...)
```

Consider a bounded publication receipt/operation identifier and explicit
published-or-unknown failure semantics, with response-budget checks before
predictable side effects. Do not solve this by overwriting existing files or
blindly deleting a published path after a transport failure.

This reproduction uses the real protocol/query boundary, not a live daemon
pipe. The real CLI transport can reject an oversized IPC response even earlier;
its precise end-to-end delivery behavior remains unverified until the main
build is complete.

## Verified Coverage

13 passed, 0 failed, 2 ignored in `final-audit.log` (exit 0):

- Same full IDs and resource hashes in two synthetic accounts return each
  account's own bytes and source path.
- Relative/traversal/ADS/device/network and attachment-tree output rejected.
- Output directory junction rejected; no external publication.
- Existing output hardlinks to DAT/resource/message/key files not overwritten;
  source bytes unchanged and no temporary output left behind.
- Duplicate exact message identity rejected before export.
- Source/decrypted directory overlap rejected in both directions.
- Empty host output rejects before synthetic cache `get` or key reads.
- Explicit key outside output accepted; key inside output, relative key,
  oversized key, malformed JSON, invalid AES/XOR fail before cache reads.
- V2 correct explicit key publishes; wrong/missing key does not publish.
- Real CLI/protocol probe verifies absent host policy reaches neither runtime
  nor IPC; client output-path injection rejected; configured host paths injected.
- Secret-bearing transport failures and malformed key errors are redacted.

Two real Windows symlink creation attempts failed with OS error 1314. These
remain explicit ignored tests, NOT passes. Directory junction coverage does
not replace file-symlink coverage. `baseline.log` retains the setup failures.

## Integration Status And Limits

`main-check.log` records a failed production `cargo check --offline`: missing
`toolkit::parse_image_aes`, `toolkit::parse_image_xor`, and
`DbCache::output_protection_paths`; also an unrelated export_chats CLI signature
conflict. This is an in-progress integration snapshot, not an assertion that
MAIN or Volta has finished. Recheck the actual final source before acceptance.

The fixture imports actual production native_image, mcp_image, strict_message,
protocol, CLI host, IPC types and image-key parser bodies. Only cache/runtime/
pipe boundaries are synthetic. Cache protection metadata is supplied by the
fixture and does NOT verify the still-missing production metadata method.
Unrelated batch/config entry points panic if called. No provider or network
implementation is part of the exercised image path; no traffic interception
or real account/daemon run is claimed.

Only this dedicated fixture was added for the audit. No production fix was
applied. Old native-attachment-security safe21 logs/tests were not replaced.
Build logs include fixture dead-code/lint-expectation warnings; no warning-free
claim is made. All command stdout/stderr is retained in the local logs.

## Repeat

```powershell
cargo test --offline --manifest-path tests/fixtures/mcp-image-security/Cargo.toml --target-dir C:/CodexLocal/build/mcp-image-security --test audit -- --nocapture
```

`source-hashes.log` records the source snapshot before the final fixture run.
Sources are concurrently being integrated; it is not a freeze of other work.
