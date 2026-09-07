//! 密钥绑定当前账号和明确的相对数据库路径；仅显式私有文件发布器可以序列化密钥。
use super::{
    checked_directory, relative_path, Account, Database, ProcessScanner, ScanOptions, ScanReport,
};
use crate::{attachment::local_files::HostOutputGuard, toolkit::enterprise};
use anyhow::{ensure, Context, Result};
use serde::{
    de::{MapAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

#[derive(Default)]
pub struct KeyOptions {
    /// 32 位 hex 原始密钥文件，仅作用于当前显式选择的账号。
    pub key_file: Option<PathBuf>,
    /// 兼容 vendor 的 {_db_dir, 相对路径: {enc_key, salt, size_mb}} 文件。
    pub keys_file: Option<PathBuf>,
    pub auto_scan: Option<ScanOptions>,
}

impl KeyOptions {
    pub fn validate(&self) -> Result<()> {
        if let Some(options) = &self.auto_scan {
            options.validate()?;
        }
        Ok(())
    }
}

pub struct KeyRing {
    directory: PathBuf,
    keys: BTreeMap<String, Zeroizing<[u8; 16]>>,
}

impl KeyRing {
    /// 在扫描前检查并固定路径；重复输出必须显式选择新文件，不替换旧密钥或配置。
    pub fn prepare_output(source: &Path, output: &Path) -> Result<PreparedKeyOutput> {
        ensure!(output.is_absolute(), "密钥输出必须使用绝对路径");
        ensure!(
            output
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("json")),
            "密钥输出必须为 JSON 文件"
        );
        let account = Account::open(source)?;
        let parent = output.parent().context("密钥输出缺少父目录")?;
        let mut guard = HostOutputGuard::new(parent)?;
        guard.protect(&account.directory)?;
        guard.verify_replaceable_file(output)?;
        require_absent(output)?;
        let (databases, failures) = super::inventory(&account)?;
        ensure!(
            failures.is_empty() && databases.iter().any(|db| !db.plain),
            "保存密钥需要完整且含加密主库的账号清单"
        );
        let identities = databases
            .iter()
            .map(|db| same_file::Handle::from_path(&db.path))
            .collect::<std::io::Result<Vec<_>>>()?;
        guard.verify()?;
        Ok(PreparedKeyOutput {
            account,
            output: output.to_path_buf(),
            guard,
            databases,
            identities,
        })
    }
    pub(crate) fn is_for(&self, account: &Account) -> bool {
        self.directory == account.directory
    }
    pub(crate) fn new(account: &Account) -> Self {
        Self {
            directory: account.directory.clone(),
            keys: BTreeMap::new(),
        }
    }
    pub(crate) fn get(&self, account: &Account, relative: &str) -> Option<&[u8; 16]> {
        if self.directory != account.directory {
            return None;
        }
        self.keys.get(relative).map(|key| &**key)
    }
    pub(crate) fn contains(&self, relative: &str) -> bool {
        self.keys.contains_key(relative)
    }
    pub(crate) fn insert(&mut self, database: &Database, key: &[u8; 16]) -> bool {
        if database.plain || self.contains(&database.relative) || !verify(key, &database.page) {
            return false;
        }
        self.keys
            .insert(database.relative.clone(), Zeroizing::new(*key));
        true
    }
    pub(crate) fn offer(&mut self, databases: &[Database], key: &[u8; 16]) -> usize {
        databases
            .iter()
            .filter(|database| self.insert(database, key))
            .count()
    }
}

/// 不持有密钥副本，整个扫描期间保留账号与输出目录的身份守卫。
pub struct PreparedKeyOutput {
    account: Account,
    output: PathBuf,
    guard: HostOutputGuard,
    databases: Vec<Database>,
    identities: Vec<same_file::Handle>,
}

impl PreparedKeyOutput {
    fn verified_inventory(&self, ring: &KeyRing) -> Result<Vec<Database>> {
        self.guard.verify()?;
        ensure!(ring.is_for(&self.account), "密钥环不属于输出绑定账号");
        let (current, failures) = super::inventory(&self.account)?;
        ensure!(
            failures.is_empty() && current.len() == self.databases.len(),
            "扫描期间数据库清单发生变化，未保存密钥"
        );
        for ((before, after), identity) in self.databases.iter().zip(&current).zip(&self.identities)
        {
            ensure!(
                before.relative == after.relative
                    && before.page == after.page
                    && *identity == same_file::Handle::from_path(&after.path)?,
                "扫描期间数据库身份或首页发生变化，未保存密钥"
            );
            if !after.plain {
                let key = ring
                    .get(&self.account, &after.relative)
                    .context("密钥环不完整，未保存密钥")?;
                ensure!(verify(key, &after.page), "密钥验证失败，未保存密钥");
            }
        }
        Ok(current)
    }

    /// 只发布完整、仍与当前数据库首页匹配的密钥环；任何失败都保留已有文件。
    pub fn publish(self, ring: &KeyRing) -> Result<()> {
        self.guard.verify_replaceable_file(&self.output)?;
        require_absent(&self.output)?;
        let current = self.verified_inventory(ring)?;
        // 只给专用文件写入器实现序列化，KeyRing 本身仍不能进入日志或普通报告。
        struct Document<'a> {
            ring: &'a KeyRing,
            databases: &'a [Database],
        }
        impl serde::Serialize for Document<'_> {
            fn serialize<S: serde::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                use serde::ser::SerializeMap;
                #[derive(serde::Serialize)]
                struct Entry<'a> {
                    enc_key: &'a str,
                    salt: String,
                }
                let mut map = serializer.serialize_map(Some(self.ring.keys.len() + 1))?;
                map.serialize_entry("_db_dir", &self.ring.directory)?;
                for database in self.databases.iter().filter(|db| !db.plain) {
                    let key = self
                        .ring
                        .keys
                        .get(&database.relative)
                        .ok_or_else(|| serde::ser::Error::custom("密钥环不完整"))?;
                    let mut text = Zeroizing::new(String::with_capacity(32));
                    for byte in key.iter() {
                        const HEX: &[u8; 16] = b"0123456789abcdef";
                        text.push(HEX[(byte >> 4) as usize] as char);
                        text.push(HEX[(byte & 15) as usize] as char);
                    }
                    let salt = database.page[..16]
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    map.serialize_entry(
                        &database.relative,
                        &Entry {
                            enc_key: &text,
                            salt,
                        },
                    )?;
                }
                map.end()
            }
        }
        let mut data = Zeroizing::new(Vec::new());
        serde_json::to_writer_pretty(
            &mut *data,
            &Document {
                ring,
                databases: &current,
            },
        )
        .map_err(|_| anyhow::anyhow!("密钥文件序列化失败"))?;
        data.push(b'\n');
        ensure!(data.len() <= 16 * 1024 * 1024, "密钥文件超过读取器大小限制");
        let mut temporary = tempfile::NamedTempFile::new_in(self.guard.output_root())?;
        crate::toolkit::private_file::restrict(temporary.as_file())?;
        temporary
            .write_all(&data)
            .map_err(|_| anyhow::anyhow!("写入私有密钥文件失败"))?;
        temporary.as_file().sync_all()?;
        let identity = same_file::Handle::from_path(temporary.path())?;
        ensure!(
            identity == same_file::Handle::from_file(temporary.as_file().try_clone()?)?,
            "密钥暂存文件在关闭写端前发生变化"
        );
        // 路径守卫禁止已有写句柄；先关闭写端，再只读固定，允许最终原子重命名。
        let temporary = temporary.into_temp_path();
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1 | 4).custom_flags(0x00200000);
        }
        let mut staged = options.open(&temporary)?;
        ensure!(
            identity == same_file::Handle::from_file(staged.try_clone()?)?
                && staged.metadata()?.len() == data.len() as u64,
            "密钥暂存文件在固定句柄时发生变化"
        );
        // 关闭写端到固定读端的短窗口不能只靠文件 ID 检测内容变化。
        let mut buffer = Zeroizing::new([0u8; 8192]);
        for expected in data.chunks(buffer.len()) {
            staged.read_exact(&mut buffer[..expected.len()])?;
            ensure!(
                buffer[..expected.len()] == *expected,
                "密钥暂存内容发生变化"
            );
        }
        self.verified_inventory(ring)?;
        self.guard.verify()?;
        self.guard.verify_replaceable_file(&temporary)?;
        self.guard.verify_replaceable_file(&self.output)?;
        require_absent(&self.output)?;
        ensure!(
            identity == same_file::Handle::from_path(&temporary)?,
            "密钥暂存文件身份发生变化"
        );
        temporary
            .persist_noclobber(&self.output)
            .map_err(|_| anyhow::anyhow!("密钥文件发布失败，拒绝覆盖现有文件"))?;
        Ok(())
    }
}

