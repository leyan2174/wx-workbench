use super::super::local_files::read_entries;
use super::*;
use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
use rusqlite::Connection;
use std::fs::OpenOptions;

const HASH: &str = "0123456789abcdef0123456789abcdef";
const PLAIN: &[u8] = b"\xff\xd8\xffsynthetic image\xff\xd9";

#[test]
fn final_evidence_refusal_cleans_shared_temporary_and_preserves_existing_output() {
    let f = Fixture::new();
    f.legacy(".dat");
    let old = f.output.join("previous.jpg");
    fs::write(&old, b"existing valid output").unwrap();
    let mut checked = false;
    let result = export_image_impl(f.request(), None, None, || {
        checked = true;
        anyhow::bail!("synthetic evidence changed")
    });
    assert!(checked);
    assert!(result
        .unwrap_err()
        .chain()
        .any(|error| error.to_string() == "synthetic evidence changed"));
    assert_eq!(fs::read(&old).unwrap(), b"existing valid output");
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 1);
}

#[test]
fn original_host_guard_allows_unchanged_export() {
    let f = Fixture::new();
    f.legacy(".dat");
    let mut guard = HostOutputGuard::new(&f.output).unwrap();
    guard.protect(&f.attach).unwrap();
    guard.protect(&f.db).unwrap();
    let result = export_image_with_guard(f.request(), &guard).unwrap();
    assert_eq!(fs::read(result.path).unwrap(), PLAIN);
}

#[test]
fn original_host_guard_rejects_replacement_output() {
    let f = Fixture::new();
    f.legacy(".dat");
    let protected = f._temp.path().join("protected");
    fs::create_dir(&protected).unwrap();
    fs::write(protected.join("sentinel"), b"protected").unwrap();
    let mut guard = HostOutputGuard::new(&f.output).unwrap();
    guard.protect(&protected).unwrap();
    if fs::rename(&f.output, f._temp.path().join("old-output")).is_err() {
        return;
    }
    if fs::rename(&protected, &f.output).is_err() {
        assert!(export_image_with_guard(f.request(), &guard).is_err());
        return;
    }
    assert!(export_image_with_guard(f.request(), &guard).is_err());
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 1);
    assert_eq!(fs::read(f.output.join("sentinel")).unwrap(), b"protected");
}

struct Fixture {
    _temp: tempfile::TempDir,
    attach: PathBuf,
    db: PathBuf,
    output: PathBuf,
    message: MessageIdentity,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let attach = temp.path().join("attach");
        let database = temp.path().join("resource");
        let output = temp.path().join("output");
        for p in [&attach, &database, &output] {
            fs::create_dir(p).unwrap();
        }
        let db = database.join("message_resource.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ChatName2Id(user_name TEXT);
            INSERT INTO ChatName2Id(rowid,user_name) VALUES(7,'test@chatroom');
            CREATE TABLE MessageResourceInfo(chat_id INTEGER,message_local_id INTEGER,
            message_local_type INTEGER,message_create_time INTEGER,packed_info BLOB);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO MessageResourceInfo VALUES(7,42,3,1700000000,?1)",
            [HASH.as_bytes()],
        )
        .unwrap();
        drop(conn);
        Self {
            _temp: temp,
            attach,
            db,
            output,
            message: MessageIdentity {
                username: "test@chatroom".into(),
                source: "message/message_0.db".into(),
                local_id: 42,
                create_time: 1700000000,
                local_type: 3,
            },
        }
    }
    fn dat(&self, month: &str, suffix: &str, bytes: &[u8]) -> PathBuf {
        let dir = self
            .attach
            .join(format!(
                "{:x}",
                md5::compute(self.message.username.as_bytes())
            ))
            .join(month)
            .join("Img");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{HASH}{suffix}"));
        fs::write(&path, bytes).unwrap();
        path
    }
    fn legacy(&self, suffix: &str) -> PathBuf {
        self.dat(
            "2023-11",
            suffix,
            &PLAIN.iter().map(|b| b ^ 0xa5).collect::<Vec<_>>(),
        )
    }
    fn request(&self) -> ImageRequest<'_> {
        ImageRequest {
            message: &self.message,
            resource_db: &self.db,
            attach_root: &self.attach,
            output_root: &self.output,
            key: decoder::V2KeyMaterial::default(),
        }
    }
    fn sql(&self, sql: &str) {
        Connection::open(&self.db)
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }
    fn fails(&self, expected: &str) {
        let before = fs::read(&self.db).unwrap();
        let error = export_image(self.request()).unwrap_err();
        assert!(
            format!("{error:#}").contains(expected),
            "expected {expected}: {error:#}"
        );
        assert_eq!(fs::read(&self.db).unwrap(), before);
        assert_eq!(fs::read_dir(&self.output).unwrap().count(), 0);
    }
}

