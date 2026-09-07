# Windows account-key provider

Windows x64 builds support optional account-key capture based on
LOGO127/wechat-ai-memory. Rust manages the process lifecycle, key derivation,
SQLCipher verification and storage. Frida runs the embedded
`src/scanner/windows/account_hook.js` inside the target process. Python and
Node.js are not required for this provider. Frida Hook JavaScript is not a Node
service. Personal media workflows now have native Rust entry points; optional
Python Whisper/PyTorch inference and the npm Node launcher remain separate
compatibility choices, not requirements of this key provider.

## Current Status (2026-09-07)

| Area | Current behavior and evidence boundary |
| --- | --- |
| `--key-provider auto` (default) | Tries the saved DPAPI account key, then the version-selected read-only memory scanner if reuse is unavailable or fails verification. It never implicitly restarts WeChat or captures with Frida. |
| `--key-provider memory` | Bypasses the DPAPI record and uses the read-only scanner. Its `< 4.1.10` / `>= 4.1.10` split chooses raw-key / Config.Cipher scanning, not account-key capture. |
| `--key-provider account` | Requires `--force --restart-wechat`; this path restarts WeChat and injects the embedded Frida script. It is not the non-injecting memory-scanner path. |
| Current automated snapshot | Main reports Rust `1325 passed / 0 failed / 11 ignored`, personal Web `61/0`, and 8 separately passing optional test executions. MSVC check passes with 9 warnings. These totals are not this provider's independent test count. |
| Validation scope | This documentation pass only inspects source and existing records. No real-account capture, model/GPU, live cloud, or installed-release validation was performed. Enterprise WeChat is excluded from the target; retained implementations and entry points are not removed. |

Source of truth: [provider dispatch](../src/scanner/mod.rs),
[init](../src/cli/init.rs), [migration ledger](rust-migration.md), and
[architecture](architecture.md). The [4.1.12.26 record](windows-wechat-4.1-key-provider-verification.md)
is a historical memory-scanner observation, not evidence that the current
account Hook works with every WeChat build.

Main also completed the documentation-sync validation:
`C:/CodexLocal/wx-cli-doc-sync-tests.log` terminated with exit 0, 20 suites and
`1325/0/11`; check exited 0 with 9 warnings, and 18 EXE help checks passed.
This was run by main, not this file's documentation maintainer. Help checks do
not validate real capture behavior or an installed deployment.

## Capture

Select the intended account configuration explicitly. These are usage examples,
not commands executed during this documentation update:

```powershell
$env:WX_CLI_CONFIG = '<account workspace>\config.json'
$env:WX_CLI_HOME = '<account workspace>\.wx-cli-runtime'
wx init --force --db-dir '<account>\db_storage' --key-provider account --restart-wechat
```

This first rejects Weixin.exe processes from a different installation, then
closes the matching running processes and starts WeChat again. Complete
QR/mobile login to the selected account within the default 300
seconds. There is no automated clicking of login controls. Optional arguments:

```text
--wechat-exe <path to Weixin.exe>
--capture-timeout <10..1800 seconds>
```

`--restart-wechat` requires `--force` and `--key-provider account`. The default
provider never implicitly invokes this restart/capture path.
`--wechat-exe` is also restricted to the account provider. `--capture-timeout`
defaults to 300 seconds and accepts 10 through 1800 inclusive.

`WX_CLI_CONFIG` takes precedence over ordinary config discovery. `--db-dir`
selects the database directory, not the configuration file. `--force` requests
re-initialization; it does not authorize a cross-account overwrite or discard
unknown configuration fields. Without `--force`, an existing selected config
and its resolved key file can cause initialization to return without scanning.
For a path/backend-only inspection, use `wx toolkit setup --check`; that check
does not capture keys or run an inference model.

## Derivation and verification

