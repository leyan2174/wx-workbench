# Third-party notices

## jackwener/wx-cli

This project includes code derived from jackwener/wx-cli, covered by the
following MIT notice. The project root license does not replace this notice.

MIT License

Copyright (c) 2026 jackwener

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## ylytdeng/wechat-decrypt

Batch export, Moments and media workflows include implementations adapted
from ylytdeng/wechat-decrypt. Available source evidence does not establish
an upstream license grant. This project's Apache-2.0 license does not grant
redistribution rights over upstream-derived expression. Permission evidence
or an appropriate implementation review and replacement is required before
approving public source distribution.

## Release scope

The clean source snapshot is an engineering candidate, not a statement that
all third-party permission questions are resolved. Dependency packages retain
their own licenses; a source snapshot is not a bundled binary release. Web
Lucide assets retain `src/web/assets/LICENSE-lucide.txt`, including the Lucide
ISC license and the MIT license for Feather-derived icons. Release staging
copies this complete file to `LICENSE-lucide.txt` beside the GitHub binary
and into both npm packages. Synthetic test
audio has generation parameters and hashes in `tests/fixtures/audio/manifest.json`.


## LOGO127/wechat-ai-memory

The attribution below is retained for this adapter. An independently
verified upstream license file is not available in the local evidence.
Confirm the original grant and copyright attribution before approving
public redistribution; this notice alone does not establish permission.

`src/scanner/windows/account_hook.js` adapts the SHA-512/HMAC capture approach
from https://github.com/LOGO127/wechat-ai-memory, specifically
`src/wechat_context_exporter/sources/wechat4_key_capture.py`.
The account-key derivation follows the same SQLCipher parameters as
`src/wechat_context_exporter/sources/wechat4_crypto.py`.

## Frida

The Windows account-key provider links Frida through `frida`/`frida-sys`.
Frida's native code and Rust bindings retain their upstream licenses,
including the wxWindows Library Licence. See https://github.com/frida/frida
and https://github.com/frida/frida-rust. The hook runs in Frida's embedded
JavaScript engine; it does not require Node.js or Python at runtime.

## SNS WxIsaac64 WASM asset

The SNS media adapter can use a fixed-hash `wasm_video_decode.wasm` binary.
Its recorded source lineage is the
WxIsaac64 media flow associated with hicccc77/WeFlow and
LifeArchiveProject/WeChatDataAnalysis. The copy imported into this repository
did not include a license or copyright notice, so this project's Apache-2.0
license must not be assumed to cover that binary. It is excluded from the
approved public source and binary release candidates. Public builds do not
download or silently discover it; encrypted single-video decoding requires an
explicit, authorized local file with the expected SHA-256. The internal
verification feature and local audit copy do not grant redistribution rights.
