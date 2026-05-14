//! macOS V2 image AES key 提取。
//!
//! 主路径：从 `~/Library/Containers/com.tencent.xinWeChat/Data/Documents/key_<uin>_*.statistic`
//! 文件名拿 uin，然后 `md5(str(uin) + sanitize(wxid)).hex()[:16]` 派生 AES key。
//!
//! Fallback：枚举 uin 候选 2^24 个（`uint32`，但 wxid 4-byte 前缀只看后 24 bit），
//! 通过 `md5(str(uin))[:4] == wxid 后 4 字节` 匹配。
//! 上游 `find_image_key_macos.py` 实测 1-2s 完成。
//!
//! ⚠️ codex 落实现。