#[test]
fn legacy_exact_identity_source_unchanged_and_no_overwrite() {
    let f = Fixture::new();
    let dat = f.legacy(".dat");
    let source = fs::read(&dat).unwrap();
    let db = fs::read(&f.db).unwrap();
    let out = export_image(f.request()).unwrap();
    assert_eq!(fs::read(&out.path).unwrap(), PLAIN);
    assert_eq!(out.source_path, dat);
    assert_eq!(out.resource_md5, HASH);
    assert_eq!(out.message.source, f.message.source);
    assert_eq!(out.decoded_md5, format!("{:x}", md5::compute(PLAIN)));
    assert_eq!(out.decoder, "legacy_xor");
    assert_eq!(out.binding, "resource_scan_filename_heuristic");
    assert!(export_image(f.request())
        .unwrap_err()
        .to_string()
        .contains("publication failed"));
    assert_eq!(fs::read(&out.path).unwrap(), PLAIN);
    assert_eq!(fs::read(&dat).unwrap(), source);
    assert_eq!(fs::read(&f.db).unwrap(), db);
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 1);
}

#[test]
fn exact_tuple_never_falls_back_or_masks_type_flags() {
    for sql in [
        "UPDATE MessageResourceInfo SET message_create_time=1700000001",
        "UPDATE MessageResourceInfo SET message_local_id=43",
        "UPDATE MessageResourceInfo SET message_local_type=4294967299",
        "UPDATE MessageResourceInfo SET chat_id=8",
    ] {
        let f = Fixture::new();
        f.legacy(".dat");
        f.sql(sql);
        f.fails("exact image resource not found");
    }
    let mut f = Fixture::new();
    f.legacy(".dat");
    f.sql("UPDATE MessageResourceInfo SET message_local_type=4294967299");
    f.message.local_type = 4294967299;
    assert!(export_image(f.request()).is_ok());
}

#[test]
fn duplicate_chat_and_duplicate_exact_row_rejected() {
    let f = Fixture::new();
    f.sql("INSERT INTO ChatName2Id VALUES('test@chatroom')");
    f.fails("chat mapping must be unique");
    let f = Fixture::new();
    f.sql("INSERT INTO MessageResourceInfo SELECT * FROM MessageResourceInfo");
    f.fails("ambiguous exact image resource");
}

#[test]
fn schemas_views_without_rowid_and_shadow_aliases_rejected() {
    for table in ["ChatName2Id", "MessageResourceInfo"] {
        for sql in [format!("ALTER TABLE {table} RENAME TO original; CREATE VIEW {table} AS SELECT * FROM original"),
            format!("DROP TABLE {table}; CREATE TABLE {table}(id INTEGER PRIMARY KEY) WITHOUT ROWID"),
            format!("ALTER TABLE {table} ADD COLUMN RoWiD INTEGER"),
            format!("ALTER TABLE {table} ADD COLUMN _RoWiD_ INTEGER GENERATED ALWAYS AS (1) VIRTUAL"),
            format!("ALTER TABLE {table} ADD COLUMN OID INTEGER")] {
            let f=Fixture::new(); f.sql(&sql);
            f.fails(if sql.contains("COLUMN") {"shadowed resource rowid"} else {"unsupported resource table schema"});
        }
    }
}

