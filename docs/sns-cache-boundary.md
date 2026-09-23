# SNS cache and unavailable-source boundary

## Ownership

- `adapters/wechat/moments/cache.rs` owns the existing SNS DAT codec, header
  inspection, cache layout/indexing, legacy image scoring, video key association,
  bounded legacy JSON interpretation and recovery-reference projection. The SNS codec is separate from the strict attachment codec.
- `application/moments/cache.rs` owns controlled recovery writes and source snapshot
  checks. Its writer returns business `RecoveredMediaFile` values containing
  only a relative path and byte count. The adapter cannot choose an output root
  or execute a daemon operation. The compatibility host entry delegates all
  recovery decisions to the adapter; there is no alternate host parser/codec.
- Timeline, album and archive hosts use the adapter's cache-root helpers and
  index types. Timeline export invokes the host writer and adapter reference
  projection. Existing cache facade reexports are aliases, not copied algorithms.

## Outcome Compatibility

### Timeline content decoding

`adapters/wechat/moments/decode.rs` interprets timeline content separately from
the media DAT codec. Encoded text or BLOB input is limited to 1,600,000 bytes.
A standard Zstandard frame header selects bounded decompression: the decoder
uses `window_log_max(23)`, reads at most 800,001 output bytes and rejects output
above 800,000 bytes. Recognized hexadecimal or Base64 text is converted once
to BLOB handling; it is not recursively unwrapped as arbitrary text encodings.
After entity decoding, XML text is limited to 200,000 Unicode characters.

This is compatibility decoding, not strict or lossless UTF-8 validation.
Malformed UTF-8 sequences are discarded; XML cleanup can remove control
characters and escape selected text. A successfully parsed post therefore does
not prove byte-for-byte preservation of its original content. The decoder
rejects `DOCTYPE`/`ENTITY` declaration markers before cleanup; XML parsing and
its errors remain separate checks. These budgets do not establish complete
remote-history coverage or a bound on the whole export's peak memory.

Missing SNS source keeps the prior report fields and includes `status: unavailable`,
`exit_code: 1` and `coverage: local_cache_only`. It returns typed
`moments::SourceError::Unavailable`, does not load image keys or scan caches, and
does not create a database or output tree. Missing source is a failure. Read-only open/canonicalization failures also
retain an Unavailable cause. A real empty database remains a valid empty export.

RecordedCompatibility for export and Effective for query remain unchanged.
Image matching is still labeled `legacy_image_heuristic`, not proof of media
identity; video association remains `post_media_md5`. Partial-video policy is
unchanged. Neither export success nor cache recovery claims complete remote
history.

### Report status, process exit and task status

The timeline host reports `status: partial` and `exit_code: 1` when media
recovery, attempted downloads or record parsing fail. It then returns an
ordinary error; this path does not construct `BusinessFailure::Partial` and
does not use worker exit code 20. The offline snapshot host likewise prints
its report before returning an error for those failure counts. A queued SNS
export with this nonzero exit is a failed task even if some files were written.

`media_missing` alone does not trigger those failure checks. For example, an
item without a download URL can remain missing without an attempted-download
failure. Inspect the individual counters and warnings; exit 0 is not a claim
that every referenced medium was recovered.

MCP task get/list or HTTP task get/list can successfully return the JSON of a
failed task. Their transport success is not the background export outcome;
clients must inspect the task status, exit code and available diagnostics.

## Offline Snapshot Material Boundary

`wx moments export-snapshot` retains explicit offline SNS/contact database and
cache-root inputs. Each invocation fixes its `RuntimeContext` and `ConfigPin`.
With a cache enabled, image material comes from the protected worker
`ImageSnapshot`, with broker revision checks; `LocalCacheArgs` no longer accepts
`image_key_file` and rejects unknown fields. `--image-xor-key` remains a byte-format
parameter, not an AES import or authorization channel. Independent video-key
inputs are not changed by this boundary.

The checked index builder uses the adapter's existing candidate enumeration.
Before inspection, the host pins each candidate and its parent/root ancestry.
It classifies images by the actual six-byte header, not their name or extension.
Every V2 candidate must have protected AES material and pass
`validate_material_for_image_source` against that same pinned source. Missing,
truncated or invalid V2 evidence aborts indexing; validation errors are not
converted into skipped-image warnings or plaintext fallback. The validation
pass has a shared 30-second deadline and 16 MiB header/sample-read budget.

Stored AES alone does not require an AES sample in every root. Plain, legacy
XOR, V1 and video candidates retain their existing codec rules; a mixed-root
export validates only actual V2 candidates, including each candidate in a root
that contains several V2 files. This does not establish account ownership of an
offline source or prove that a recovered image belongs to a particular post.

The report's `image_material_status` is `not_requested` without a cache,
`unverified` when no V2 samples were verified, or `verified_local_samples` when
the local V2 checks succeeded. The last value describes material validation,
not media association or whole-account completeness.

## Source And Publication Guards

