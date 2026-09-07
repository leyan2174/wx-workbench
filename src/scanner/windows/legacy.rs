//! 微信 4.1.10 之前的原始十六进制密钥扫描，复用固定库存中的首页验证候选。

use anyhow::Result;
use std::collections::BTreeSet;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Memory::{
    VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOCACHE, PAGE_READWRITE, PAGE_WRITECOMBINE,
    PAGE_WRITECOPY,
};
use zeroize::Zeroizing;

use super::super::KeyEntry;
use super::config_cipher::DbPage;

const MAX_HEX_LEN: usize = 192;
const CHUNK_SIZE: usize = 2 * 1024 * 1024;

struct Candidate {
    key: Zeroizing<[u8; 32]>,
    salt: Option<[u8; 16]>,
}

/// 只使用调用方固定的首页，不枚举目录或按路径重开数据库。
pub(super) fn scan(process: HANDLE, pages: &[DbPage]) -> Result<Vec<KeyEntry>> {
    eprintln!("扫描进程内存...");
    let raw_keys = scan_memory(process)?;
    eprintln!("找到 {} 个候选密钥", raw_keys.len());
    let entries = verify_candidates(&raw_keys, pages);
    eprintln!("已验证 {}/{} 个数据库", entries.len(), pages.len());
    Ok(entries)
}

fn verify_candidates(candidates: &[Candidate], pages: &[DbPage]) -> Vec<KeyEntry> {
    let mut remaining: BTreeSet<String> = pages.iter().map(|db| db.db_name.clone()).collect();
    let mut entries = Vec::new();
    let mut verified_keys = Vec::new();
    for candidate in candidates {
        if verify_key(
            &candidate.key,
            candidate.salt.as_ref(),
            pages,
            &mut remaining,
            &mut entries,
        ) > 0
        {
            verified_keys.push(&candidate.key);
        }
    }
    // 和旧脚本一致：仅用已经通过首页 HMAC 的钥交叉验证其余数据库。
    for key in verified_keys {
        verify_key(key, None, pages, &mut remaining, &mut entries);
    }
    entries
}

fn verify_key(
    key: &[u8; 32],
    salt: Option<&[u8; 16]>,
    pages: &[DbPage],
    remaining: &mut BTreeSet<String>,
    entries: &mut Vec<KeyEntry>,
) -> usize {
    let before = entries.len();
    for db in pages {
        if !remaining.contains(&db.db_name) {
            continue;
        }
        let Some(page) = db.page1_variants.iter().find(|page| {
            salt.is_none_or(|salt| page.get(..16) == Some(salt.as_slice()))
                && crate::crypto::verify_page1(key, page)
        }) else {
            continue;
        };
        remaining.remove(&db.db_name);
        entries.push(KeyEntry {
            db_name: db.db_name.clone(),
            enc_key: super::super::hex::encode(key),
            salt: super::super::hex::encode(&page[..16]),
        });
    }
    entries.len() - before
}

fn scan_memory(process: HANDLE) -> Result<Vec<Candidate>> {
    let mut results = Vec::new();
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

fn scan_region(process: HANDLE, base: usize, size: usize, results: &mut Vec<Candidate>) {
    let overlap = MAX_HEX_LEN + 3;
    let mut offset = 0usize;

    loop {
        if offset >= size {
            break;
        }
        let chunk_size = std::cmp::min(CHUNK_SIZE, size - offset);
        let addr = base + offset;
        let mut buf = Zeroizing::new(vec![0u8; chunk_size]);
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
            search_pattern(&buf[..bytes_read.min(buf.len())], results);
        }

        if chunk_size > overlap {
            offset += chunk_size - overlap;
        } else {
            offset += chunk_size;
        }
    }
}

fn search_pattern(buf: &[u8], results: &mut Vec<Candidate>) {
    let mut i = 0;
    while i + 67 <= buf.len() {
        if buf[i] != b'x' || buf[i + 1] != b'\'' {
            i += 1;
            continue;
        }
        let hex_start = i + 2;
        let length = buf[hex_start..]
            .iter()
            .take(MAX_HEX_LEN + 1)
            .take_while(|c| c.is_ascii_hexdigit())
            .count();
        // 旧脚本支持纯 64hex，或 96..192 的偶数串（首 64 为钥，末 32 为 salt）。
        if !(length == 64 || (96..=MAX_HEX_LEN).contains(&length) && length % 2 == 0)
            || buf.get(hex_start + length) != Some(&b'\'')
        {
            i += 1;
            continue;
        }
        let candidate = Candidate {
            key: Zeroizing::new(decode_hex(&buf[hex_start..hex_start + 64])),
            salt: (length >= 96)
                .then(|| decode_hex(&buf[hex_start + length - 32..hex_start + length])),
        };
        if !results.iter().any(|existing| {
            existing.key[..] == candidate.key[..] && existing.salt == candidate.salt
        }) {
            results.push(candidate);
        }
        i = hex_start + length + 1;
    }
}