#[test]
fn text_packed_oversized_packed_and_noninteger_identity_rejected() {
    let f = Fixture::new();
    f.sql("UPDATE MessageResourceInfo SET packed_info=CAST(packed_info AS TEXT)");
    f.fails("packed_info must be BLOB");
    let f = Fixture::new();
    f.sql("UPDATE MessageResourceInfo SET packed_info=zeroblob(1048577)");
    f.fails("packed_info size limit");
    let f = Fixture::new();
    f.sql("ALTER TABLE MessageResourceInfo RENAME TO old;
        CREATE TABLE MessageResourceInfo(chat_id,message_local_id,message_local_type,message_create_time,packed_info);
        INSERT INTO MessageResourceInfo SELECT 7.0,42,3,1700000000,packed_info FROM old;");
    f.fails("resource identity must use INTEGER");
}

#[test]
fn selection_full_then_hd_then_thumb_and_uppercase() {
    for suffix in [".DAT", "_H.DAT", "_T.DAT"] {
        let f = Fixture::new();
        let selected = f.legacy(suffix);
        if suffix == ".DAT" {
            f.dat("2024-01", "_h.dat", b"not selected");
        }
        if suffix != "_T.DAT" {
            f.dat("2025-12", "_t.dat", b"not selected");
        }
        let out = export_image(f.request()).unwrap();
        assert_eq!(out.source_path, selected);
    }
}

#[test]
fn same_rank_duplicates_reject_even_identical_content() {
    let f = Fixture::new();
    let p = f.legacy(".dat");
    f.dat("2025-01", ".dat", &fs::read(p).unwrap());
    f.fails("ambiguous DAT");
}

#[test]
fn broad_prefix_and_other_chat_never_match() {
    let f = Fixture::new();
    f.legacy("-wrong.dat");
    f.fails("local DAT not found");
    let f = Fixture::new();
    let dat = f.legacy(".dat");
    let chat = dat.parent().unwrap().parent().unwrap().parent().unwrap();
    fs::rename(chat, f.attach.join("another_chat")).unwrap();
    assert!(export_image(f.request()).is_err());
}

fn encrypted(magic: &[u8], key: &[u8; 16], xor: u8) -> Vec<u8> {
    let mut block = [13u8; 16];
    block[..3].copy_from_slice(&PLAIN[..3]);
    let mut block = GenericArray::clone_from_slice(&block);
    aes::Aes128::new(key.into()).encrypt_block(&mut block);
    let mut bytes = magic.to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&block);
    bytes.extend_from_slice(&PLAIN[3..PLAIN.len() - 2]);
    bytes.extend(PLAIN[PLAIN.len() - 2..].iter().map(|b| b ^ xor));
    bytes
}

#[test]
fn v1_and_v2_reuse_decoder_explicit_keys_no_provider() {
    for (magic, aes, tag) in [
        (&decoder::V1_MAGIC, b"cfcd208495d565ef", "v1_aes"),
        (&decoder::V2_MAGIC, b"1234567890abcdef", "v2"),
    ] {
        let f = Fixture::new();
        f.dat("2023-11", ".dat", &encrypted(magic, aes, 0xa2));
        if tag == "v2" {
            f.fails("AES key");
        }
        let mut request = f.request();
        request.key = decoder::V2KeyMaterial {
            aes_key: Some(aes),
            xor_key: 0xa2,
        };
        let out = export_image(request).unwrap();
        assert_eq!(out.decoder, tag);
        assert_eq!(fs::read(out.path).unwrap(), PLAIN);
    }
}

#[test]
fn malformed_dat_and_wrong_v2_key_leave_no_output() {
    let f = Fixture::new();
    f.dat("2023-11", ".dat", b"invalid");
    assert!(export_image(f.request()).is_err());
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 0);
    let f = Fixture::new();
    f.dat(
        "2023-11",
        ".dat",
        &encrypted(&decoder::V2_MAGIC, b"1234567890abcdef", 0xa2),
    );
    let mut request = f.request();
    request.key = decoder::V2KeyMaterial {
        aes_key: Some(b"xxxxxxxxxxxxxxxx"),
        xor_key: 0xa2,
    };
    assert!(export_image(request).is_err());
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 0);
}

