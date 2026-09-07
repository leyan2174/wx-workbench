# Catalog migration verification

Reference: `vendor/wechat-decrypt/emoticons.py::build_emoji_lookup`.
`oracle.py` extracts and runs the original mapping AST against synthetic,
read-only SQLite fixtures. Production does not invoke Python.

## Preserved mapping behavior

- NonStore first insertion order, last duplicate value, NULL/empty fields.
- Template collection includes rows without MD5; last nonempty template wins.
- Store skips empty and existing MD5; package matching is case-sensitive.
- Store requires an ampersand, but does not require a regex match.
- Lowercase hex regex retains partial, non-parameter and multiple matches.
- Replacement strings interpret Python escapes, octal bytes and whole-match
  references; invalid escapes/references fail even when nothing matches.
- Captions only use language `default`; last duplicate wins; NULL becomes
  an empty caption, distinct from an absent caption.
- Counts reflect distinct NonStore MD5 and newly inserted Store MD5.

## Corrections in this pass

- Replaced literal substitution with the legacy replacement-string semantics.
- Removed underlying SQLite/cache errors from public error chains: schema
  names may contain sensitive values. Display, alternate Display and Debug
  are tested against a synthetic secret-bearing view.
- Mapping assertion failures no longer dump the complete URL/key mapping.

## Intentional differences from Python

- No shared temporary decrypted database. The supplied account's DbCache
  owns decryption, cached copies, persistence and WAL application.
- Only the exact normalized relative database name is selected; the original
  key is passed unchanged to DbCache. Canonical collisions are errors.
- Missing key/source returns an empty catalog. Decryption/read failures return
  a redacted error rather than printing an exception and returning empty.
- Only a missing caption table is optional. Broken schema/views, locks and
  invalid rows fail the complete read instead of silently omitting captions.
- Mapping columns must be text or NULL. Python's accidental support for
  numeric/blob dictionary values is not part of the typed Rust contract.
- The three queries share a read transaction instead of separate snapshots.

## Verification

Nine catalog tests pass on Windows x64 MSVC. The AST comparisons include
15 original mapping/template cases, 7 valid replacement cases and 14 invalid
replacement cases. Cache tests use synthetic encrypted pages, different
account keys, cold WAL, cache hits, incremental WAL and restart-time WAL
updates. They assert source DB/WAL bytes remain unchanged by catalog loads.

Commands (PowerShell, from repository root):

```powershell
$env:LIBCLANG_PATH = 'C:\CodexLocal\build-tools\libclang\clang\native'
cargo check --target x86_64-pc-windows-msvc
cargo test --target x86_64-pc-windows-msvc toolkit::emoticons::catalog
cargo test --target x86_64-pc-windows-msvc --bin wx toolkit::emoticons::catalog
```

Full command output: `cargo-check.log` and `cargo-test-bin.log`.
The initial unqualified test run passed all nine catalog tests. A later
logging run could not link the unrelated `chat_plan_runtime` executable
(LNK1104, possibly an executable held by a concurrent test); its complete
output is retained in `cargo-test.log`. The `--bin wx` run avoids rebuilding
unrelated integration-test executables.
The test filter excludes unrelated integration tests; this is not a full
repository test-suite result. CLI/facade registration remains owned by the
main task.