fn require_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => anyhow::bail!("无法检查密钥输出路径"),
        Ok(_) => anyhow::bail!("密钥输出已存在，拒绝覆盖；请指定新文件"),
    }
}

/// 页一只是候选筛选；发布前仍由核心执行整库结构校验，不宣称 MAC 认证。
fn verify(key: &[u8; 16], page: &[u8]) -> bool {
    let Ok(decoded) = enterprise::decrypt_page(key, page, 1) else {
        return false;
    };
    let decoded = Zeroizing::new(decoded);
    decoded.starts_with(b"SQLite format 3\0")
        && decoded[16..18] == [0x10, 0]
        && (1..=2).contains(&decoded[18])
        && (1..=2).contains(&decoded[19])
        && decoded[20..24] == [0, 64, 32, 32]
        && matches!(decoded[100], 2 | 5 | 10 | 13)
}

struct SecretText(Zeroizing<String>);
impl<'de> Deserialize<'de> for SecretText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(deserializer).map(|value| Self(Zeroizing::new(value)))
    }
}

#[derive(Deserialize)]
struct FileKey {
    enc_key: SecretText,
    #[serde(default)]
    salt: Option<String>,
}

struct KeyDocument {
    directory: PathBuf,
    entries: BTreeMap<String, FileKey>,
}