#[test]
fn dat_and_resource_size_limits_are_enforced() {
    let f = Fixture::new();
    let path = f.legacy(".dat");
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(MAX_DAT_BYTES + 1)
        .unwrap();
    f.fails("DAT size limit");
    let f = Fixture::new();
    OpenOptions::new()
        .write(true)
        .open(&f.db)
        .unwrap()
        .set_len(MAX_RESOURCE_BYTES + 1)
        .unwrap();
    assert!(export_image(f.request())
        .unwrap_err()
        .to_string()
        .contains("database limit"));
}

#[test]
fn output_inside_source_or_existing_hardlink_rejected() {
    let f = Fixture::new();
    let dat = f.legacy(".dat");
    let mut request = f.request();
    request.output_root = dat.parent().unwrap();
    assert!(export_image(request)
        .unwrap_err()
        .to_string()
        .contains("outside attachment source"));
    let output = f.output.join(format!("{:x}.jpg", md5::compute(PLAIN)));
    fs::hard_link(&dat, &output).unwrap();
    let before = fs::read(&dat).unwrap();
    assert!(export_image(f.request()).is_err());
    assert_eq!(fs::read(&dat).unwrap(), before);
    assert_eq!(fs::read(&output).unwrap(), before);
}

#[test]
fn resource_sidecars_and_dat_directory_rejected() {
    for suffix in ["-wal", "-shm", "-journal"] {
        let f = Fixture::new();
        fs::write(format!("{}{suffix}", f.db.display()), b"sentinel").unwrap();
        f.fails("static resource snapshot");
    }
    let f = Fixture::new();
    let path = f.legacy(".dat");
    fs::remove_file(&path).unwrap();
    fs::create_dir(path).unwrap();
    f.fails("DAT candidate is not a regular file");
}

#[test]
fn pinned_file_blocks_writers_and_detects_changed_directory() {
    let f = Fixture::new();
    let dat = f.legacy(".dat");
    let pin = Pin::open(&dat, false).unwrap();
    assert!(OpenOptions::new().write(true).open(&dat).is_err());
    assert!(fs::remove_file(&dat).is_err());
    pin.verify().unwrap();
    let mut scan = Scan::new();
    scan.root(&f.output).unwrap();
    scan.entries(&f.output).unwrap();
    fs::write(f.output.join("new"), b"changed").unwrap();
    assert!(scan.verify().is_err());
}

#[test]
fn bounded_enumeration_and_unsafe_roots() {
    let f = Fixture::new();
    fs::write(f.output.join("one"), b"1").unwrap();
    assert!(read_entries(&f.output, &mut 0).is_err());
    for path in [
        Path::new("relative"),
        Path::new(r"\\server\share"),
        Path::new(r"C:\temp\..\other"),
    ] {
        assert!(Scan::new().root(path).is_err());
    }
    for name in ["file:stream", "NUL", "COM1.dat", "name.", ".."][..].iter() {
        assert!(safe_name(name).is_err());
    }
}

#[cfg(windows)]
#[test]
fn directory_junction_rejected_without_following_target() {
    let f = Fixture::new();
    let link = f._temp.path().join("junction");
    let status = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&f.attach)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let result = Scan::new().root(&link);
    fs::remove_dir(&link).unwrap();
    assert!(result.unwrap_err().to_string().contains("reparse point"));
    assert!(f.attach.is_dir());
}

