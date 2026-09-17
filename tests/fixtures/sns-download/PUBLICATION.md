# SNS single-file publication

The production image, video and general download paths use `ExportTarget`.
The album/timeline directory transaction is separate: media
publication within its protected staging tree does not replace that transaction.

- Images use `new_file` and `write_bytes_checked`; existing images are never
  overwritten by this publication path.
- Videos capture their destination before streaming and use `write_with_checked`.
  Local cache copies retain source pins, identity/size/mtime checks and the source
  mtime. Remote videos retain the 128 KiB decode prefix, 64 KiB copy buffer,
  size limit, HTTP completeness and Content-Length checks.
- General downloads read at most 12 signature bytes before selecting and capturing
  the actual destination. Remaining data uses a 16 KiB buffer. Hidden filenames,
  explicit extensions, minimum payload size and bounded replacement are preserved.
- The caller's output guard checks protected sources before commit; the shared
  publisher checks staging, parent and captured target before atomic publication.
  Failed streams leave existing targets intact and clean up only their own stage.

These paths launch no subprocesses. In-process WASM limits apply;
network access requires explicit authorization, with no implicit download entry.

Synthetic coverage lives in `download_tests.rs`, `album_images_tests.rs` and
`album_videos_tests.rs`: short reads, bounded streaming, exact limits/overflow,
hidden names, concurrent destination changes, no-clobber and typed video errors.
HTTP tests use loopback only. `sns-download` registers the real shared publisher,
runtime, encrypted store and cache dependencies through `publication.rs`; album
test runners still select the production root modules via `run-module.ps1`.

Execution requirements are in the [test guide](../../README.md).