impl<'de> Deserialize<'de> for KeyDocument {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct DocumentVisitor;
        impl<'de> Visitor<'de> for DocumentVisitor {
            type Value = KeyDocument;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("账号绑定密钥对象")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut directory = None;
                let mut entries = BTreeMap::new();
                // 直接流式反序列化，避免 flatten/Value 中间容器复制未清零的密钥字符串。
                while let Some(name) = map.next_key::<String>()? {
                    if name == "_db_dir" {
                        if directory.is_some() {
                            return Err(serde::de::Error::custom("重复账号目录"));
                        }
                        directory = Some(map.next_value::<PathBuf>()?);
                    } else {
                        if entries.contains_key(&name) {
                            return Err(serde::de::Error::custom("重复数据库记录"));
                        }
                        entries.insert(name, map.next_value::<FileKey>()?);
                    }
                }
                Ok(KeyDocument {
                    directory: directory.ok_or_else(|| serde::de::Error::custom("缺少账号目录"))?,
                    entries,
                })
            }
        }
        deserializer.deserialize_map(DocumentVisitor)
    }
}

fn read_secret(path: &Path, limit: u64) -> Result<Zeroizing<Vec<u8>>> {
    checked_directory(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    let meta = fs::symlink_metadata(path).map_err(|_| anyhow::anyhow!("密钥文件不可读"))?;
    ensure!(
        meta.is_file() && !super::is_reparse(&meta),
        "密钥文件必须为普通文件"
    );
    let mut data = Zeroizing::new(Vec::new());
    File::open(path)
        .map_err(|_| anyhow::anyhow!("密钥文件不可读"))?
        .take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|_| anyhow::anyhow!("读取密钥文件失败"))?;
    ensure!(data.len() as u64 <= limit, "密钥文件过大");
    Ok(data)
}

