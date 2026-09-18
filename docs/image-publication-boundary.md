# Image publication boundary

The image entry points share `application::publication_context::PublicationContext` and `ExportTarget`.
`decode_images_for` accepts the task host's selected `RuntimeContext`; it does
not rediscover configuration. Ordinary and persistent-task entries pass their
already selected runtime and protected material from the execution host.
When `WX_CLI_EXPECTED_RUNTIME` is present, current configuration must resolve to
that runtime ID, matching the worker's expected-account check. Missing or
changed accounts cannot fall back to offline execution.

Existing configuration is held by `ConfigPin`. Publication protects the config,
keys, key store and associated key/lock files, database, decrypted cache, runtime
directory and input. Image source handles and parent guards remain pinned while
decoding and publishing. The final checked-publication callback revalidates the
configuration, source and host-supplied material revision before replacing the
destination. The host callback uses the existing process-bound material broker;
neither the format adapter nor generic publication context depends on that broker.

Explicit album input/output still requires a selected account configuration.
Ordinary operations no longer accept plaintext AES overrides. Host-only
`keys import-image --stdin` verifies supplied material against pinned samples
before saving it through the existing broker; see [key storage](key-store.md).
An optional XOR format byte is not an AES authorization override. A valid
protected snapshot without AES still supports non-AES image formats.

`adapters/wechat/media/image_batch.rs` owns DAT dispatch policy, `_t`/`_h`
normalization, account attachment layout and the album four-component rule.
It delegates decoding to the existing codec. Missing-key V2 skipping remains a
batch-only compatibility policy; single decode and keyed malformed input fail.
Album and mirror existing-output policies remain distinct. This does not relax
the separate strict attachment resolver or turn legacy results into hash proofs.

## Directory guard compatibility

Shared local-file directory pins request `FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY`
(`0x81`) with read/write sharing but no delete sharing, retaining reparse rejection
and identity checks. Write sharing permits atomic child-file publication; denying
it also blocked `NamedTempFile::persist`, including task-history startup writes.
Attributes-only handles did not reliably prevent renaming the pinned directory.
Every existing ancestor from the volume root is pinned with the same access;
locations permitting traversal or writing but not directory listing can now fail
closed. There is no attributes-only fallback. Ordinary file access is unchanged.
This prevents directory rename/delete while held, not changes to directory contents:
child creation and atomic replacement remain allowed. Ordinary source file pins
retain read-only sharing. Sources still require their own pinned file handles
and publication-time verification. A dedicated no-list ACL fixture has not yet
been validated; permission compatibility must not be inferred from rename tests.

## Synthetic evidence

- `application::publication_context::tests`: explicit expected-runtime parameters without
  environment mutation; missing, matching and changed config; missing ancestor
  appearance; protection of future config and input.
- `application::image_publication::publication_tests`: protected outputs across single, album,
  mirror and fixed-runtime task entry points; exact-byte normal/default output;
  missing-account refusal for explicit paths; config appearance before final publication keeps
  previous output; source/config write-delete-rename attempts are denied; stale
  task runtime is rejected; a material revision change before publication keeps
  the previous target and preserves the already published batch prefix.
- `adapters::wechat::media::image_batch::tests`: layout and suffix compatibility,
  batch-only missing-key skipping, malformed keyed rejection and legacy XOR.
- Existing image parser fixtures and encrypted/parity tests are retained.

See [test instructions](../tests/README.md) for execution. Synthetic coverage
does not replace real-account or media-quality verification.
