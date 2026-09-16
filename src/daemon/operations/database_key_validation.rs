//! 验证 daemon 快照中的数据库密钥及其账号范围内文件身份。

use anyhow::{Context, Result};
use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    io::Read,
    path::PathBuf,
};

use crate::{crypto, runtime::RuntimeContext};

#[derive(Debug)]
struct UnsafeKeys(&'static str);
impl fmt::Display for UnsafeKeys {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for UnsafeKeys {}

fn relative_path(name: &str) -> Result<PathBuf> {
    let normalized = name.replace('\\', "/");
    if normalized.split('/').any(|part| {
        part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part.contains([':', '\0'])
    }) {
        return Err(UnsafeKeys("密钥含不安全的数据库相对路径").into());
    }
    Ok(PathBuf::from(normalized))
}

pub(super) fn validate_paths(
    runtime: &RuntimeContext,
    keys: &HashMap<String, String>,
) -> Result<()> {
    anyhow::ensure!(!keys.is_empty(), "未获取到任何数据库密钥");
    let root = runtime
        .config
        .db_dir
        .canonicalize()
        .context("无法解析账号数据库目录")?;
    let mut names = HashSet::new();
    let mut paths = HashSet::new();
    let mut targets = Vec::new();
    // 先检查全部路径，再读取任何数据库，确保冲突不会被失效密钥错误掩盖。
    for (name, key) in keys {
        let rel = relative_path(name)?;
        if !names.insert(name.replace('\\', "/").to_ascii_lowercase()) {
            return Err(UnsafeKeys("密钥存在规范化重名路径").into());
        }
        targets.push((root.join(rel), key));
    }
    for (path, _) in &mut targets {
        // 离线导出允许无关库缺失，但已有祖先联接也不能越过账号边界。
        let mut ancestor = path.clone();
        while !ancestor.exists() {
            anyhow::ensure!(ancestor.pop(), "无法解析数据库路径祖先");
        }
        if !ancestor.canonicalize()?.starts_with(&root) {
            return Err(UnsafeKeys("数据库路径越过当前账号目录").into());
        }
        if !path.exists() {
            continue;
        }
        *path = path.canonicalize()?;
        if !path.starts_with(&root) {
            return Err(UnsafeKeys("数据库路径越过当前账号目录").into());
        }
        if !paths.insert(path.to_string_lossy().to_ascii_lowercase()) {
            return Err(UnsafeKeys("密钥存在指向同一文件的路径").into());
        }
    }
    Ok(())
}

pub(super) fn validate_keys(
    runtime: &RuntimeContext,
    keys: &HashMap<String, String>,
) -> Result<()> {
    validate_paths(runtime, keys)?;
    for (name, key) in keys {
        let path = runtime.config.db_dir.join(relative_path(name)?);
        let key = decode_key(key)?;
        let guard = crate::attachment::local_files::HostOutputGuard::new(
            path.parent().context("数据库缺少父目录")?,
        )?;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // 活动数据库允许写入，但本次验证期间不允许删除或改名。
            options.share_mode(1 | 2).custom_flags(0x00200000);
        }
        let mut file = options.open(&path).context("无法打开当前账号数据库")?;
        let metadata = file.metadata()?;
        anyhow::ensure!(metadata.is_file(), "数据库必须是普通文件");
        #[cfg(windows)]
        {
            use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
            use windows::Win32::{
                Foundation::HANDLE,
                Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
            };
            anyhow::ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "数据库不能是重解析点"
            );
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            unsafe {
                GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?;
            }
            anyhow::ensure!(info.nNumberOfLinks == 1, "数据库不能是多硬链接文件");
        }
        let identity = same_file::Handle::from_file(file.try_clone()?)?;
        anyhow::ensure!(
            identity == same_file::Handle::from_path(&path)?,
            "数据库身份在打开期间变化"
        );
        let mut page = [0; crypto::PAGE_SZ];
        file.read_exact(&mut page).context("无法读取数据库首页")?;
        guard.verify()?;
        anyhow::ensure!(
            identity == same_file::Handle::from_path(&path)?,
            "数据库身份在验证期间变化"
        );
        anyhow::ensure!(
            crypto::verify_page1(&key, &page),
            "已有数据库密钥未通过首页 HMAC 验证"
        );
    }
    Ok(())
}