Offline SNS and optional contact databases are pinned with their parent
directories. Existing `-wal`, `-shm` and `-journal` files are pinned; absent
sidecars are recorded and checked again by `Sources::verify`. A newly appearing
sidecar is a source-change error, not silently incorporated into the export.
SQLite still uses the existing read-only opener: these guards do not provide
a WAL checkpoint, an immutable SQLite snapshot or a promise that every live
WAL configuration can be read. Open failures propagate rather than enabling
writes or falling back to another database.

Cache candidate pins and validated-sample guards live through export. Offline
export supplies an application-local `VerifiedPublication` callback independently
of cache recovery. It checks the runtime/config binding and SNS/contact sources,
including absent sidecars, even when no cache is selected. The no-cache branch
does not read image material or create an empty `CacheRecovery`. With cache
enabled, verification additionally checks sample guards and broker image revision.
The account timeline constructor retains its mandatory cache verification callback.
No callback is a business request/DTO field.

The writer verifies at timeline publication entry, immediately before fresh
directory rename, and through `OutputTree::publish_with` immediately before each
update file persist. Source failure before the first commit leaves old timeline
and media bytes unchanged. A failure at a later file preserves earlier committed
files and leaves later targets unchanged; neither this callback nor output-tree
checks provide cross-file/object atomicity or CAS. Other callers retain the
existing `publish_all` wrapper and its no-op caller hook.

The update-mode `source_id` hashes selected database paths solely to label a
publication source. It is not evidence of account ownership. Output roots remain
separate from input databases/cache roots, and recovery records expose relative
output paths. Network retrieval still requires `--download-media`.

## Minimal Offline CLI Fixture

Use a closed, synthetic SQLite database in `source/sns.db` with:

```sql
CREATE TABLE SnsTimeLine(tid INTEGER, user_name TEXT, content TEXT);
```

Insert one row with `tid=1`, `user_name='test-user'` and this `content`:

```xml
<root><LocalExtraInfo><nickname>Tester</nickname></LocalExtraInfo><TimelineObject><id>1</id><username>test-user</username><createTime>1700000000</createTime><ContentObject><type>1</type><mediaList><media><type>2</type><url>https://synthetic.invalid/image</url><thumb></thumb><size width="0" height="0" /></media></mediaList></ContentObject></TimelineObject></root>
```

For a single image candidate, use either layout:

```text
xwechat/2026-09/Sns/Img/aa/sample
legacy-sns/2026-09/sample
```

The image filename need not match the XML URL or media ID. Legacy-root files
ending in `_t` are excluded. Image selection remains the documented heuristic:
time window (falling back to the full index when empty), dimensions and size.
One candidate plus one image media with unspecified dimensions avoids ambiguous
fixture matching; it does not strengthen production association claims.

After explicit protected-material import under the same runtime/config, invoke:

```powershell
wx moments export-snapshot "$fixture/source/sns.db" "$fixture/output" --xwechat-cache "$fixture/xwechat" --contacts test-user --utc-offset +08:00
```

Use `--sns-cache "$fixture/legacy-sns"` for the second layout, or both flags for
mixed roots. Keep output outside the source/cache directories and fresh unless
testing `--update`. Do not pass `--download-media`; the synthetic URL is not
fetched. The export command neither imports AES nor accepts `--image-key-file`.
For byte assertions, the synthetic `tests/fixtures/sns/cache_golden.json`
`decrypt_cases` entries provide `input_hex`, `expected_hex`, `key_hex` and
`xor_key`; use a valid V2 case such as `v2_aes_96` and import its matching material.
Import-sample discovery is a separate contract from the extensionless SNS cache
layout above.

## Synthetic coverage

- `daemon::operations::export_sns::tests` covers stored AES with plain cache,
  XOR-only recovery without AES, mixed V2/plain roots, per-candidate evidence in
  one root, rejection of later invalid/truncated V2 and missing protected AES.
  Compatibility cases compare exact decoded bytes; pinned candidates cannot be
  replaced or removed during consumption.
- `failed_publication_verification_preserves_existing_timeline_and_media`
  compares the entire published file tree before/after a rejected update, checks
  callback invocation and then proves that the same update changes output when
  verification succeeds.
- No-cache tests cover successful export without recovery fields, rejection at
  fresh entry/final rename, a real previously absent WAL appearing in the
  first update commit callback, and rejection of the second update file with
  the first commit retained and all later files unchanged.
- Existing `application::moments::cache::cache_tests` golden DAT/header/dimension,
  matching, exact-byte recovery, source mutation, root bounds and JSON projection
  assertions execute the real adapter.
- `adapter_recovery_uses_typed_writer_and_preserves_partial_failure`:
  typed sink dispatch, per-item failure continuation, unsupported/invalid media,
  legacy reference fields and changed-post rejection.
- `missing_sns_db_is_unavailable_without_creating_output` and
  `read_only_path_entry_and_missing_path` to require typed Unavailable while
  preserving the no-created-source/output assertions.
- The real CLI test
  `missing_source_reports_unavailable_and_nonzero_without_touching_output`,
  checking process failure, report fields, local-only scope and an untouched
  existing output tree.

Run module and fixture checks according to [test instructions](../tests/README.md).
Synthetic coverage does not establish real-account completeness or remote media availability.
