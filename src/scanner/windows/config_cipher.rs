//! Read-only Windows 4.1+ Config.Cipher scanner.
//!
//! WeChat keeps a Config.Cipher object in memory. The object is reached through
//! the `com.Tencent.WCDB.Config.Cipher` string object and contains a short XOR
//! encoded blob with `x'<key><salt>'` material.

use anyhow::Result;
use hmac::{Hmac, Mac};
use pbkdf2::pbkdf2_hmac;
use sha2::Sha512;
use std::collections::{BTreeSet, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Memory::{
    VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE, PAGE_EXECUTE_READ,
    PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOCACHE, PAGE_READONLY,
    PAGE_READWRITE, PAGE_WRITECOMBINE, PAGE_WRITECOPY,
};

use super::super::KeyEntry;

const NEEDLE: &[u8] = b"com.Tencent.WCDB.Config.Cipher";
const XOR_MASK: [u8; 32] = [
    0xd2, 0xc7, 0x44, 0x24, 0x58, 0x02, 0x00, 0x00, 0x00, 0x48, 0x89, 0x44, 0x24, 0x50, 0x48, 0x8b,
    0x45, 0x00, 0x48, 0x84, 0x4c, 0x24, 0x48, 0x48, 0x89, 0x44, 0x25, 0x40, 0x48, 0x58, 0x4c, 0x24,
];
const MAX_USER_ADDRESS: u64 = 0x0000_8000_0000_0000;
const MAX_BLOB_SIZE: usize = 1024;
const CHUNK_SIZE: usize = 2 * 1024 * 1024;
const PAGE_SIZE: usize = 4096;
const KEY_SIZE: usize = 32;
const SALT_SIZE: usize = 16;

pub(super) struct DbPage {
    pub(super) db_name: String,
    pub(super) page1_variants: Vec<Vec<u8>>,
}

#[derive(Default)]
struct Stats {
    needle_occurrences: usize,
    string_object_refs: usize,
    node_read_failures: usize,
    node_address_mismatches: usize,
    node_length_mismatches: usize,
    node_candidates: usize,
    blob_count: usize,
    candidate_count: usize,
    verified_count: usize,
}

pub(super) fn scan(
    process: HANDLE,
    db_dir: &Path,
) -> Result<Vec<KeyEntry>> {
    let db_pages = collect_db_pages(db_dir)?;
    let regions = enum_readable_regions(process);
    let needle_addresses = find_bytes_in_regions(process, &regions, NEEDLE);
    let mut stats = Stats {
        needle_occurrences: needle_addresses.len(),
        ..Stats::default()
    };

    if needle_addresses.is_empty() {
        eprintln!("Config.Cipher: 未找到字符串特征");
        return Ok(Vec::new());
    }

    let mut patterns = Vec::with_capacity(needle_addresses.len());
    for address in &needle_addresses {
        let mut pattern = Vec::with_capacity(16);
        pattern.extend_from_slice(&address.to_le_bytes());
        pattern.extend_from_slice(&(NEEDLE.len() as u64).to_le_bytes());
        patterns.push(pattern);
    }

    let mut seen_candidates = HashSet::new();
    let mut attempted_databases = HashSet::new();
    let mut remaining_databases: HashSet<String> =
        db_pages.iter().map(|db| db.db_name.clone()).collect();
    let mut entries = Vec::new();

    scan_regions(process, &regions, 0x80, |base, data| {
        if remaining_databases.is_empty() {
            return;
        }
        for pattern in &patterns {
            let mut position = 0usize;
            while let Some(relative) = data[position..]
                .windows(pattern.len())
                .position(|window| window == pattern)
            {
                let offset = position + relative;
                position = offset + 1;
                stats.string_object_refs += 1;

                let qaddr = base.saturating_add(offset);
                let Some(node_base) = qaddr.checked_sub(0x10) else {
                    continue;
                };
                let Some(node) = read_memory(process, node_base, 0x50) else {
                    stats.node_read_failures += 1;
                    continue;
                };
                let Some(node_string_ptr) = read_u64(&node, 0x10) else {
                    stats.node_address_mismatches += 1;
                    continue;
                };
                if !needle_addresses.contains(&node_string_ptr) {
                    stats.node_address_mismatches += 1;
                    continue;
                }
                if read_u64(&node, 0x18) != Some(NEEDLE.len() as u64) {
                    stats.node_length_mismatches += 1;
                    continue;
                }

                let Some(config_ptr) = read_u64(&node, 0x28) else {
                    continue;
                };
                if !(0x1_0000..MAX_USER_ADDRESS).contains(&config_ptr) {
                    continue;
                }
                stats.node_candidates += 1;

                let Some(object) =
                    read_memory(process, config_ptr.saturating_add(0x88) as usize, 0x28)
                else {
                    continue;
                };
                let Some(data_ptr) = read_u64(&object, 0x08) else {
                    continue;
                };
                let Some(data_len) = read_u64(&object, 0x10).map(|value| value as usize) else {
                    continue;
                };
                if data_len == 0
                    || data_len > MAX_BLOB_SIZE
                    || !(0x1_0000..MAX_USER_ADDRESS).contains(&data_ptr)
                {
                    continue;
                }
                let Some(blob) = read_memory(process, data_ptr as usize, data_len) else {
                    continue;
                };
                if data_len != 99 {
                    continue;
                }
                stats.blob_count += 1;

                for candidate in decode_candidates(&blob) {
                    if !seen_candidates.insert(candidate.clone()) {
                        continue;
                    }
                    stats.candidate_count += 1;
                    let matched = verify_candidate(
                        &candidate,
                        &db_pages,
                        &mut remaining_databases,
                        &mut attempted_databases,
                        &mut entries,
                    );
                    stats.verified_count += matched;
                }
            }
        }
    });

    eprintln!(
        "Config.Cipher: needles={}, refs={}, node_read_failures={}, node_address_mismatches={}, node_length_mismatches={}, nodes={}, blobs={}, candidates={}, verified={}",
        stats.needle_occurrences,
        stats.string_object_refs,
        stats.node_read_failures,
        stats.node_address_mismatches,
        stats.node_length_mismatches,
        stats.node_candidates,
        stats.blob_count,
        stats.candidate_count,
        stats.verified_count
    );
    let missing = db_pages
        .iter()
        .filter(|db| remaining_databases.contains(&db.db_name))
        .map(|db| {
            let status = if attempted_databases.contains(&db.db_name) {
                "candidate attempted, HMAC failed"
            } else {
                "no candidate targeted"
            };
            format!("{} [{}]", db.db_name, status)
        })
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        eprintln!("Config.Cipher 未验证数据库: {}", missing.join(", "));
    }
    Ok(entries)
}

