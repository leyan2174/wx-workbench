# G1 Runtime Fixture

No production account, download, upload, mock-success dispatcher, or external wx executable is used.

## Running

Run from the wx-workbench checkout. The executable comes exclusively from
`env!("CARGO_BIN_EXE_wx")`; do not substitute a PATH executable or another checkout's build.

```powershell
$env:CARGO_TARGET_DIR = 'C:\CodexLocal\wx-workbench-target-20260916'
cargo test --target x86_64-pc-windows-msvc --test g1_query_runtime
```

The ignored browser case requires a separate explicit run:

```powershell
$env:CARGO_TARGET_DIR = 'C:\CodexLocal\wx-workbench-target-20260916'
$env:WX_G1_UI_FIXTURE_INFO = 'C:\CodexLocal\wx-g1-ui.json'
cargo test --target x86_64-pc-windows-msvc --test g1_query_runtime g1_ui_fixture_browsermanual -- --ignored --exact --nocapture
```

Wait for the ready message, read the external JSON, and open its `url` in the actual browser.
The URL contains the synthetic Web token as a fragment. JSON also provides `origin`, `token`,
`stop_file`, Web `pid`, and the 600-second maximum wait. No real-account secret is present.
Use the exact `stop_file` from this run to end inspection:

```powershell
$fixture = Get-Content -Raw -LiteralPath 'C:\CodexLocal\wx-g1-ui.json' | ConvertFrom-Json
Set-Content -LiteralPath $fixture.stop_file -Value 'stop' -NoNewline
```

The parent test must remain running during browser inspection. It removes its rendezvous
and stop marker, kills/reaps Web, stops/reaps daemon, and deletes its temporary runtime.
Timeout fails the test after cleanup. Assertion unwinding also drops process guards.
Existing rendezvous or stop files are rejected, never silently overwritten; investigate
their owning run before removing stale files. Forced termination of the test runner or
machine cannot execute Rust destructors and is outside the cleanup guarantee.

## Coverage

- Real encrypted synthetic SQLite -> fixed-runtime daemon IPC -> CLI JSON, MCP stdio,
  authenticated random-loopback-port Web GET. Accounts A and B run concurrently, then A
  is queried again to catch cross-account contamination.
- Sixteen query families: tags, tag members, group members, history, search, stats,
  voice metadata, unread summaries, favorites, business articles, SNS feed, SNS search,
  SNS notifications, refer, file-message, and record-item details.
- Nonempty independent witnesses; cross-shard history type union, oldest-first offset,
  inclusive time bounds; search type/time filtering; aggregate stats; voice page identity;
  favorite has_more; article partial/issues; SNS cache coverage and no mark-read effects.
- CLI array-only projections are explicit. MCP and HTTP must preserve complete business
  objects, including metadata. Only Web's validated image action descriptor is removed
  from history for comparison. Real warm-up queries stabilize cache-mode metadata.
- Bad arguments, conflicting history filters, unsupported/duplicate HTTP keys, missing
  decode timestamp, ambiguous names/tags/message identities, and missing favorite DB
  must fail rather than masquerade as empty successful data.
- Source DBs, local synthetic attachment files, config, legacy anchor, and current-format
  DPAPI store are byte-snapshotted. The legacy anchor is `{}`; all usable keys are DPAPI.

Not covered by the automatic tests: browser interaction, transfer/location decoding,
image decoding/media downloads, ASR, observed-sender membership fallback, large-response
budgets/cancellation, SNS scan thresholds, or every pagination/filter combination.
The read-only shared fixture modules are imported, not edited.
