# SNS cache and unavailable-source boundary

The two residual findings in `phase5-domain-audit-snapshot.md` are addressed by
this slice. This is implementation evidence, not a claim that tests were run.

## Ownership

- `adapters/wechat/moments/cache.rs` owns the existing SNS DAT codec, header
  inspection, cache layout/indexing, legacy image scoring, video key association,
  bounded legacy JSON interpretation and recovery-reference projection. Codec
  algorithms are moved unchanged, not merged with the strict attachment codec.
- `toolkit/sns/cache.rs` owns controlled recovery writes and source snapshot
  checks. Its writer returns business `RecoveredMediaFile` values containing
  only a relative path and byte count. The adapter cannot choose an output root
  or execute a daemon operation. The compatibility host entry delegates all
  recovery decisions to the adapter; there is no alternate host parser/codec.
- Timeline, album and archive hosts now use the adapter's cache-root helpers and
  index types. Timeline export invokes the host writer and adapter reference
  projection. Existing cache facade reexports are aliases, not copied algorithms.

## Outcome Compatibility

Missing SNS source keeps the prior report fields and adds `status: unavailable`,
`exit_code: 1` and `coverage: local_cache_only`. It returns typed
`moments::SourceError::Unavailable`, does not load image keys or scan caches, and
does not create a database or output tree. This intentionally replaces the old
successful missing-source exit. Read-only open/canonicalization failures also
retain an Unavailable cause. A real empty database remains a valid empty export.

RecordedCompatibility for export and Effective for query remain unchanged.
Image matching is still labeled `legacy_image_heuristic`, not proof of media
identity; video association remains `post_media_md5`. Partial-video policy is
unchanged. Neither export success nor cache recovery claims complete remote
history.

## Synthetic Evidence To Run

- Existing `toolkit::sns::cache::cache_tests` golden DAT/header/dimension,
  matching, exact-byte recovery, source mutation, root bounds and JSON projection
  assertions are retained and now execute the real adapter.
- Added `adapter_recovery_uses_typed_writer_and_preserves_partial_failure`:
  typed sink dispatch, per-item failure continuation, unsupported/invalid media,
  legacy reference fields and changed-post rejection.
- Updated `missing_sns_db_is_unavailable_without_creating_output` and
  `read_only_path_entry_and_missing_path` to require typed Unavailable while
  preserving the no-created-source/output assertions.
- Added real CLI test
  `missing_source_reports_unavailable_and_nonzero_without_touching_output`,
  checking process failure, report fields, local-only scope and an untouched
  existing output tree.

No Cargo, test run or commit was performed in this slice. Parent integration
owns compilation and targeted/root/fixture validation.
