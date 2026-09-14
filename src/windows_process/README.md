# External Process Lifecycle

`managed.rs` shares Windows Job ownership between daemon workers, FFmpeg and local ASR.
Children start suspended and without a console, join a kill-on-close Job, then resume.
Ordinary Jobs never allow breakaway. Incompatible host Jobs fail closed; there is no
fallback to an unowned child. Account capture alone retains its existing explicit
silent-breakaway exception for the user application that must survive capture.

`output(command, deadline, limit, cancelled)` is the one-shot helper entrypoint.
It polls both pipes fairly, stores at most the combined byte limit, checks the same
absolute deadline through spawn/output/exit, and terminates descendants even after
normal parent exit. No blocking reader thread or unbounded output/wait is used.
Cancellation and limit/deadline errors trigger actual Job termination. Explicit
cleanup has an additional two-second budget; failure means termination is not
confirmed, not that stopping the wait killed all work. Job Drop is a final
best-effort termination request, not a substitute for checked cleanup.

Direct FFmpeg: 120 seconds including preparation/publication checks, 1 MiB combined
output. Existing codec parameters and temporary same-directory publication remain
unchanged. Failure/cancellation through the API removes temporary files and leaves
an existing destination intact. Worker tasks use a 24-hour total deadline across
steps, and a 64 MiB combined output budget per step (including suppressed output).
Worker output reader shutdown is separately bounded to two seconds per pipe.
ASR retains domain-specific disk/response/stream limits and configured timeouts.

Forced host termination does not run Rust destructors: closing the Job kills owned
helpers, but temporary files created by the terminated host can remain. The task
host must handle abandoned task artifacts after confirmed worker cleanup; Job
ownership alone does not promise transactional multi-file publication or disk quotas.

Tests use synthetic processes only: hang, flood both pipes, spawn descendants,
parent exit with inherited pipes, cancellation, inner Job cleanup without killing
the outer worker, and outer Job termination including the inner tree. Audio tests
use a synthetic FFmpeg-shaped executable and verify failed publication preserves
old output. No account data, WeChat, scans, uploads or model downloads are required.
