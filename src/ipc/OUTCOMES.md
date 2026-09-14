# Business Outcomes

`Response::outcome()` / `require_success()` interpret business meaning without
changing the legacy `ok`, `error`, flattened-data response representation.
`BusinessOutcome` is internal: Success, Partial, Refused, Failure. Connection,
timeout, framing and JSON errors remain transport/protocol errors, not outcomes.

Only top-level legacy markers (`status`, `ok`, `success`, `exit_code`, `error`)
are interpreted. Nested message text is never classified. Aggregate producers
use `from_counts(succeeded, failed)` with their own domain counts. No alternate
backend, automatic retry, upload, key acquisition or recovery is triggered.

Adapters:

- CLI checks business success and prints only the typed failure's safe message.
  Public decode ambiguity code 2 is retained; internal worker codes are below.
- MCP maps partial/refused/failure to `isError: true` and fixed public text;
  business failure is not reported as backend unavailability. Completed query
  payloads and tool schemas stay unchanged.
- Web maps partial completion to HTTP 409, refusal/failure to HTTP 422, and
  transport unavailability to HTTP 503. Existing busy/host authorization mappings
  remain separate. Partial results are not returned as a successful HTTP payload.
- Worker adapters map Success/Partial/Refused/Failure to 0/20/21/1. Existing
  nonzero domain exit codes (including special first-run code 10) are unchanged
  unless a typed BusinessFailure is explicitly returned. Task terminal strings
  stay `succeeded`/`failed`/`cancelled`; a partial task is `failed`, exit 20, with
  a public message that successful artifacts remain. Cleanup failure is not
  hidden by a concurrently requested cancellation.

Key-store query initialization errors add an optional flattened `error_code`;
the existing `ok:false` and `error` string remain. The eight whitelisted codes
are `key_store_missing`, `key_store_migration_required`, `key_store_invalid`,
`key_store_wrong_account`, `key_store_protection`, `key_store_conflict`,
`key_store_busy`, and `key_store_io`. Adapters reconstruct static messages from
these codes; arbitrary backend error text and error chains are never forwarded.
Corruption, wrong account and protection failures are never classified as missing.

Specialized image adapters must preserve their existing code 1/2 distinctions
through `BusinessFailure::legacy_exit_code()` when receiving a checked query error.
ASR chat/batch count transcribed and existing results as success; batch persistence
warnings remain non-success while engine identity notices are informational.
VoiceBatch and Strict decrypt also return typed partial results. Legacy
`toolkit run decrypt` intentionally retains its zero batch exit code even with
item failures; its summary remains authoritative for that compatibility route.
A successful lookup of an already-failed task is still a
successful lookup; task-page data is not recursively reclassified.

Job termination does not imply transactional file publication. Forced termination
of the worker cannot execute its Rust destructors; its host must clean abandoned
temporary artifacts after confirming actual process cleanup.
