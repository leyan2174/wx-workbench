# Fixture warning ownership

This note covers test-only wiring changes against the supplied
`C:/CodexLocal/wx-cli-warnings-index.json` and
`C:/CodexLocal/wx-cli-warnings-before.jsonl` baseline (main f85ca4c).
The JSONL also contains Cargo text lines; only compiler-message JSON records are
counted. Root test targets account for 18 warning records:

| Target | Records |
| --- | ---: |
| mcp_runtime | 8 |
| mcp_security | 7 |
| mcp_readonly_runtime | 1 |
| run_decrypt_runtime | 1 |
| sns_timeline_runtime | 1 |

## Shared helper members

- `key_store::seed`, `seed_unverified` and `seed_image` build current-format DPAPI
  fixtures from explicit synthetic values. They do not read legacy key files or
  launch migration/acquisition commands. Individual convenience members permit
  dead_code because different integration targets seed different material types.
- The shared Account `seed_keys` method is used by mcp-image-runtime after
  adding synthetic keys, but not by mcp-readonly-runtime. The allowance is
  on this one method, not its fixture module or all warnings.
- cfg(test) cannot distinguish these consumers: both are test targets. An expect
  would be unfulfilled in targets that do use the member, so these shared
  optional surfaces deliberately use allow(dead_code), not expect.

## Embedded production modules

The following allowances apply at named module inclusion points under tests,
never to the production binaries. They keep the real source and its embedded
unit tests; they do not create fake references just to mark code used.

- files: voice/WAV/image/download fixtures use publication guards, not the full
  directory collector surface. File-module dead_code is expected in those hosts.
- key_store: not every embedded fixture consumes every current snapshot/update
  member. Legacy import, its JSON reader, counts and XOR-only construction have
  been removed. Image fixtures always supply a complete artificial AES+XOR pair;
  one-byte image records exist only in explicit protected-format rejection tests.
- windows_process / managed: managed execution is real, but account-capture Job
  construction and attachment are outside the ASR/voice/download host scope.
  Existing lifecycle tests remain included.
- business::media: shared voice/emoticon fixture registrations do not construct
  every message-attachment identity. The allowance is on that domain module,
  not on all business modules.
- voice_catalog: decode-only hosts include the actual adapter and its SQLite
  tests but not the daemon inventory entry point (discover_media). Listing
  fixtures that do call it keep working; no expect is used across the two cases.
- The independent voice-host-security cache registration also allows its unused
  ResourceSnapshot re-export, matching the existing voice host / ASR pattern.

Unused separate re-exports are removed from mcp-voice-host,
mcp-voice-host-security and wav-publish. They remain in asr-cache-security and
asr-video-security because their real asr_database operation calls that API.

The same module-inclusion reasoning was applied to the direct file/store/managed
registrations in the relevant independent ASR, image, emoticon and SNS download
fixtures, as well as shared support. No manifest dependency was added or changed.

## Synchronized Names cleanup

Removed the unused md5_to_uname initializer from mcp-image, mcp-refer,
mcp-image-listing-parity and delta-query fixtures, and the unused mock Names field
from mcp-readonly-security. Real map, message inventory and verification fields
and all assertions remain. Fixture-owned DbCache::new constructors are unchanged.
This accompanies the parent's removal of the production Names field, not a
replacement of any business identity or inventory proof.

## Validation boundary

No Cargo command or test execution was performed. Static work includes examining
the warning JSON, checking actual helper/import consumers, enumerating all 26
independent fixture manifests and checking the patch for whitespace errors.
Root all-targets and all 26 manifests still require the parent's centralized
check. A source-only scan is not a claim that the resulting build is warning-free.

No global allow(warnings), crate-wide lint suppression, new expect annotation,
production deletion or src edit was introduced. The binary warnings in the index
remain with the parent/Tesla. Future production expect annotations may behave
differently when the same module is public in a fixture; do not hide new
unfulfilled expectations globally instead of checking their target scope.

## after-1 targeted follow-up

The parent's root after-1 check exited successfully but reported 32 test-side
unfulfilled_lint_expectations plus two production MAX_STORED_BYTES import
diagnostics. The test diagnostics concern media, messages and emoticons business
modules exposed publicly by fixtures. Their production-local dead_code expects
are valid for private binary reachability but can be unfulfilled in public or
partial fixture embedding.

Only these three module declarations in media_business, their direct equivalents
in emoticons-download/business.rs, and the direct messages declaration in
mcp-protocol permit unfulfilled_lint_expectations. No production expect changed,
and no crate/global lint allowance was added. The private whole-business contract
target had no such diagnostic and was not changed. The two production import
diagnostics remain with the parent.

The emoticons-download normal library no longer calls the cfg(test) download
wrapper. convert_download creates a temporary real SQLite catalog using the
existing schema, obtains its CatalogSource reference, pins the catalog input,
and calls the actual export_from with DownloadOptions and protected input paths.
Its synthetic HTTP/converter supervision tests retain their original interface
and assertions; no fake catalog or production shim was added. The subsequent
centralized root/26-manifest checks completed without warnings or errors in the
declared scopes. See `CHECK_MATRIX.md` and `docs/compiler-warnings.md` for the
final commands, log locations and explicit runtime feature binding.
