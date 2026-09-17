# Emoticon Catalog and Export

## Boundaries

`business::emoticons` owns catalog descriptions, selection, materialization
results and ordered batch reporting. Its catalog reference belongs to one
materialized read instance; it has no message identity, path, URL, key or
authorization flag. Dropping the source expires references, and another source
cannot accept them. There is no global registry or retained database lease.

`adapters::wechat::emoticons` owns catalog schema, field decoding, three-stage
mapping, template interpretation and private resource material. The adapter
uses the existing account cache, reads one SQLite transaction and materializes
the catalog before releasing it. Missing input is distinct from an empty valid
catalog. Schema, row and decryption errors do not return partial catalogs.

The host retains account selection, saved-key preparation, locking, authorization,
protected output construction and terminal formatting. The formal CLI entry is
`wx emoticons export`, dispatched as `Operation::ExportEmoticons`.
This entry grants no HTTP/MCP permission, remote discovery or automatic download.

## Compatibility

- Duplicate NonStore MD5 values update data without moving their first slot.
- NonStore entries win over Store entries. Missing Store entries may derive URLs
  from a recorded package template, retaining Python replacement-string behavior.
- `CatalogRecorded` and `TemplateDerived` describe provenance, not hash proof.
- Description/package filtering remains case-insensitive and order-preserving.
- Preview does not create output directories or start network/process work.
- Direct empty/failed responses may use the existing encrypted-resource fallback;
  a short nonempty response does not silently switch sources.
- The filename cache keeps its existing gif/png/jpg/webp precedence and identity
  checks. `LegacyCache` deliberately does not claim verified content MD5.
- HEVC conversion changes bytes. `Converted` and `ConversionFallback` are distinct
  outcomes; neither claims the output hash equals the catalog MD5.
- Per-item errors remain visible in the typed batch report while later items run.
  The host retains the legacy successful batch exit code with failure counts.

Chat-directory strict local MD5 matching is a separate media association policy.
It must not be replaced by this filename cache or require automatic downloading.
CatalogSource exposes read-only find/revalidate/material access for adapter-side
composition; private resource material must never become a business response.

## Publication and Processes

Images use `ExportTarget::new_file`; unknown binary outputs use
`capture_paths`. The binary target fingerprint is captured before fetching so a
concurrent change is not silently overwritten. Both publish through
`write_bytes_checked`, with the existing HostOutputGuard rechecked immediately
before the core's final temporary/parent/target validation. A batch is a sequence
of atomic files, not an atomic directory transaction. Failed publication preserves
the previous file and does not increment successful items.

HEVC conversion invokes `windows_process::managed::output`, using a deadline
covering spawn, execution and pipe draining, a 64 KiB diagnostic bound and the
shared job-object cleanup. Existing ffmpeg arguments, media size checks and
conversion-failure binary fallback are retained. Scratch files contain only
synthetic/downloaded media, not plaintext keys, and are removed on scope exit.

## Verification

Memory tests cover catalog filtering, stale/foreign references and partial batch
continuation. Actual SQLite tests cover provenance, private-material isolation
and mapping compatibility. Download tests cover protected binary replacement,
concurrent image publication, legacy-cache labeling and the managed zero-deadline
path in addition to the existing loopback/AES/limits/cleanup tests.

The download fixture also builds a synthetic converter executable. Integration
tests drive the real download/conversion path through loopback HTTP and check
successful conversion, bounded diagnostic output and timeout cleanup of a
descendant that has explicitly signaled startup. They do not require FFmpeg or
touch any real account. An optional real-FFmpeg test covers the external converter.

The catalog and download standalone fixtures reference real production modules;
the download fixture also includes real publication, runtime/cache and managed
runner code. Execution requirements are in the [test guide](../../tests/README.md);
formatting and static diff checks alone are not runtime verification.
