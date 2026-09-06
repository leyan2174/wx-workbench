# Windows account-key provider

Windows x64 builds support optional account-key capture based on
LOGO127/wechat-ai-memory. Rust manages the process lifecycle, key derivation,
SQLCipher verification and storage. Frida runs the embedded
`src/scanner/windows/account_hook.js` inside the target process. Python and
Node.js are not required for this provider. This does not migrate the separate
Python/Node-based media toolkit to Rust.

## Capture

Run from the account workspace containing its `config.json`:

```powershell
$env:WX_CLI_HOME = '<account workspace>\.wx-cli-runtime'
wx init --force --db-dir '<account>\db_storage' --key-provider account --restart-wechat
```

This explicitly closes the matching running Weixin.exe processes and starts
WeChat again. Complete QR/mobile login to the selected account within 300
seconds. There is no automated clicking of login controls. Optional arguments:

```text
--wechat-exe <path to Weixin.exe>
--capture-timeout <10..1800 seconds>
```

`--restart-wechat` requires `--force` and `--key-provider account`. The default
provider never implicitly invokes this restart/capture path.

## Derivation and verification

The hook locates a SHA-512 implementation using constants and instruction
references, then identifies HMAC input blocks with the 32-byte key and ipad
layout. Candidates travel as binary Frida messages and are not logged.
PBKDF2-HMAC-SHA512 with 256000 iterations derives a database key from the
candidate and that database's 16-byte salt. Page-one HMAC verification checks
the candidate against `message/message_0.db`. All target databases are then
individually derived and verified, including WAL page-one variants.

The same top-level `migrate` exclusion and target-path coverage rules apply as
in the read-only scanner. Missing coverage fails before saving keys.

## Storage and reuse

After complete verification, `account_key.dpapi` is saved alongside the active
`config.json`. It contains a versioned record protected by Windows DPAPI for
the current Windows user and bound to the canonical database directory.
Replacing it preserves the prior encrypted file as a timestamped `.bak`.
The file, temporary files and backups are ignored by Git. Do not distribute
them or assume they can be decrypted under another Windows user or machine.
Existing `all_keys.json` remains the per-database key format used by the daemon.

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
Process-level regression tests check EOF on both stdout and stderr while the
background child is still alive, for Rust and Frida spawn paths.

The Windows build downloads a Frida core devkit through `frida-sys` and uses
libclang for bindgen. Set `LIBCLANG_PATH` to the directory containing
`libclang.dll` when it is not already discoverable. This is a build dependency.

```powershell
cargo check
cargo test --quiet
cargo test frida_binary_message_smoke -- --ignored --test-threads=1
cargo build --release
```

The explicit integration test starts a hidden temporary PowerShell process to
validate Frida attachment, embedded script loading and binary messages. It does
not capture real keys or restart WeChat. Unit tests verify multi-salt derivation,
wrong-key rejection, complete coverage, DPAPI integrity, directory binding,
backup preservation and new-shard reuse. Real WeChat compatibility must be
tested separately; instruction layout and HMAC calling conventions can change
between WeChat builds.

Upstream source and notices: [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).
