//! V2 image AES key 提取 — 平台相关。
//!
//! ⚠️ 此模块由 codex 落地。本文件只放公共 trait + 平台 dispatch 占位。
//!
//! 路径：
//! - macOS：磁盘派生（`key_<uin>_*.statistic` 文件名拿 uin → `md5(str(uin) + wxid)[:16]`）
//!   + brute-force fallback（`md5(str(uin))[:4] == wxid_suffix` 枚举 2^24）
//! - Windows：扫 `Weixin.exe` 内存，匹配 `[a-zA-Z0-9]{32}` 候选，按已知 AES ciphertext-block
//!   反验（`find_image_key.py` / `find_image_key.c` 已写实）
//! - Linux：上游空白；当前不实现，遇到 V2 .dat 返回 unsupported 错误

#[allow(dead_code)]
pub mod macos;
#[allow(dead_code)]
pub mod windows;

use anyhow::Result;

/// 单个 wxid 的 V2 image key 提取接口。
///
/// 实现者负责跨调用缓存（一台机器上同一 wxid 的 image key 在微信不重启时是稳定的）。
pub trait ImageKeyProvider {
    /// 返回当前 wxid 的 16 字节 AES key。失败要带可执行的诊断（例如「macOS 没找到
    /// kvcomm cache，请确认微信已登录」/「Windows 进程不在跑」）。
    fn get_aes_key(&self, wxid: &str) -> Result<[u8; 16]>;
}

/// 平台默认实现（codex 后续填）。
///
/// 调用方目前可以直接传 `None`，让 resolver 在遇到 V2 .dat 时报「image key 未提取」错。
pub fn default_provider() -> Option<Box<dyn ImageKeyProvider + Send + Sync>> {
    // TODO(codex): 按 cfg(target_os) 返回 macOS / Windows / 不支持
    None
}