fn enum_readable_regions(process: HANDLE) -> Vec<(usize, usize)> {
    let mut regions = Vec::new();
    let mut address = 0usize;
    loop {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        let ret = unsafe {
            VirtualQueryEx(
                process,
                Some(address as *const _),
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if ret == 0 {
            break;
        }
        let base = mbi.BaseAddress as usize;
        let size = mbi.RegionSize;
        if mbi.State == MEM_COMMIT
            && size > 0
            && size < 500 * 1024 * 1024
            && is_readable(mbi.Protect.0)
        {
            regions.push((base, size));
        }
        let next = base.saturating_add(size);
        if next <= address {
            break;
        }
        address = next;
    }
    regions
}

fn is_readable(protect: u32) -> bool {
    if protect & PAGE_GUARD.0 != 0 {
        return false;
    }
    let base = protect & !(PAGE_NOCACHE.0 | PAGE_WRITECOMBINE.0);
    matches!(
        base,
        x if x == PAGE_READONLY.0
            || x == PAGE_READWRITE.0
            || x == PAGE_WRITECOPY.0
            || x == PAGE_EXECUTE.0
            || x == PAGE_EXECUTE_READ.0
            || x == PAGE_EXECUTE_READWRITE.0
            || x == PAGE_EXECUTE_WRITECOPY.0
    )
}

fn find_bytes_in_regions(
    process: HANDLE,
    regions: &[(usize, usize)],
    needle: &[u8],
) -> BTreeSet<u64> {
    let mut addresses = BTreeSet::new();
    scan_regions(
        process,
        regions,
        needle.len().saturating_sub(1),
        |base, data| {
            let mut position = 0usize;
            while let Some(relative) = data[position..]
                .windows(needle.len())
                .position(|window| window == needle)
            {
                let offset = position + relative;
                addresses.insert(base.saturating_add(offset) as u64);
                position = offset + 1;
            }
        },
    );
    addresses
}

fn scan_regions<F>(process: HANDLE, regions: &[(usize, usize)], overlap: usize, mut callback: F)
where
    F: FnMut(usize, &[u8]),
{
    for &(base, size) in regions {
        let mut offset = 0usize;
        let mut tail = Vec::new();
        let mut tail_base = base;
        while offset < size {
            let current_size = std::cmp::min(CHUNK_SIZE, size - offset);
            let current_base = base.saturating_add(offset);
            let chunk = read_memory(process, current_base, current_size).unwrap_or_default();
            let data_base = if tail.is_empty() {
                current_base
            } else {
                tail_base
            };
            let mut data = Vec::with_capacity(tail.len() + chunk.len());
            data.extend_from_slice(&tail);
            data.extend_from_slice(&chunk);
            if !data.is_empty() {
                callback(data_base, &data);
                let keep = std::cmp::min(overlap, data.len());
                tail_base = data_base + data.len() - keep;
                tail = data[data.len() - keep..].to_vec();
            } else {
                tail.clear();
            }
            offset = offset.saturating_add(current_size);
        }
    }
}

fn read_memory(process: HANDLE, address: usize, size: usize) -> Option<Vec<u8>> {
    let mut buffer = vec![0u8; size];
    let mut bytes_read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            process,
            address as *const _,
            buffer.as_mut_ptr() as *mut _,
            size,
            Some(&mut bytes_read),
        )
        .is_ok()
    };
    if ok && bytes_read == size {
        Some(buffer)
    } else if ok && bytes_read > 0 {
        buffer.truncate(bytes_read);
        Some(buffer)
    } else {
        None
    }
}

