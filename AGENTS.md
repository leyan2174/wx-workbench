# wx-cli Agent Rules

This fork targets Windows x64 MSVC only.

- After Rust changes, run `cargo check`; never commit failing Rust code.
- Before delivery, run `cargo check --target x86_64-pc-windows-msvc` and `cargo test`.
- When changing the package version, run `cargo update --workspace`.
- Preserve account isolation; never commit keys, private data or decrypted caches.
- After each commit, push to the configured `origin` remote.
- Do not restore removed platform code or release targets without a user request.