/// 调用方已校验十六进制字符与精确长度。
fn decode_hex<const N: usize>(input: &[u8]) -> [u8; N] {
    let mut output = [0; N];
    for (byte, pair) in output.iter_mut().zip(input.chunks_exact(2)) {
        *byte = ((pair[0] as char).to_digit(16).unwrap() * 16
            + (pair[1] as char).to_digit(16).unwrap()) as u8;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::{Hmac, Mac};
    use sha2::Sha512;

    fn page(key: &[u8; 32], salt: u8) -> Vec<u8> {
        let mut data = vec![0x55; crate::crypto::PAGE_SZ];
        data[..16].fill(salt);
        let mut mac_key = [0u8; 32];
        pbkdf2::pbkdf2_hmac::<Sha512>(key, &[salt ^ 0x3a; 16], 2, &mut mac_key);
        let mut mac = Hmac::<Sha512>::new_from_slice(&mac_key).unwrap();
        mac.update(&data[16..4032]);
        mac.update(&1u32.to_le_bytes());
        data[4032..].copy_from_slice(&mac.finalize().into_bytes());
        data
    }

    fn database(name: &str, key: &[u8; 32], salt: u8) -> DbPage {
        DbPage {
            db_name: name.into(),
            page1_variants: vec![page(key, salt)],
        }
    }

    fn literal(key: &[u8; 32], salt: u8, length: usize) -> Vec<u8> {
        let mut hex = super::super::super::hex::encode(key);
        if length >= 96 {
            hex.push_str(&"a".repeat(length - 96));
            hex.push_str(&super::super::super::hex::encode(&[salt; 16]));
        }
        format!("x'{hex}'").into_bytes()
    }

    fn decode_candidates(bytes: &[u8]) -> Vec<Candidate> {
        let mut candidates = Vec::new();
        search_pattern(bytes, &mut candidates);
        candidates
    }

    #[test]
    fn legacy_64_96_and_long_candidates_require_correct_hmac() {
        let key = [0x23; 32];
        let pages = vec![database("a.db", &key, 0x41)];
        for length in [64, 96, 98, 128, 192] {
            let candidates = decode_candidates(&literal(&key, 0x41, length));
            assert_eq!(candidates.len(), 1);
            let entries = verify_candidates(&candidates, &pages);
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].db_name, "a.db");
            assert_eq!(entries[0].enc_key, super::super::super::hex::encode(&key));
            let wrong = decode_candidates(&literal(&[0x99; 32], 0x41, length));
            assert!(verify_candidates(&wrong, &pages).is_empty());
        }
    }

    #[test]
    fn legacy_rejects_malformed_and_unsupported_literals() {
        for length in [0, 62, 65, 66, 94, 95, 97, 191, 193, 194, 256] {
            assert!(decode_candidates(format!("x'{}'", "a".repeat(length)).as_bytes()).is_empty());
        }
        assert!(decode_candidates(format!("x'{}z'", "a".repeat(95)).as_bytes()).is_empty());
        assert!(decode_candidates(format!("x'{}", "a".repeat(96)).as_bytes()).is_empty());
    }

    #[test]
    fn legacy_same_salt_expands_all_verified_databases_once() {
        let key = [0x23; 32];
        let mut bytes = literal(&key, 0x41, 96);
        bytes.extend(literal(&key, 0x41, 96));
        let candidates = decode_candidates(&bytes);
        assert_eq!(candidates.len(), 1);
        let pages = vec![
            database("a.db", &key, 0x41),
            database("b.db", &key, 0x41),
            database("a.db", &key, 0x41),
            database("wrong.db", &[0x99; 32], 0x41),
        ];
        let entries = verify_candidates(&candidates, &pages);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.db_name.as_str())
                .collect::<Vec<_>>(),
            ["a.db", "b.db"]
        );
    }

    #[test]
    fn legacy_verified_key_cross_checks_other_salts() {
        let key = [0x23; 32];
        let candidates = decode_candidates(&literal(&key, 0x41, 96));
        let pages = vec![database("a.db", &key, 0x41), database("b.db", &key, 0x42)];
        let entries = verify_candidates(&candidates, &pages);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].salt, "42".repeat(16));
        // 无法直接验证的带 salt 候选不能充当交叉验证的已知钥。
        let unrelated = decode_candidates(&literal(&key, 0x43, 96));
        assert!(verify_candidates(&unrelated, &pages).is_empty());
    }

    #[test]
    fn legacy_uses_verified_wal_variant_from_the_same_inventory() {
        let key = [0x23; 32];
        let candidates = decode_candidates(&literal(&key, 0x41, 96));
        let pages = vec![DbPage {
            db_name: "a.db".into(),
            page1_variants: vec![page(&[0x99; 32], 0x41), page(&key, 0x41)],
        }];
        assert_eq!(verify_candidates(&candidates, &pages).len(), 1);
    }
}
