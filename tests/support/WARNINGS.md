# Fixture warning ownership

This document defines warning scope for root test targets and independent
fixtures. The [check matrix](CHECK_MATRIX.md) lists target selection and runtime
feature requirements; the [test guide](../README.md) defines execution and
dependency requirements. Build status comes from actual exit codes and logs,
not from this document.

## Shared helper members

- `key_store::seed`, `seed_unverified` and `seed_image` build current-format DPAPI
  fixtures from explicit synthetic values. They do not read legacy key files or
  launch migration/acquisition commands. Individual convenience members permit
  dead_code because integration targets seed different material types.
- The shared Account `seed_keys` method is used by mcp-image-runtime after
  adding synthetic keys, but not by mcp-readonly-runtime. The allowance belongs
  on this one method, not its fixture module or all warnings.
- cfg(test) cannot distinguish these consumers: both are test targets. An expect
  would be unfulfilled in targets that use the member, so shared optional
  surfaces use allow(dead_code), not expect.

## Embedded production modules

Allowances apply at named module inclusion points under tests, never to
production binaries. Fixtures include real source and its embedded unit tests;
they do not create fake references just to mark code used.

- files: voice/WAV/image/download fixtures use publication guards, not the full
  directory collector surface.
- key_store: not every fixture consumes every snapshot/update member.
  Image fixtures supply a complete artificial AES+XOR pair; one-byte image
  records belong only to explicit protected-format rejection tests.
- windows_process / managed: managed execution is real, but account-capture Job
  construction and attachment are outside ASR/voice/download host scope.
  Lifecycle tests remain included.
- business::media: shared voice/emoticon registrations do not construct every
  message-attachment identity. The allowance belongs on that domain module,
  not all business modules.
- voice_catalog: decode-only hosts include the adapter and SQLite tests but not
  the daemon inventory entry point (discover_media). Listing fixtures consume
  that entry point, so an expect cannot apply to both cases.
- The independent voice-host-security cache registration permits its unused
  ResourceSnapshot re-export. ASR fixtures that call asr_database consume the
  corresponding API.

The same module-inclusion rules apply to direct file/store/managed registrations
in ASR, image, emoticon and SNS download fixtures and shared support.

## Public business modules

Production-local dead_code expects can be valid for private binary reachability
but unfulfilled in public or partial fixture embedding. The media, messages and
emoticons declarations in media_business, their direct equivalents in
emoticons-download/business.rs, and the direct messages declaration in
mcp-protocol permit unfulfilled_lint_expectations at those inclusion points.
Do not suppress unfulfilled expectations globally or change production expects
to accommodate a fixture.

## Synthetic catalog coverage

The emoticons-download normal library calls export_from through convert_download.
It creates a temporary real SQLite catalog with the production schema, obtains
its CatalogSource reference, pins the catalog input, and supplies DownloadOptions
and protected input paths. Synthetic HTTP/converter supervision tests exercise
this path; they do not replace the catalog with a fake production shim.

## Validation boundary

Inspect actual helper/import consumers and target reachability before adding an
allowance. Keep allowances local; do not use global allow(warnings), crate-wide
lint suppression, fake references or deleted assertions to pass checks.
Root all-targets and independent fixture checks are separate scopes. Runtime
feature compilation is not runtime execution, and static inspection does not
establish a warning-free build. No real account is required for these checks.