The hook locates a SHA-512 implementation using constants and instruction
references, then identifies HMAC input blocks with the 32-byte key and ipad
layout. Candidates travel as binary Frida messages and are not logged.
PBKDF2-HMAC-SHA512 with 256000 iterations derives a database key from the
candidate and that database's 16-byte salt. Page-one HMAC verification checks
the candidate against `message/message_0.db`. All target databases are then
individually derived and verified, including WAL page-one variants.

The account path excludes top-level `migrate` and requires coverage of every
collected target database before saving the account key. Its `collect_db_salts`
and `collect_db_pages` traversal is not the memory scanner's `CheckedInventory`;
do not assume identical path pinning and inventory limits for both providers.

## Storage and reuse

After complete verification, `account_key.dpapi` is saved alongside the active
`config.json`. It contains a versioned record protected by Windows DPAPI for
the current Windows user and bound to the canonical database directory.
Replacing it preserves the prior encrypted file as a timestamped `.bak`.
The file, temporary files and backups are ignored by Git. Do not distribute
them or assume they can be decrypted under another Windows user or machine.
The daemon uses the per-database JSON file resolved from the selected config's
`keys_file` (default `all_keys.json`). Relative paths are resolved against that
config's directory. Unlike the DPAPI account record, the JSON contains raw
per-database key material and must be treated as a secret.

`init` rejects malformed/non-object JSON, unsafe/conflicting output paths, and
detected concurrent changes before overwriting selected files. It preserves
unknown config fields and writes the resolved custom `keys_file`, not a second
hard-coded `all_keys.json`. Keys and configuration are published atomically
**per file**, in that order; this is not a multi-file transaction. If config
publication fails after keys are saved, the command reports failure and leaves
the saved keys available for inspection/retry. Account capture has its own
earlier DPAPI save, also not part of a cross-file transaction.

```powershell
# Reuse the saved account key and derive newly added database shards.
wx init --force --db-dir '<account>\db_storage'

# Explicitly bypass the account key and use the existing read-only scanner.
wx init --force --db-dir '<account>\db_storage' --key-provider memory
```

The default `--key-provider auto` tries the local DPAPI record first. If it is
unreadable, belongs to a different directory, or fails complete database
verification, the old account-key file is retained and the read-only scanner
is used. No account key implies the existing read-only behavior.

This is a local database derivation secret, not the WeChat login password or a
guaranteed permanent cross-device account identifier. An old account key may
not cover migrated, re-keyed or newly created databases; validation is required.

## Build and validation

Windows startup clears inheritance on the original standard handles before
creating threads or children. This prevents a long-lived WeChat process or
daemon from keeping a PowerShell caller's output pipe open after wx exits.
Explicit child stdio redirection is preserved. Frida retains its default spawn
stdio mode; piped Frida stdio is not used because it can delay Frida teardown.
Process-level regression tests cover EOF on both stdout and stderr while the
background child is still alive, for Rust and Frida spawn paths. Their presence
does not imply every ignored process fixture ran in the current snapshot; see
the migration ledger's separate optional-test accounting.

The Windows build downloads a Frida core devkit through `frida-sys` and uses
libclang for bindgen. Set `LIBCLANG_PATH` to the directory containing
`libclang.dll` when it is not already discoverable. This is a build dependency.

```powershell
cargo check --target x86_64-pc-windows-msvc
cargo test --target x86_64-pc-windows-msvc --quiet
cargo test frida_binary_message_smoke -- --ignored --test-threads=1
cargo build --release --target x86_64-pc-windows-msvc
```

The explicit integration test starts a hidden temporary PowerShell process to
validate Frida attachment, embedded script loading and binary messages. It does
not capture real keys or restart WeChat. Unit tests verify multi-salt derivation,
wrong-key rejection, complete coverage, DPAPI integrity, directory binding,
backup preservation and new-shard reuse. Real WeChat compatibility must be
tested separately; instruction layout and HMAC calling conventions can change
between WeChat builds.

The commands above are maintainer validation recipes, not this documentation
pass's results. In particular, a Frida test against a temporary synthetic
process does not establish current real WeChat login/capture compatibility.

Upstream source and notices: [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).