fn read_u64(data: &[u8], offset: usize) -> Option<u64> {
    let bytes: [u8; 8] = data.get(offset..offset + 8)?.try_into().ok()?;
    Some(u64::from_le_bytes(bytes))
}

fn decode_candidates(blob: &[u8]) -> Vec<Vec<u8>> {
    let decoded: Vec<u8> = blob
        .iter()
        .enumerate()
        .map(|(index, value)| value ^ XOR_MASK[index % XOR_MASK.len()])
        .collect();
    let mut result = Vec::new();
    let mut position = 0usize;
    while position + 2 <= decoded.len() {
        if decoded[position] != b'x' && decoded[position] != b'X' || decoded[position + 1] != b'\''
        {
            position += 1;
            continue;
        }
        let start = position + 2;
        let Some(end_relative) = decoded[start..].iter().position(|byte| *byte == b'\'') else {
            break;
        };
        let end = start + end_relative;
        let run = &decoded[start..end];
        if (64..=192).contains(&run.len())
            && run.len().is_multiple_of(2)
            && run.iter().all(|byte| byte.is_ascii_hexdigit())
        {
            let mut starts = vec![0usize];
            if run.len() > 96 {
                starts.extend((0..=run.len() - 64).step_by(32));
                starts.push(run.len() - 64);
            }
            for candidate_start in starts {
                if candidate_start + 64 > run.len() {
                    continue;
                }
                let key_hex = &run[candidate_start..candidate_start + 64];
                let Some(key) = decode_hex(key_hex) else {
                    continue;
                };
                if !result.contains(&key) {
                    result.push(key);
                }
            }
        }
        position = end + 1;
    }
    result
}

