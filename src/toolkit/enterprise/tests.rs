use super::*;
use serde_json::Value;
use sha2::{Digest, Sha256};

const KEY: [u8; 16] = [
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];
const PLAIN: &[u8] = include_bytes!("../../../tests/fixtures/enterprise/plain.db");
const HEADER: &[u8] = include_bytes!("../../../tests/fixtures/enterprise/header.enc");
const LEGACY: &[u8] = include_bytes!("../../../tests/fixtures/enterprise/legacy.enc");
const MANIFEST: &str = include_str!("../../../tests/fixtures/enterprise/golden.json");

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let seq = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("wx-enterprise-test-{}-{seq}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn source(&self, bytes: &[u8]) -> PathBuf {
        let path = self.0.join("source.db");
        fs::write(&path, bytes).unwrap();
        path
    }
    fn output(&self) -> PathBuf {
        self.0.join("output.db")
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn golden_hashes_and_derivation_match_vendor() {
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    assert_eq!(manifest["synthetic_only"], true);
    for (name, bytes) in [
        ("plain.db", PLAIN),
        ("header.enc", HEADER),
        ("legacy.enc", LEGACY),
    ] {
        assert_eq!(
            hex(&Sha256::digest(bytes)),
            manifest["sha256"][name].as_str().unwrap()
        );
    }
    for vector in manifest["vectors"].as_array().unwrap() {
        let page = vector["page"].as_u64().unwrap() as u32;
        assert_eq!(
            hex(&derive_page_key(&KEY, page).unwrap()),
            vector["key"].as_str().unwrap()
        );
        assert_eq!(
            hex(&generate_initial_vector(page).unwrap()),
            vector["iv"].as_str().unwrap()
        );
    }
}

#[test]
fn every_encrypted_page_matches_python_plaintext() {
    for encrypted in [HEADER, LEGACY] {
        for (index, page) in encrypted.chunks_exact(PAGE_SIZE).enumerate() {
            assert_eq!(
                decrypt_page(&KEY, page, index as u32 + 1).unwrap(),
                PLAIN[index * PAGE_SIZE..(index + 1) * PAGE_SIZE]
            );
        }
    }
}

#[test]
fn file_roundtrip_all_layouts_preserves_source_and_checks_rows() {
    for (data, format) in [
        (HEADER, DatabaseFormat::WxSqlite3Aes128Header),
        (LEGACY, DatabaseFormat::WxSqlite3Aes128Legacy),
        (PLAIN, DatabaseFormat::PlainSqlite),
    ] {
        let dir = Sandbox::new();
        let source = dir.source(data);
        let report = decrypt_database(&source, &dir.output(), &KEY).unwrap();
        assert_eq!(report.format, format);
        assert_eq!(report.bytes, PLAIN.len() as u64);
        assert_eq!(report.pages as usize, PLAIN.len() / PAGE_SIZE);
        assert_eq!(fs::read(&source).unwrap(), data);
        assert_eq!(fs::read(dir.output()).unwrap(), PLAIN);
        let db =
            Connection::open_with_flags(dir.output(), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let count: i64 = db
            .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 80);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2);
    }
}

#[test]
fn rejects_bad_keys_and_page_parameters() {
    assert_eq!(
        parse_key_hex(" x'00112233445566778899aabbccddeeff' ").unwrap(),
        KEY
    );
    for key in [
        "",
        "1234",
        "zz112233445566778899aabbccddeeff00",
        "00112233445566778899aabbccddeef中",
    ] {
        assert!(parse_key_hex(key).is_err());
    }
    assert!(derive_page_key(&KEY, 0).is_err());
    assert!(generate_initial_vector(0).is_err());
    assert!(decrypt_page(&KEY, &HEADER[..PAGE_SIZE - 1], 1).is_err());
    assert!(decrypt_page(&KEY, &HEADER[..PAGE_SIZE], 0).is_err());
    for data in [HEADER, LEGACY] {
        let dir = Sandbox::new();
        let source = dir.source(data);
        assert!(decrypt_database(&source, &dir.output(), &[0xff; 16]).is_err());
        assert_eq!(fs::read(source).unwrap(), data);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }
}

