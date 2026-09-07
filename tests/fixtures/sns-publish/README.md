# SNS Publish Harness

> Documentation check, 2026-09-07: `sns/mod.rs` already registers `publish`, and
> production timeline/album exports use it. The standalone harness remains;
> this document update did not run tests. Run results belong to their recorded
> source snapshot, not to this cleanup.

Windows-only, synthetic temporary files; no account discovery, database reads, or network media.
Imports the actual new publisher and existing HostOutputGuard without changing the main module tree.

```powershell
cargo check --manifest-path tests/fixtures/sns-publish/Cargo.toml --target x86_64-pc-windows-msvc
cargo test --manifest-path tests/fixtures/sns-publish/Cargo.toml --target x86_64-pc-windows-msvc publish::tests -- --nocapture
```

For the registered module, run `cargo test --bin wx toolkit::sns::publish::tests -- --nocapture`
from the repository root. No additional module registration is needed. Bindings must come
from the caller's account/database context; source IDs are compared verbatim, not authenticated.
Pass all candidate relative files to `prepare`, including optional media extensions and sidecars.
Use `guard(Path::new(""))` for root files, and `guard(Path::new("images"))` / `videos` for reuse.
Parents of planned files are created; `tree_kind = "album"` also always prepares empty `images`
and `videos` directories, even without media jobs. Parent depth is bounded, with at most 128 subdirectories.
Stage downloads outside the output tree. `publish_all` checks every source and target before
replacing anything and preserves caller ordering. An error states the number of committed files;
earlier files are not rolled back, later summary files are not published.

The binding manifest is an ownership declaration, not a completion receipt. Explicit legacy
adoption keeps `legacy_unverified` true on subsequent updates. Unknown files and subtrees are
not recursively scanned, removed, or claimed as verified. The persistent lock file is intentionally
not deleted; the exclusive Windows write handle releases automatically on drop/process exit.
This excludes cooperating writers; final path checks and atomic replacement are not an adversarial
cross-process compare-and-swap or a multi-file crash transaction.
