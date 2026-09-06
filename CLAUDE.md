# wx-cli Project Rules

This fork supports Windows x64 MSVC only. Follow AGENTS.md.

- Run cargo check after Rust edits and cargo test before delivery.
- Check the x86_64-pc-windows-msvc target; no other release targets are supported.
- Keep client and server on compatible interprocess named-pipe APIs.
- Update Cargo.lock with cargo update --workspace after version changes.
- Preserve Windows process handle isolation and account-scoped files.
- Never commit private keys or chat exports.
- Push commits to the configured origin remote.