fn decode_key(value: &str) -> Result<zeroize::Zeroizing<[u8; 32]>> {
    anyhow::ensure!(
        value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()),
        "数据库密钥必须是 32 字节十六进制"
    );
    let mut key = zeroize::Zeroizing::new([0; 32]);
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16)?;
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::scanner;

    fn load_saved(runtime: &RuntimeContext) -> Result<HashMap<String, String>> {
        let keys = crate::key_store::Store::for_runtime(runtime)?
            .load()?
            .database_keys();
        validate_paths(runtime, &keys)?;
        Ok(keys)
    }

    fn save_keys(runtime: &RuntimeContext, keys: &HashMap<String, String>) -> Result<()> {
        validate_keys(runtime, keys)?;
        crate::key_store::Store::for_runtime(runtime)?.update(
            None,
            &[crate::key_store::Update::Databases(
                keys,
                crate::key_store::Verification::Verified,
            )],
        )?;
        Ok(())
    }

    fn fixture() -> (tempfile::TempDir, RuntimeContext) {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        fs::create_dir(base.join("db")).unwrap();
        fs::write(
            base.join("config.json"),
            b"synthetic config; must not be rewritten",
        )
        .unwrap();
        let runtime = RuntimeContext {
            config: Config {
                key_store: Some(base.join("keys.dpapi")),
                db_dir: base.join("db"),
                keys_file: base.join("custom-keys.json"),
                decrypted_dir: base.join("out"),
                wechat_process: "SyntheticWeChat.exe".into(),
            },
            config_path: base.join("config.json"),
            root: base.join("home"),
            id: "synthetic".into(),
            directory: base.join("home/account"),
        };
        (dir, runtime)
    }

    fn entry(runtime: &RuntimeContext, name: &str) -> scanner::KeyEntry {
        use hmac::{Hmac, Mac};
        use sha2::Sha512;
        let mut page = [0x21; crypto::PAGE_SZ];
        let key = [0x42; 32];
        let salt = [0x21 ^ 0x3a; 16];
        let mut mac_key = [0; 32];
        pbkdf2::pbkdf2_hmac::<Sha512>(&key, &salt, 2, &mut mac_key);
        let mut mac = Hmac::<Sha512>::new_from_slice(&mac_key).unwrap();
        mac.update(&page[16..4032]);
        mac.update(&1u32.to_le_bytes());
        page[4032..].copy_from_slice(&mac.finalize().into_bytes());
        let path = runtime.config.db_dir.join(relative_path(name).unwrap());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, page).unwrap();
        scanner::KeyEntry {
            db_name: name.into(),
            enc_key: "42".repeat(32),
            salt: "21".repeat(16),
        }
    }

    #[test]
    fn valid_saved_keys_normalize_names() {
        let (_dir, runtime) = fixture();
        let e = entry(&runtime, "emoticon\\emoticon.db");
        save_keys(&runtime, &HashMap::from([(e.db_name.clone(), e.enc_key)])).unwrap();
        let bytes = fs::read(runtime.config.key_store.as_ref().unwrap()).unwrap();
        let keys = load_saved(&runtime).unwrap();
        assert!(keys.contains_key("emoticon/emoticon.db"));
        assert_eq!(
            fs::read(runtime.config.key_store.as_ref().unwrap()).unwrap(),
            bytes
        );
    }

    #[test]
    fn missing_or_corrupt_store_never_triggers_acquisition_or_writes() {
        for old in [
            None,
            Some("not json"),
            Some("{}"),
            Some(r#"{"_db_dir":"missing-account"}"#),
        ] {
            let (dir, runtime) = fixture();
            entry(&runtime, "emoticon/emoticon.db");
            if let Some(old) = old {
                fs::write(runtime.config.key_store.as_ref().unwrap(), old).unwrap();
            }
            fs::write(dir.path().join("unrelated-keys.json"), b"unrelated").unwrap();
            let before = fs::read(runtime.config.key_store.as_ref().unwrap()).ok();
            assert!(load_saved(&runtime).is_err());
            assert_eq!(
                fs::read(runtime.config.key_store.as_ref().unwrap()).ok(),
                before
            );
            assert_eq!(
                fs::read(&runtime.config_path).unwrap(),
                b"synthetic config; must not be rewritten"
            );
            assert_eq!(
                fs::read(dir.path().join("unrelated-keys.json")).unwrap(),
                b"unrelated"
            );
            assert!(!dir.path().join("all_keys.json").exists());
        }
    }

    #[test]
    fn empty_or_unsafe_material_preserves_old_file() {
        for empty in [true, false] {
            let (_dir, runtime) = fixture();
            let mut e = entry(&runtime, "emoticon/emoticon.db");
            e.db_name = "../outside.db".into();
            let path = runtime.config.keys_file.clone();
            fs::write(&path, b"old invalid file").unwrap();
            let keys = if empty {
                HashMap::new()
            } else {
                HashMap::from([(e.db_name, e.enc_key)])
            };
            assert!(validate_paths(&runtime, &keys).is_err());
            assert_eq!(fs::read(path).unwrap(), b"old invalid file");
        }
    }

    #[test]
    fn offline_loading_leaves_hmac_checks_to_run_and_catalog() {
        let (_dir, runtime) = fixture();
        let e = entry(&runtime, "emoticon/emoticon.db");
        save_keys(&runtime, &HashMap::from([(e.db_name.clone(), e.enc_key)])).unwrap();
        assert!(load_saved(&runtime).is_ok());
        let path = runtime.config.db_dir.join(&e.db_name);
        let mut page = fs::read(&path).unwrap();
        page[64] ^= 1;
        fs::write(path, page).unwrap();
        assert!(load_saved(&runtime).is_ok());
        assert!(validate_keys(&runtime, &load_saved(&runtime).unwrap()).is_err());
        fs::remove_file(runtime.config.db_dir.join(&e.db_name)).unwrap();
        assert!(load_saved(&runtime).is_ok());
    }

    #[test]
    fn legacy_unverified_mapping_cannot_be_silently_reused_by_runtime() {
        for metadata in [
            serde_json::Value::Null,
            serde_json::json!(""),
            serde_json::json!(false),
            serde_json::json!(0),
        ] {
            let (_dir, mut runtime) = fixture();
            runtime.config.key_store = None;
            let document = serde_json::json!({
                "_db_dir": metadata,
                "unrelated\\missing.db": {"enc_key": "old-unverified-value"}
            });
            fs::write(
                &runtime.config.keys_file,
                serde_json::to_vec(&document).unwrap(),
            )
            .unwrap();
            let error = load_saved(&runtime).err().unwrap();
            assert_eq!(
                error.downcast_ref::<crate::key_store::Error>(),
                Some(&crate::key_store::Error::LegacyMigrationRequired)
            );
        }
    }

    #[test]
    fn save_rejects_config_source_database_and_hardlink_targets() {
        for mode in 0..3 {
            let (dir, mut runtime) = fixture();
            let e = entry(&runtime, "emoticon/emoticon.db");
            let source = runtime.config.db_dir.join(&e.db_name);
            let config_before = fs::read(&runtime.config_path).unwrap();
            let source_before = fs::read(&source).unwrap();
            runtime.config.key_store = Some(match mode {
                0 => runtime.config_path.clone(),
                1 => source.clone(),
                _ => {
                    let alias = dir.path().join("alias.json");
                    fs::hard_link(&source, &alias).unwrap();
                    alias
                }
            });
            assert!(save_keys(&runtime, &HashMap::from([(e.db_name, e.enc_key)])).is_err());
            assert_eq!(fs::read(&runtime.config_path).unwrap(), config_before);
            assert_eq!(fs::read(source).unwrap(), source_before);
        }
    }
}