pub(crate) fn obtain(
    account: &Account,
    databases: &[Database],
    options: &KeyOptions,
    scanner: Option<&dyn ProcessScanner>,
) -> Result<(KeyRing, Option<ScanReport>)> {
    options.validate()?;
    let mut ring = KeyRing::new(account);
    if let Some(path) = &options.keys_file {
        let data = read_secret(path, 16 * 1024 * 1024)?;
        let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&data);
        // 不转发 serde 原始错误，避免恶意字段把密钥文本带入错误消息。
        let document: KeyDocument = serde_json::from_slice(data)
            .map_err(|_| anyhow::anyhow!("密钥 JSON 格式错误或缺少 _db_dir"))?;
        ensure!(
            document.directory.is_absolute(),
            "密钥 _db_dir 必须是绝对账号目录"
        );
        let bound = checked_directory(&document.directory)
            .map_err(|_| anyhow::anyhow!("密钥绑定账号目录不可用"))?;
        ensure!(
            bound == account.directory,
            "密钥文件属于另一个账号目录；拒绝跨账号使用"
        );
        let mut normalized = BTreeMap::new();
        for (name, entry) in &document.entries {
            let path = relative_path(name).map_err(|_| anyhow::anyhow!("密钥记录路径无效"))?;
            let name = path.to_string_lossy().replace('\\', "/");
            let key = Zeroizing::new(
                enterprise::parse_key_hex(&entry.enc_key.0)
                    .map_err(|_| anyhow::anyhow!("逐库密钥格式错误，应为 32 位 hex"))?,
            );
            if let Some(salt) = &entry.salt {
                ensure!(
                    salt.len() == 32 && salt.bytes().all(|b| b.is_ascii_hexdigit()),
                    "逐库密钥盐值格式错误"
                );
            }
            ensure!(
                normalized
                    .insert(name.to_lowercase(), (key, entry.salt.as_deref()))
                    .is_none(),
                "密钥记录有重复路径"
            );
        }
        for database in databases {
            if database.plain {
                continue;
            }
            let Some((key, salt)) = normalized.get(&database.relative.to_lowercase()) else {
                continue;
            };
            if let Some(salt) = salt {
                let expected = database.page[..16]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>();
                ensure!(
                    salt.eq_ignore_ascii_case(&expected),
                    "逐库密钥盐值与当前数据库不符，拒绝全局兜底"
                );
            }
            ensure!(ring.insert(database, key), "逐库密钥验证失败，拒绝全局兜底");
        }
    }
    // 逐库覆盖全部验证成功后才补缺；存在但错误的覆盖项不能被全局密钥或扫描掩盖。
    if let Some(path) = &options.key_file {
        let data = read_secret(path, 128)?;
        let text = std::str::from_utf8(&data).map_err(|_| anyhow::anyhow!("密钥文件应为 UTF-8"))?;
        let key = Zeroizing::new(
            enterprise::parse_key_hex(text.trim_start_matches('\u{feff}'))
                .map_err(|_| anyhow::anyhow!("原始密钥文件格式错误，应为 32 位 hex"))?,
        );
        ring.offer(databases, &key);
    }
    let report = match &options.auto_scan {
        Some(options) => Some(super::scan::fill(
            account,
            databases,
            &mut ring,
            options,
            scanner.context("自动扫描需要 ProcessScanner 适配器")?,
        )?),
        None => None,
    };
    Ok((ring, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    const HEX: &str = "00112233445566778899aabbccddeeff";
    const HEADER: &[u8] = include_bytes!("../../../tests/fixtures/enterprise/header.enc");
    const PLAIN: &[u8] = include_bytes!("../../../tests/fixtures/enterprise/plain.db");

    struct Fixture {
        _root: tempfile::TempDir,
        account: Account,
        output: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let source = root.path().join("account");
            let output = root.path().join("keys");
            fs::create_dir(&source).unwrap();
            fs::create_dir(&output).unwrap();
            fs::write(source.join("a.db"), HEADER).unwrap();
            Self {
                account: Account::open(&source).unwrap(),
                _root: root,
                output,
            }
        }
        fn ring(&self) -> KeyRing {
            let (databases, failures) = super::super::inventory(&self.account).unwrap();
            assert!(failures.is_empty());
            let mut ring = KeyRing::new(&self.account);
            ring.offer(&databases, &enterprise::parse_key_hex(HEX).unwrap());
            ring
        }
        fn document(&self, entries: serde_json::Value) -> PathBuf {
            let mut entries = entries.as_object().unwrap().clone();
            entries.insert(
                "_db_dir".into(),
                serde_json::to_value(&self.account.directory).unwrap(),
            );
            let file = self.output.join("input.json");
            fs::write(&file, serde_json::to_vec(&entries).unwrap()).unwrap();
            file
        }
        fn read(&self, options: &KeyOptions) -> Result<KeyRing> {
            let (databases, _) = super::super::inventory(&self.account)?;
            obtain(&self.account, &databases, options, None).map(|(ring, _)| ring)
        }
    }

    #[test]
    fn saved_account_keys_roundtrip_through_existing_decrypt() {
        let fixture = Fixture::new();
        let path = fixture.output.join("saved.json");
        let ring = fixture.ring();
        KeyRing::prepare_output(&fixture.account.directory, &path)
            .unwrap()
            .publish(&ring)
            .unwrap();
        let options = KeyOptions {
            keys_file: Some(path.clone()),
            ..Default::default()
        };
        let restored = fixture.read(&options).unwrap();
        assert_eq!(
            restored.get(&fixture.account, "a.db"),
            ring.get(&fixture.account, "a.db")
        );
        let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            json["_db_dir"],
            serde_json::to_value(&fixture.account.directory).unwrap()
        );
        assert_eq!(json["a.db"]["enc_key"], HEX);
        assert_eq!(json["a.db"]["salt"].as_str().unwrap().len(), 32);
        let report = super::super::decrypt_batch(
            &fixture.account.directory,
            &fixture._root.path().join("decrypted"),
            &options,
            None,
        )
        .unwrap();
        assert_eq!((report.decrypted, report.failed), (1, 0));
        assert_eq!(fs::read(report.output.join("a.db")).unwrap(), PLAIN);
        assert!(!serde_json::to_string(&report).unwrap().contains(HEX));
        assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        #[cfg(windows)]
        crate::toolkit::private_file::assert_private_acl(&path);
    }

    #[test]
    fn repeat_and_raced_output_preserve_existing_files() {
        let fixture = Fixture::new();
        let path = fixture.output.join("saved.json");
        let prepared = KeyRing::prepare_output(&fixture.account.directory, &path).unwrap();
        fs::write(&path, b"existing-config").unwrap();
        assert!(prepared.publish(&fixture.ring()).is_err());
        assert!(KeyRing::prepare_output(&fixture.account.directory, &path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"existing-config");
        let fresh = fixture.output.join("next.json");
        KeyRing::prepare_output(&fixture.account.directory, &fresh)
            .unwrap()
            .publish(&fixture.ring())
            .unwrap();
        assert!(KeyRing::prepare_output(&fixture.account.directory, &fresh).is_err());
        assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 2);
    }

    #[test]
    fn foreign_account_and_changed_database_never_publish() {
        let fixture = Fixture::new();
        let other = Fixture::new();
        let path = fixture.output.join("saved.json");
        assert!(KeyRing::prepare_output(&fixture.account.directory, &path)
            .unwrap()
            .publish(&other.ring())
            .is_err());
        assert!(!path.exists());
        let prepared = KeyRing::prepare_output(&fixture.account.directory, &path).unwrap();
        let mut changed = HEADER.to_vec();
        changed[200] ^= 1;
        fs::write(fixture.account.directory.join("a.db"), changed).unwrap();
        assert!(prepared.publish(&fixture.ring()).is_err());
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&fixture.output).unwrap().count(), 0);
    }

    #[test]
    fn saved_file_cannot_be_imported_by_another_account() {
        let fixture = Fixture::new();
        let other = Fixture::new();
        let path = fixture.output.join("saved.json");
        KeyRing::prepare_output(&fixture.account.directory, &path)
            .unwrap()
            .publish(&fixture.ring())
            .unwrap();
        let error = other
            .read(&KeyOptions {
                keys_file: Some(path),
                ..Default::default()
            })
            .err()
            .unwrap();
        assert!(error.to_string().contains("另一个账号"));
        assert!(!error.to_string().contains(HEX));
    }

    #[test]
    fn incomplete_ring_and_unsafe_output_are_rejected() {
        let fixture = Fixture::new();
        let path = fixture.output.join("saved.json");
        assert!(KeyRing::prepare_output(&fixture.account.directory, &path)
            .unwrap()
            .publish(&KeyRing::new(&fixture.account))
            .is_err());
        assert!(!path.exists());
        assert!(KeyRing::prepare_output(
            &fixture.account.directory,
            &fixture.account.directory.join("keys.json")
        )
        .is_err());
        assert!(
            KeyRing::prepare_output(&fixture.account.directory, Path::new("relative.json"))
                .is_err()
        );
        assert!(
            KeyRing::prepare_output(&fixture.account.directory, &fixture.output.join("a.db"))
                .is_err()
        );
        assert!(KeyRing::prepare_output(
            &fixture.account.directory,
            &fixture.output.join("missing").join("keys.json")
        )
        .is_err());
    }

    #[test]
    fn per_database_override_precedes_global_and_global_fills_missing() {
        use aes::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};
        let fixture = Fixture::new();
        let second = [0x35u8; 16];
        let key = enterprise::derive_page_key(&second, 1).unwrap();
        let iv = enterprise::generate_initial_vector(1).unwrap();
        let mut page = PLAIN[..enterprise::PAGE_SIZE].to_vec();
        let len = page.len();
        cbc::Encryptor::<aes::Aes128>::new((&key).into(), (&iv).into())
            .encrypt_padded_mut::<NoPadding>(&mut page, len)
            .unwrap();
        fs::write(fixture.account.directory.join("b.db"), page).unwrap();
        let global = fixture.output.join("global.txt");
        fs::write(&global, HEX).unwrap();
        let keys = fixture.document(serde_json::json!({"b.db": {"enc_key": "35".repeat(16)}}));
        let ring = fixture
            .read(&KeyOptions {
                key_file: Some(global),
                keys_file: Some(keys),
                auto_scan: None,
            })
            .unwrap();
        assert_eq!(
            ring.get(&fixture.account, "a.db"),
            Some(&enterprise::parse_key_hex(HEX).unwrap())
        );
        assert_eq!(ring.get(&fixture.account, "b.db"), Some(&second));
    }

    #[test]
    fn malformed_or_wrong_overrides_never_fall_back_or_scan() {
        let fixture = Fixture::new();
        let global = fixture.output.join("global.txt");
        fs::write(&global, HEX).unwrap();
        let cases = [
            serde_json::json!({"a.db": {"enc_key": "secret-invalid"}}),
            serde_json::json!({"a.db": {"enc_key": "35".repeat(16)}}),
            serde_json::json!({"a.db": {"enc_key": HEX, "salt": "00".repeat(16)}}),
            serde_json::json!({"unused.db": {"enc_key": "secret-invalid"}}),
            serde_json::json!({"unused.db": {"enc_key": HEX, "salt": "secret-invalid"}}),
            serde_json::json!({"a.db": {"salt": "00".repeat(16)}}),
            serde_json::json!({"../unused.db": {"enc_key": HEX}}),
            serde_json::json!({"a.db": {"enc_key": HEX}, "A.DB": {"enc_key": HEX}}),
        ];
        for entries in cases {
            let keys = fixture.document(entries);
            let options = KeyOptions {
                key_file: Some(global.clone()),
                keys_file: Some(keys),
                auto_scan: Some(ScanOptions {
                    authorized: true,
                    ..Default::default()
                }),
            };
            let (databases, _) = super::super::inventory(&fixture.account).unwrap();
            let error = obtain(&fixture.account, &databases, &options, Some(&NeverScan))
                .err()
                .unwrap()
                .to_string();
            assert!(!error.contains(HEX) && !error.contains("secret-invalid"));
        }
    }

    struct NeverScan;
    impl ProcessScanner for NeverScan {
        fn wxwork_pids(&self) -> Result<Vec<u32>> {
            panic!("坏覆盖项不得触发扫描")
        }
        fn open(&self, _: u32) -> Result<Box<dyn super::super::ProcessMemory>> {
            panic!("不得读取进程")
        }
    }
}