fn verify_candidate(
    key: &[u8],
    db_pages: &[DbPage],
    remaining_databases: &mut HashSet<String>,
    attempted_databases: &mut HashSet<String>,
    entries: &mut Vec<KeyEntry>,
) -> usize {
    if key.len() != 32 || remaining_databases.is_empty() {
        return 0;
    }
    let mut matched_salts = HashSet::new();
    for db in db_pages {
        if !remaining_databases.contains(&db.db_name) {
            continue;
        }
        let key_array: [u8; 32] = key.try_into().unwrap();
        attempted_databases.insert(db.db_name.clone());
        let Some(page1) = db
            .page1_variants
            .iter()
            .find(|page| verify_page1(&key_array, page))
        else {
            continue;
        };
        let salt_hex = hex_encode(&page1[..SALT_SIZE]);
        if remaining_databases.remove(&db.db_name) {
            matched_salts.insert(salt_hex.clone());
            entries.push(KeyEntry {
                db_name: db.db_name.clone(),
                enc_key: hex_encode(key),
                salt: salt_hex,
            });
        }
    }
    matched_salts.len()
}

pub(super) fn collect_db_pages(root: &Path) -> Result<Vec<DbPage>> {
    let mut pages = Vec::new();
    collect_db_pages_recursive(root, root, &mut pages)?;
    Ok(pages)
}

fn collect_db_pages_recursive(root: &Path, dir: &Path, pages: &mut Vec<DbPage>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if super::super::is_migration_path(root, &path) {
            continue;
        }
        if path.is_dir() {
            collect_db_pages_recursive(root, &path, pages)?;
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("db")
            || std::fs::metadata(&path)?.len() < PAGE_SIZE as u64
        {
            continue;
        }
        let mut file = std::fs::File::open(&path)?;
        let mut page1 = vec![0u8; PAGE_SIZE];
        file.read_exact(&mut page1)?;
        if &page1[..15] == b"SQLite format 3" {
            continue;
        }
        let mut page1_variants = Vec::with_capacity(2);
        if let Some(wal_page1) = read_wal_page1(&path)? {
            page1_variants.push(wal_page1);
        }
        page1_variants.push(page1);
        pages.push(DbPage {
            db_name: path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/"),
            page1_variants,
        });
    }
    Ok(())
}

/// In WAL mode, page 1 in the main file can be older than the page currently
/// visible to SQLite. Use the last complete WAL frame for page 1 when present.
fn read_wal_page1(db_path: &Path) -> Result<Option<Vec<u8>>> {
    let wal_path = db_path.with_extension("db-wal");
    let Ok(mut file) = std::fs::File::open(wal_path) else {
        return Ok(None);
    };
    let length = file.metadata()?.len() as usize;
    if length < 32 {
        return Ok(None);
    }

    let mut header = [0u8; 32];
    file.read_exact(&mut header)?;
    let magic = u32::from_be_bytes(header[..4].try_into().unwrap());
    if magic != 0x377f0682 && magic != 0x377f0683 {
        return Ok(None);
    }
    let page_size = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;
    if page_size != PAGE_SIZE {
        return Ok(None);
    }

    let frame_size = 24 + page_size;
    let frame_count = (length - 32) / frame_size;
    let mut latest_page1 = None;
    for frame_index in 0..frame_count {
        let frame_offset = 32 + frame_index * frame_size;
        file.seek(SeekFrom::Start(frame_offset as u64))?;
        let mut frame_header = [0u8; 24];
        file.read_exact(&mut frame_header)?;
        if frame_header[8..16] != header[16..24] {
            continue;
        }
        let page_number = u32::from_be_bytes(frame_header[..4].try_into().unwrap());
        if page_number != 1 {
            continue;
        }
        let mut page1 = vec![0u8; PAGE_SIZE];
        file.read_exact(&mut page1)?;
        latest_page1 = Some(page1);
    }
    Ok(latest_page1)
}

