# SNS media WASM asset

This directory contains a local audit copy of `wasm_video_decode.wasm`. It is
not embedded by default and must not be included in the public source or binary
release candidate.

- SHA-256: `dca796bacec37d8522c7983b3945e5d579bd74164e3b21f0ebc773be6dfc8b6e`
- Size: `3,785,516` bytes
- Recorded lineage: hicccc77/WeFlow and
  LifeArchiveProject/WeChatDataAnalysis WxIsaac64 media flow

The historical source snapshot from which this file was imported did not
include a license or copyright notice for the binary. The repository's
Apache-2.0 license therefore must not be assumed to cover this asset.
Redistributors must independently confirm permission to distribute or use it.
The `sns-wasm-test-asset` feature exists only to verify an authorized local
copy; enabling the feature does not grant redistribution rights.

The Rust host accepts only this exact hash. Replacing the binary requires a new
ABI review, resource-limit review, synthetic vector generation, and regression
test run.
