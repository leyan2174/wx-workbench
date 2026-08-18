//! The original raw `x'<key><salt>'` scanner for WeChat versions before 4.1.10.
//!
//! Keep this provider behaviorally aligned with the pre-existing Windows
//! scanner. New-version Config.Cipher logic belongs in `config_cipher.rs`.

use anyhow::Result;
use std::path::Path;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Memory::{
    VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOCACHE, PAGE_READWRITE, PAGE_WRITECOMBINE,
    PAGE_WRITECOPY,
};

use super::super::{collect_db_salts, KeyEntry};

const HEX_PATTERN_LEN: usize = 96;
const CHUNK_SIZE: usize = 2 * 1024 * 1024;

pub(super) fn scan(process: HANDLE, db_dir: &Path) -> Result<Vec<KeyEntry>> {
    let db_salts = collect_db_salts(db_dir);
    eprintln!("找到 {} 个加密数据库", db_salts.len());

    eprintln!("扫描进程内存...");
    let raw_keys = scan_memory(process)?;
    eprintln!("找到 {} 个候选密钥", raw_keys.len());

    let mut entries = Vec::new();
    for (key_hex, salt_hex) in &raw_keys {
        for (db_salt, db_name) in &db_salts {
            if salt_hex == db_salt {
                entries.push(KeyEntry {
                    db_name: db_name.clone(),
                    enc_key: key_hex.clone(),
                    salt: salt_hex.clone(),
                });
                break;
            }
        }
    }
    eprintln!("匹配到 {}/{} 个密钥", entries.len(), raw_keys.len());
    Ok(entries)
}

fn scan_memory(process: HANDLE) -> Result<Vec<(String, String)>> {
    let mut results: Vec<(String, String)> = Vec::new();
    let mut addr: usize = 0;

    loop {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        let ret = unsafe {
            VirtualQueryEx(
                process,
                Some(addr as *const _),
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if ret == 0 {
            break;
        }

        let region_size = mbi.RegionSize;
        let base = mbi.BaseAddress as usize;
        if mbi.State == MEM_COMMIT && is_writable_readable_page(mbi.Protect.0) {
            scan_region(process, base, region_size, &mut results);
        }

        addr = base.saturating_add(region_size);
        if addr == 0 {
            break;
        }
    }

    Ok(results)
}

fn is_writable_readable_page(protect: u32) -> bool {
    let base = protect & !(PAGE_GUARD.0 | PAGE_NOCACHE.0 | PAGE_WRITECOMBINE.0);
    matches!(
        base,
        x if x == PAGE_READWRITE.0
            || x == PAGE_WRITECOPY.0
            || x == PAGE_EXECUTE_READWRITE.0
            || x == PAGE_EXECUTE_WRITECOPY.0
    )
}

fn scan_region(process: HANDLE, base: usize, size: usize, results: &mut Vec<(String, String)>) {
    let overlap = HEX_PATTERN_LEN + 3;
    let mut offset = 0usize;

    loop {
        if offset >= size {
            break;
        }
        let chunk_size = std::cmp::min(CHUNK_SIZE, size - offset);
        let addr = base + offset;
        let mut buf = vec![0u8; chunk_size];
        let mut bytes_read: usize = 0;

        let ok = unsafe {
            ReadProcessMemory(
                process,
                addr as *const _,
                buf.as_mut_ptr() as *mut _,
                chunk_size,
                Some(&mut bytes_read),
            )
            .is_ok()
        };

        if ok && bytes_read > 0 {
            buf.truncate(bytes_read);
            search_pattern(&buf, results);
        }

        if chunk_size > overlap {
            offset += chunk_size - overlap;
        } else {
            offset += chunk_size;
        }
    }
}

#[inline]
fn is_hex_char(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

fn search_pattern(buf: &[u8], results: &mut Vec<(String, String)>) {
    let total = HEX_PATTERN_LEN + 3;
    if buf.len() < total {
        return;
    }
    let mut i = 0;
    while i + total <= buf.len() {
        if buf[i] != b'x' || buf[i + 1] != b'\'' {
            i += 1;
            continue;
        }
        let hex_start = i + 2;
        let all_hex = buf[hex_start..hex_start + HEX_PATTERN_LEN]
            .iter()
            .all(|&c| is_hex_char(c));
        if !all_hex {
            i += 1;
            continue;
        }
        if buf[hex_start + HEX_PATTERN_LEN] != b'\'' {
            i += 1;
            continue;
        }
        let key_hex = String::from_utf8_lossy(&buf[hex_start..hex_start + 64]).to_lowercase();
        let salt_hex =
            String::from_utf8_lossy(&buf[hex_start + 64..hex_start + 96]).to_lowercase();
        let is_dup = results.iter().any(|(k, s)| k == &key_hex && s == &salt_hex);
        if !is_dup {
            results.push((key_hex, salt_hex));
        }
        i += total;
    }
}
