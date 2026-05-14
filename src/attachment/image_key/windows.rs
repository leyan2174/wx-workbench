//! Windows V2 image AES key 提取。
//!
//! 扫 `Weixin.exe` 进程内存，匹配模式 `(?<![a-zA-Z0-9])[a-zA-Z0-9]{32}(?![a-zA-Z0-9])`
//! 取候选 key，然后用已知 AES ciphertext-block 反验：每个 candidate 用 AES-128-ECB
//! 解一段已知 ciphertext，看产物是否落在合理图片 magic 上。
//!
//! 上游 `find_image_key.py` / `find_image_key.c` 已经把 signature scan + false-positive
//! 控制写实，可以直接对照。
//!
//! ⚠️ codex 落实现。
