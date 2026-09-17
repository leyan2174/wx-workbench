# Image publication boundary

The image entry points share `application::publication_context::PublicationContext` and `ExportTarget`.
`decode_images_for` accepts the task host's selected `RuntimeContext`; it does
not rediscover configuration. The ordinary entries use `PublicationContext::current`.
When `WX_CLI_EXPECTED_RUNTIME` is present, current configuration must resolve to
that runtime ID, matching the worker's expected-account check. Missing or
changed accounts cannot fall back to offline execution.

Existing configuration is held by `ConfigPin`. Publication protects the config,
keys, key store and associated key/lock files, database, decrypted cache, runtime
directory and input. Image source handles and parent guards remain pinned while
decoding and publishing. The final checked-publication callback revalidates the
configuration and source before replacing the destination.

Explicit album input/output and explicit AES remain usable without configuration
when no expected runtime is supplied. In this case the future config path and
input are protected, and the nearest existing config ancestor is pinned. Any
previously absent config path or ancestor appearing makes verification fail.
An invalid existing config never selects offline mode. Account-derived defaults
still require configuration.

`adapters/wechat/media/image_batch.rs` owns DAT dispatch policy, `_t`/`_h`
normalization, account attachment layout and the album four-component rule.
It delegates decoding to the existing codec. Missing-key V2 skipping remains a
batch-only compatibility policy; single decode and keyed malformed input fail.
Album and mirror existing-output policies remain distinct. This does not relax
the separate strict attachment resolver or turn legacy results into hash proofs.

## Synthetic evidence

- `application::publication_context::tests`: explicit expected-runtime parameters without
  environment mutation; missing, matching and changed config; missing ancestor
  appearance; protection of future config and input.
- `application::image_publication::publication_tests`: protected outputs across single, album,
  mirror and fixed-runtime task entry points; exact-byte normal/default output;
  offline explicit paths/AES; config appearance before final publication keeps
  previous output; source/config write-delete-rename attempts are denied; stale
  task runtime is rejected.
- `adapters::wechat::media::image_batch::tests`: layout and suffix compatibility,
  batch-only missing-key skipping, malformed keyed rejection and legacy XOR.
- Existing image parser fixtures and encrypted/parity tests are retained.

See [test instructions](../tests/README.md) for execution. Synthetic coverage
does not replace real-account or media-quality verification.