fn decode_hex(value: &[u8]) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .chunks_exact(2)
        .map(hex_pair)
        .collect()
}

fn hex_pair(pair: &[u8]) -> Option<u8> {
    Some((hex_digit(pair[0])? << 4) | hex_digit(pair[1])?)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|value| format!("{value:02x}")).collect()
}

pub(super) fn verify_page1(enc_key: &[u8; KEY_SIZE], page: &[u8]) -> bool {
    if page.len() < PAGE_SIZE {
        return false;
    }
    let mut mac_salt = [0u8; SALT_SIZE];
    for (index, value) in page[..SALT_SIZE].iter().enumerate() {
        mac_salt[index] = value ^ 0x3a;
    }
    let mut mac_key = [0u8; KEY_SIZE];
    pbkdf2_hmac::<Sha512>(enc_key, &mac_salt, 2, &mut mac_key);
    let Ok(mut mac) = Hmac::<Sha512>::new_from_slice(&mac_key) else {
        return false;
    };
    mac.update(&page[16..4032]);
    mac.update(&1u32.to_le_bytes());
    mac.verify_slice(&page[4032..PAGE_SIZE]).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_and_salt_collectors_exclude_the_same_migration_paths() {
        let root = std::env::temp_dir().join(format!(
            "wx-scanner-scope-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        for relative in ["MiGrAtE/nested/old.db", "message/current.db", "message/migrate/keep.db"] {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, vec![0xcc; PAGE_SIZE]).unwrap();
        }
        let pages = collect_db_pages(&root).unwrap();
        let page_names: BTreeSet<String> = pages.into_iter().map(|page| page.db_name).collect();
        let salt_names: BTreeSet<String> = super::super::super::collect_db_salts(&root)
            .into_iter().map(|(_, name)| name).collect();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(page_names, salt_names);
        assert_eq!(page_names, BTreeSet::from([
            "message/current.db".into(), "message/migrate/keep.db".into()
        ]));
    }

    #[test]
    fn decodes_config_cipher_blob() {
        let key: Vec<u8> = (0u8..32).collect();
        let salt: Vec<u8> = (32u8..48).collect();
        let plain = format!("x'{}{}'", hex_encode(&key), hex_encode(&salt));
        assert_eq!(plain.len(), 99);
        let encoded: Vec<u8> = plain
            .as_bytes()
            .iter()
            .enumerate()
            .map(|(index, value)| value ^ XOR_MASK[index % XOR_MASK.len()])
            .collect();

        let candidates = decode_candidates(&encoded);
        assert_eq!(candidates, vec![key]);
    }

    #[test]
    fn rejects_non_config_blob() {
        assert!(decode_candidates(&[0u8; 99]).is_empty());
    }

    #[test]
    fn reads_page1_from_current_wal_generation() {
        let dir = std::env::temp_dir().join(format!(
            "wx-cli-wal-test-{:?}",
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test.db");
        std::fs::write(&db_path, vec![0u8; PAGE_SIZE]).unwrap();

        let mut wal = vec![0u8; 32];
        wal[..4].copy_from_slice(&0x377f0682u32.to_be_bytes());
        wal[8..12].copy_from_slice(&(PAGE_SIZE as u32).to_be_bytes());
        wal[16..24].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        for (salt_matches, marker) in [(false, 0x11u8), (true, 0x22u8)] {
            let mut frame = vec![0u8; 24 + PAGE_SIZE];
            frame[..4].copy_from_slice(&1u32.to_be_bytes());
            if salt_matches {
                frame[8..16].copy_from_slice(&wal[16..24]);
            }
            frame[24] = marker;
            wal.extend_from_slice(&frame);
        }
        std::fs::write(db_path.with_extension("db-wal"), wal).unwrap();

        let page = read_wal_page1(&db_path).unwrap().unwrap();
        assert_eq!(page[0], 0x22);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
