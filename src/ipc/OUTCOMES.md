# Business Outcomes

`Response::outcome()` / `require_success()` interpret business meaning without
changing the legacy `ok`, `error`, flattened-data response representation.
`BusinessOutcome` is internal: Success, Partial, Refused, Failure. Connection,
timeout, framing and JSON errors remain transport/protocol errors, not outcomes.

Query response exhaustion is `QueryLimitExceeded::ResponseLimitExceeded`, with
`code: "response_limit_exceeded"`, the request's protocol operation name and
`response_limit_bytes` (the minimum of the requested and server-supported limits).
The existing account-bound query-v3 `Oversize` reply is unchanged. Only an actual
response encode/read overflow has this meaning; request size, malformed JSON,
identity mismatch and other transport failures are not relabeled.

History's adapter read budget can fail before response serialization, for example
at the existing 100,000 candidate bound. This adds `error_code:
"query_read_limit_exceeded"` to the legacy failure response; the client reports
that separate code and operation, without claiming a response byte count.
Malformed history parameters remain `InvalidData` with the existing validation
order and error text; zero limits, excessive type counts and integer overflow
are not relabeled as execution-budget exhaustion.
Both diagnostics contain no chat text, username, database path or underlying
error chain. CLI query commands with `--json` write one JSON diagnostic to stderr
and exit 1; history JSON also suppresses the existing daemon startup notice.
Other commands retain their existing startup notices. Text mode includes
`--offset` / smaller `--limit` advice. No safe page
size is inferred from message count, and no retry or larger-budget fallback runs.

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
through `BusinessFailure::worker_exit_code()` when receiving a checked query error.
Decrypt returns typed partial results. Decrypt item failures
are non-success: mixed results return the partial outcome code and total failure
returns the failure code, while the report remains available to callers.
A successful lookup of an already-failed task is still a
successful lookup; task-page data is not recursively reclassified.

Job termination does not imply transactional file publication. Forced termination
of the worker cannot execute its Rust destructors; its host must clean abandoned
temporary artifacts after confirming actual process cleanup.
