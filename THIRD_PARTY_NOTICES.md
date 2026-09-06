# Third-party notices

## Account key capture

`src/scanner/windows/account_hook.js` adapts the SHA-512/HMAC capture approach
from https://github.com/LOGO127/wechat-ai-memory, specifically
`src/wechat_context_exporter/sources/wechat4_key_capture.py`.
The account-key derivation follows the same SQLCipher parameters as
`src/wechat_context_exporter/sources/wechat4_crypto.py`.

MIT License

Copyright (c) 2026 WeChat AI Memory contributors

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

## Frida

The Windows account-key provider links Frida through `frida`/`frida-sys`.
Frida's native code and Rust bindings retain their upstream licenses,
including the wxWindows Library Licence. See https://github.com/frida/frida
and https://github.com/frida/frida-rust. The hook runs in Frida's embedded
JavaScript engine; it does not require Node.js or Python at runtime.
