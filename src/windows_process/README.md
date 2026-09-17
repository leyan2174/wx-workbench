# External Process Lifecycle

`managed.rs` shares Windows Job ownership between daemon workers and image/video helpers.
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

Image/video helpers apply their caller's execution and output limits.
Worker tasks use a 24-hour total deadline across
steps, and a 64 MiB combined output budget per step (including suppressed output).
Worker output reader shutdown is separately bounded to two seconds per pipe.

Forced host termination does not run Rust destructors: closing the Job kills owned
helpers, but temporary files created by the terminated host can remain. The task
host must handle abandoned task artifacts after confirmed worker cleanup; Job
ownership alone does not promise transactional multi-file publication or disk quotas.

Tests use synthetic processes only: hang, flood both pipes, spawn descendants,
parent exit with inherited pipes, cancellation, inner Job cleanup without killing
the outer worker, and outer Job termination including the inner tree.
No account data, WeChat or scans are required.
