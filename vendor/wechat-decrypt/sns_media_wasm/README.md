# SNS video WASM assets

These files provide the `WxIsaac64` keystream used by WeChat Moments video
media. The implementation follows the SNS media flow used by
[WeFlow](https://github.com/hicccc77/WeFlow) and
[WeChatDataAnalysis](https://github.com/LifeArchiveProject/WeChatDataAnalysis):
only the first 128 KiB of a downloaded video is XOR-decoded.

Keep these files together:

- `wasm_video_decode.js`
- `wasm_video_decode.wasm`
- `weflow_wasm_keystream.js`

`export_sns_album.py` starts the wrapper once in `--stdio` mode and reuses it
for every encrypted video in an export. Node.js is required at runtime.

Quick local check:

```powershell
node .\weflow_wasm_keystream.js 1 16
```

The command prints a 16-byte keystream encoded as base64.
