# Legacy timeline update oracle

> Documentation check, 2026-09-07: the native observations and test counts below
> are historical evidence, not current registration or full-suite status. No
> oracle, golden or test was rerun for this document update.

## Current native entry points

The four thin wrappers removed from `src/toolkit/sns/export.rs` are
`write_export`, `write_export_with_cache`, `export_database` and
`export_database_with_cache`; their `sns/mod.rs` re-exports were also removed.
Existing tests now call `write_export_with_media` / `export_database_with_media`,
passing explicit `None, None` for absent cache/download options. The publication
path is separate and remains in production; removing wrappers does not remove
timeline update tests, the Python oracle or golden files.

From the repository root, `cargo test --bin wx toolkit::sns::tests -- --nocapture`
targets the retargeted core tests. SNS harness sources and `run.ps1` files remain;
cleanup removed the generated album-image/video test executables and PDBs, not
those harnesses. This command is a current entry point, not a new passing result.

## Legacy oracle entry

Run from repository root (Python 3.12 stdlib; no pytest installation needed):

```powershell
python -B tests/fixtures/sns-timeline-upsert-oracle/oracle.py
```

This is an independent executable oracle, not a reimplementation of export.
`oracle.py` extracts allowlisted AST definitions and parser constants from
`vendor/wechat-decrypt/export_sns.py`, including the unmodified
`export_sns_timeline`, actual XML parser, SQLite contact/comment loaders,
filename generator, JSON writes and HTML renderer. Vendor module top-level
code and imports are never executed. `config.py` was read, not executed:
`load_config` can discover accounts and write config.json. Explicit temporary
database/output paths replace its effects. Cache lookup/decryption supplies
only an embedded synthetic PNG. Download attempts call a recording failure
stub, never a network library. No real keys, accounts or cache trees are read.
Only plain XML is exercised, not zstd or real cache matching/decryption.
UTC+08:00 and export time 1700001000 are fixed test clocks, not a claim that
legacy fixes timezone: production legacy uses host-local datetime.

## Golden contract

`input.json` contains public invented source rows. `golden.json` contains a
complete second-run summary and post, exact remaining filename list, HTML
text/image projections and author/collision expectations. Tests compare real
generated artifacts, not a separately implemented expected exporter.
The source SHA256 is printed on each run; the inspected baseline was
`95539b459f4c38d6e106f8a601b9fdb21edf0f1ef916c3d15c662a1f19b44df3`.

- First run exports two current-author posts and one other contact. Second
  run removes an older source row, edits the retained row, and filters the
  other contact despite its XML username matching the selected author.
- Same-name current post JSON, timeline.json and timeline.html are rewritten.
  Both bytes and old deliberately fixed mtimes change. The full second JSON
  is asserted, including total_posts=1 and just tid=1.
- The older extra post JSON, its flat media, current-name old media, unrelated
  file and nested unrelated file survive byte-for-byte with unchanged mtimes.
  The filtered contact's complete tree also retains bytes and mtimes.
- **This is not merging old timeline posts.** Old tid=2 remains a loose JSON
  file, but disappears from the new summary and HTML. A consumer scanning all
  loose JSON files will therefore see more posts than timeline.json.
- Round two has no cache match and a failed stubbed download. Existing flat
  images remain on disk but are NOT rediscovered into the new HTML. Legacy
  media JSON has no local_file enrichment; HTML uses this run's image_files.
- A further empty-row call leaves all output files and mtimes unchanged and
  never builds the cache index. It still loads contacts/comments first.
- summary.user_name is the database grouping key, post.db_user_name is the
  database author (NULL/empty becomes ""), and post.username is the XML author.
  NULL/empty database authors group under "unknown" even with distinct XML
  authors. Directory naming can use XML nickname, so names are not identity.
- Same-second stems are assigned afresh from an empty used_names set, not
  looked up by tid/id or reserved against existing files. Removing the first
  tied row makes the survivor overwrite ...000.json while old ...001.json
  survives, now duplicating the survivor on disk. SQL has no ORDER BY; equal
  timestamp assignment follows fetched row order, not a guaranteed stable ID.
  The collision golden uses timestamp 0 to exercise the zero-name sentinel.

## Native migration observations (read-only)

Observed source state is a concurrent working tree, not a native-runtime
differential test. These are source-review findings, separate from the
executed legacy assertions above:

- Final reread of `src/toolkit/sns/export.rs` found concurrent integration of
  write_export_with_publication. Without publication it still rejects an
  existing SNS destination and renames a fresh directory. With publication
  it now calls publish::prepare/publish_all and returns early for no timelines.
  This supersedes the initial fresh-only observation; native behavior has
  not been executed by this Python oracle.
- `publish.rs` prepare/publish_all supports bound destinations, explicit Adopt
  for unbound nonempty trees, caller-supplied legacy validation, unknown-file
  preservation and per-file replacement. The final export source integrates
  it through the optional publication path, not the default fresh-only path.
  Its manifest binds caller-provided source values, not proven historical
  account ownership. Validate summary.user_name/post.db_user_name, not XML
  username or display directory, while retaining this provenance limitation.
  Final legacy_post_identity handles empty db_user_name as "unknown" and
  legacy_timeline_identity checks summary.user_name; missing db_user_name is
  accepted, so this remains explicit unverified adoption, not proof of origin.
- Legacy output is flat SNS/<timestamp>_<index>.<ext>. Native cache.rs returns
  images/<timestamp>_<index>.<ext> and videos/<timestamp>_<index>.mp4 through
  local_file; export adds media references and _media_recovery.json. Adopting
  legacy output must preserve flat extras while planning nested destinations.
  Do not assume all old media have local_file or live in images/videos, and
  do not silently merge old summary posts to recover references. Publisher
  preflight must include the actual nested target parents and files. The
  final publication_targets source plans both flat download candidates and
  nested images/videos cache candidates.
- Native safe_dirname additionally rejects/normalizes controls, trailing dots
  and Windows reserved names; read_database disambiguates case-folded display
  names across contacts. Legacy only substitutes forbidden characters and
  strips whitespace, so native directory selection can differ. Mapping an
  existing display folder to an account requires explicit source validation.
- Preserve the local-time timestamp policy and run-local same-second suffix
  rule if compatibility is intended. The suffix is minimum-width three
  digits, not capped at 999; 1001+ tied posts can exceed a 17-character stem
  (source review only). A strict 17-digit adoption filter would miss them;
  the final adoption code uses >=17 all-digit stems, covering that case.
  Older same-name media with another extension can survive; HTML currently
  reflects only successful recovery/download during this run, not disk scan.
- Legacy writes post/media, summary, then HTML non-transactionally. Publisher
  documents caller ordering and partial commits without rollback. A new
  publication order or binding/lock files are deliberate safety differences,
  not byte-identical legacy behavior. No recommendation to delete vendor.

## Scope

Validation: all 3 stdlib unittest cases passed (6 actual legacy exporter
invocations). `verification.json` preserves the successful command's complete
combined stdout/stderr. Repository-required `cargo check --target
x86_64-pc-windows-msvc --locked` and `cargo test --locked` were also attempted;
both exited 1 in dependency build scripts because bindgen could not find
clang.dll/libclang.dll (silk-codec/frida-sys). No native test pass is claimed.

Only new files in this directory are owned by this evidence task. No existing
tests, production code, configuration or vendor files are modified. Runtime
SQLite databases and generated export trees live in TemporaryDirectory and
are closed and cleaned after each test. HTML is checked structurally, not
as a full stylesheet golden. Native runtime behavior, real network/cache
recovery, filesystem races and malformed legacy-adoption inputs are outside
this oracle's coverage.
