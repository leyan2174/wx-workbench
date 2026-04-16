use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

/// 扫描到的一条密钥记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyEntry {
    /// 相对路径，如 "message/message_0.db"
    pub db_name: String,
    /// 32字节 AES 密钥（hex）
    pub enc_key: String,
    /// 16字节 salt（hex，来自数据库文件头）
    pub salt: String,
}

/// 从进程内存中扫描所有 SQLCipher 密钥
///
/// 需要以 root/Administrator 权限运行
pub fn scan_keys(db_dir: &Path) -> Result<Vec<KeyEntry>> {
    #[cfg(target_os = "macos")]
    return macos::scan_keys(db_dir);
    #[cfg(target_os = "linux")]
    return linux::scan_keys(db_dir);
    #[cfg(target_os = "windows")]
    return windows::scan_keys(db_dir);
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        anyhow::bail!("当前平台不支持自动密钥扫描")
    }
}

/// 读取 DB 文件前 16 字节作为 salt（hex），如果是明文 SQLite 则返回 None
pub fn read_db_salt(path: &Path) -> Option<String> {
    let mut buf = [0u8; 16];
    let mut f = std::fs::File::open(path).ok()?;
    use std::io::Read;
    f.read_exact(&mut buf).ok()?;
    // 明文 SQLite：头部是 "SQLite format 3"
    if &buf[..15] == b"SQLite format 3" {
        return None;
    }
    Some(hex::encode(&buf))
}

/// 遍历 db_dir，收集所有 .db 文件的 salt -> 相对路径 映射
pub fn collect_db_salts(db_dir: &Path) -> Vec<(String, String)> {
    let mut result = Vec::new();
    collect_recursive(db_dir, db_dir, &mut result);
    result
}

fn collect_recursive(base: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_recursive(base, &path, out);
        } else if path.extension().map(|e| e == "db").unwrap_or(false) {
            if let Some(salt) = read_db_salt(&path) {
                if let Ok(rel) = path.strip_prefix(base) {
                    let rel_str = rel.to_string_lossy().replace('\\', "/");
                    out.push((salt, rel_str));
                }
            }
        }
    }
}

// hex encoding helper (avoid adding hex crate by implementing inline)
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}