#[test]
fn rejects_empty_truncated_unknown_and_non_4096_headers() {
    let mut wrong_size = HEADER.to_vec();
    wrong_size[16..18].copy_from_slice(&[0x20, 0]);
    let mut wrong_plain = PLAIN.to_vec();
    wrong_plain[16..18].copy_from_slice(&[0, 1]);
    let mut extra_page = HEADER.to_vec();
    extra_page.extend_from_slice(&[0; PAGE_SIZE]);
    for data in [
        vec![],
        HEADER[..HEADER.len() - 1].to_vec(),
        HEADER[..HEADER.len() - PAGE_SIZE].to_vec(),
        extra_page,
        vec![0; PAGE_SIZE],
        wrong_size,
        wrong_plain,
    ] {
        let dir = Sandbox::new();
        let source = dir.source(&data);
        assert!(decrypt_database(&source, &dir.output(), &KEY).is_err());
        assert_eq!(fs::read(source).unwrap(), data);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }
}

#[test]
fn integrity_failure_cleans_temporary_and_preserves_source() {
    // 第二页是实际的表根页；破坏该页，确保不是仅校验第一页就发布。
    for original in [HEADER, LEGACY, PLAIN] {
        let mut corrupt = original.to_vec();
        corrupt[PAGE_SIZE..PAGE_SIZE * 2].fill(0);
        let dir = Sandbox::new();
        let source = dir.source(&corrupt);
        let error = decrypt_database(&source, &dir.output(), &KEY).unwrap_err();
        assert!(format!("{error:#}").contains("structurally valid"));
        assert_eq!(fs::read(source).unwrap(), corrupt);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }
}

#[test]
fn never_overwrites_existing_output_or_source_alias() {
    let dir = Sandbox::new();
    let source = dir.source(HEADER);
    fs::write(dir.output(), b"previous output").unwrap();
    assert!(decrypt_database(&source, &dir.output(), &KEY).is_err());
    assert_eq!(fs::read(dir.output()).unwrap(), b"previous output");
    assert!(decrypt_database(&source, &source, &KEY).is_err());
    let alias = dir.0.join("alias.db");
    fs::hard_link(&source, &alias).unwrap();
    assert!(decrypt_database(&source, &alias, &KEY).is_err());
    assert_eq!(fs::read(source).unwrap(), HEADER);
    assert_eq!(fs::read(alias).unwrap(), HEADER);
}

#[test]
fn rejects_source_and_destination_sidecars() {
    for suffix in ["-wal", "-shm", "-journal"] {
        for output_side in [false, true] {
            let dir = Sandbox::new();
            let source = dir.source(HEADER);
            let base = if output_side {
                dir.output()
            } else {
                source.clone()
            };
            let mut name = base.into_os_string();
            name.push(suffix);
            fs::write(Path::new(&name), b"synthetic sidecar").unwrap();
            assert!(decrypt_database(&source, &dir.output(), &KEY).is_err());
            assert_eq!(fs::read(source).unwrap(), HEADER);
            assert!(!dir.output().exists());
            assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2);
        }
    }
}

#[test]
fn rejects_missing_source_directory_source_and_missing_output_parent() {
    let dir = Sandbox::new();
    assert!(decrypt_database(&dir.0.join("missing.db"), &dir.output(), &KEY).is_err());
    assert!(decrypt_database(&dir.0, &dir.output(), &KEY).is_err());
    let source = dir.source(HEADER);
    assert!(decrypt_database(&source, &dir.0.join("missing/out.db"), &KEY).is_err());
    assert_eq!(fs::read(source).unwrap(), HEADER);
}
