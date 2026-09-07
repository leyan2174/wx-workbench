# MCP Voice Host Regression

> Documentation check, 2026-09-07: production host and receipt wiring already
> exist. Counts and pending items below describe the recorded delivery stage,
> not the current full-suite status. This update did not run tests.

This fixture links production host, codec, publisher, ASR, cache, path guards,
runtime and protocol modules. The local probe is a controlled executable, not a
speech-recognition model. Tests do not measure recognition accuracy.

## Timeout And Cache Compatibility

The host clamps backend execution time to the call's remaining budget after IPC,
credential loading and pre-execution checks. Local execution uses the smaller
timeout; explicit cloud execution uses the existing client's tightening API.
No fallback backend is selected.

Local successful-transcription cache identity no longer includes execution
timeout. Different remaining budgets can reuse the same successful result.
This changes the configuration digest: older entries whose digest included
timeout will not immediately match new requests. They are retained, not deleted.
Other recognition identity fields remain unchanged.

The host uses transcribe_cached_with_receipt_checked: preflight validates the complete text,
original guards, context and account. store_success_checked repeats this callback
after staging and snapshot checks, immediately before persist. Host rejection
prevents this cache publication; ordinary cache I/O failure remains recoverable.
Publication is not a transaction with MCP delivery: later channel failure cannot
roll back a committed result. WAV checks the exact response before committing.

The newer receipt fast path checks exact username/media ID, bound account, backend
identity and record digest before voice IPC. A valid hit can survive source removal
and a stopped daemon; display-name resolution may still require the daemon. Lookup
is read-only and does not authenticate current source existence. The separate host
process log reports 7 passing tests with 2 unused warnings; full regression and image
metadata output verification for those changes were pending at that delivery.

## Run

From the repository root, the retained standalone harness is:

```powershell
$env:LIBCLANG_PATH = 'C:/CodexLocal/build-tools/libclang/clang/native'
cargo test --offline --manifest-path tests/fixtures/mcp-voice-host/Cargo.toml -- --nocapture
```

Save new output separately. Receipt core entry points and the retired orphan
target are documented in [voice-cache-receipt](../voice-cache-receipt/README.md);
neither that cleanup nor this command implies a new passing result.