#[test]
fn virtual_resource_tables_rejected_before_queries() {
    for table in ["ChatName2Id", "MessageResourceInfo"] {
        let f = Fixture::new();
        // 模拟由其他 SQLite 构建生成的虚拟表目录，不依赖本机启用 FTS 模块。
        f.sql(&format!("DROP TABLE {table}; PRAGMA writable_schema=ON;
            INSERT INTO sqlite_schema(type,name,tbl_name,rootpage,sql)
            VALUES('table','{table}','{table}',0,'CREATE VIRTUAL TABLE {table} USING fts5(value)');"));
        f.fails("unsupported resource table schema");
    }
}

#[test]
fn candidate_limit_rejects_instead_of_truncating() {
    let f = Fixture::new();
    for i in 0..=MAX_CANDIDATES {
        f.dat(&format!("month-{i}"), ".dat", b"not decoded");
    }
    f.fails("candidate limit");
}

#[test]
fn resource_hardlink_output_never_overwritten() {
    let f = Fixture::new();
    f.legacy(".dat");
    let output = f.output.join(format!("{:x}.jpg", md5::compute(PLAIN)));
    fs::hard_link(&f.db, &output).unwrap();
    let before = fs::read(&f.db).unwrap();
    assert!(export_image(f.request()).is_err());
    assert_eq!(fs::read(&f.db).unwrap(), before);
    assert_eq!(fs::read(&output).unwrap(), before);
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 1);
}

#[test]
fn invalid_identity_is_rejected_before_path_access() {
    let mut f = Fixture::new();
    fs::remove_dir(&f.attach).unwrap();
    for (id, time, kind) in [(0, 1, 3), (1, 0, 3), (1, 1, 34), (1, 1, -1)] {
        f.message.local_id = id;
        f.message.create_time = time;
        f.message.local_type = kind;
        f.fails("invalid image identity");
    }
}

#[test]
fn host_output_guard_rejects_missing_and_relative_outputs() {
    for path in [
        Path::new(""),
        Path::new("relative"),
        Path::new(r"\\server\share"),
    ] {
        assert!(HostOutputGuard::new(path).is_err());
    }
}

#[test]
fn host_output_guard_isolates_directories_and_protected_files() {
    let f = Fixture::new();
    let mut guard = HostOutputGuard::new(&f.output).unwrap();
    guard.protect(&f.attach).unwrap();
    guard.protect(&f.db).unwrap();
    assert!(guard.protect(&f.output).is_err());
    let input = f.output.join("keys.json");
    fs::write(&input, b"protected").unwrap();
    assert!(guard.protect(&input).is_err());
    let nested = f.output.join("nested");
    fs::create_dir(&nested).unwrap();
    assert!(guard.protect(&nested).is_err());
    assert!(guard.protect(f.output.parent().unwrap()).is_err());
    assert_eq!(fs::read(input).unwrap(), b"protected");
}

#[test]
fn host_key_read_is_bounded_and_keeps_readonly_handle() {
    let f = Fixture::new();
    let key = f._temp.path().join("image-key.json");
    fs::write(&key, br#"{"aes_key":"1234567890abcdef","xor_key":"0xa2"}"#).unwrap();
    let mut guard = HostOutputGuard::new(&f.output).unwrap();
    let bytes = guard.read_key_file(&key).unwrap();
    assert!(bytes.starts_with(b"{"));
    assert!(OpenOptions::new().write(true).open(&key).is_err());
    assert!(fs::remove_file(&key).is_err());
    assert!(guard.read_key_file(Path::new("relative-key.json")).is_err());
    let large = f._temp.path().join("oversized.json");
    fs::write(&large, vec![b'x'; 4097]).unwrap();
    assert!(guard
        .read_key_file(&large)
        .unwrap_err()
        .to_string()
        .contains("byte limit"));
}

#[test]
fn host_key_inside_output_or_directory_is_rejected() {
    let f = Fixture::new();
    let key = f.output.join("image-key.json");
    fs::write(&key, b"secret sentinel").unwrap();
    let mut guard = HostOutputGuard::new(&f.output).unwrap();
    assert!(guard.read_key_file(&key).is_err());
    assert!(guard.read_key_file(&f.attach).is_err());
    assert_eq!(fs::read(key).unwrap(), b"secret sentinel");
}

#[test]
fn host_guard_accepts_local_canonical_paths_and_future_protected_files() {
    let f = Fixture::new();
    let mut guard = HostOutputGuard::new(&f.output.canonicalize().unwrap()).unwrap();
    guard.protect(&f.db.canonicalize().unwrap()).unwrap();
    let future = f._temp.path().join("future-mtime.json");
    guard.protect(&future).unwrap();
    assert!(!future.exists());
    assert!(guard.protect(&f.output.join("future-key.json")).is_err());
}
