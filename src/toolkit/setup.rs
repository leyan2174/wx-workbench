//! 首次配置及 init 共用的安全读写：不依赖数据库密钥或 RuntimeContext。
use crate::attachment::local_files::HostOutputGuard;
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use zeroize::Zeroizing;

const MAX_JSON: u64 = 16 * 1024 * 1024;

// 不派生 Debug，配置快照可能含旧版保存的明文凭据。
pub(crate) struct Snapshot {
    pub path: PathBuf,
    bytes: Option<Zeroizing<Vec<u8>>>,
}

pub(crate) struct ConfigDocument {
    pub snapshot: Snapshot,
    pub value: Value,
}

pub(crate) struct ConfigLock {
    _file: File,
    _guard: HostOutputGuard,
}

pub(crate) struct InitPaths {
    pub keys_file: PathBuf,
    pub account_key_file: PathBuf,
    pub protected: Vec<PathBuf>,
}

pub(crate) fn exists(path: &Path) -> Result<bool> {
    Ok(inspect(path)?.2)
}

fn safe_component(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && !name.ends_with([' ', '.'])
            && !name
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
        "不安全的配置路径组件"
    );
    let stem = name.split('.').next().unwrap_or_default().to_uppercase();
    let number = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    ensure!(
        !matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) && !number.is_some_and(|n| matches!(
            n,
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )),
        "不能使用设备路径"
    );
    Ok(())
}

fn local_path(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "配置路径必须为绝对路径");
    for part in path.components() {
        match part {
            Component::Prefix(p) => ensure!(
                matches!(
                    p.kind(),
                    std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                ),
                "只允许本地磁盘路径"
            ),
            Component::RootDir => (),
            Component::Normal(name) => safe_component(name.to_str().context("路径编码无效")?)?,
            _ => anyhow::bail!("配置路径不能包含相对跳转"),
        }
    }
    Ok(())
}

pub(crate) fn resolve(base: &Path, path: &Path) -> Result<PathBuf> {
    ensure!(!path.as_os_str().is_empty(), "路径不能为空");
    let path = if path.is_absolute() {
        path.into()
    } else {
        base.join(path)
    };
    local_path(&path)?;
    Ok(path)
}

// 逐级固定祖先后再访问下一级，避免先穿过目录联接再事后拒绝。
// 缺失尾部只做投影，不创建目录；返回最近已有目录的守卫。
fn inspect(path: &Path) -> Result<(PathBuf, HostOutputGuard, bool)> {
    local_path(path)?;
    let mut current = PathBuf::new();
    let mut resolved = PathBuf::new();
    let mut guard: Option<HostOutputGuard> = None;
    let mut missing = false;
    for part in path.components() {
        current.push(part);
        if !current.is_absolute() {
            continue;
        }
        if missing {
            resolved.push(part);
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() => {
                let next = HostOutputGuard::new(&current)?;
                resolved = fs::canonicalize(&current)?;
                guard = Some(next);
            }
            Ok(_) => {
                ensure!(current == path, "配置路径的祖先不是普通目录");
                guard
                    .as_ref()
                    .context("文件缺少父目录")?
                    .verify_replaceable_file(&current)?;
                resolved = fs::canonicalize(&current)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ensure!(guard.is_some(), "配置路径缺少已有磁盘根目录");
                missing = true;
                resolved.push(part);
            }
            Err(error) => return Err(error).context("无法访问配置路径祖先"),
        }
    }
    let guard = guard.context("配置路径缺少已有目录")?;
    guard.verify()?;
    Ok((
        PathBuf::from(resolved.to_string_lossy().to_lowercase()),
        guard,
        !missing,
    ))
}

fn projected(path: &Path) -> Result<PathBuf> {
    Ok(inspect(path)?.0)
}

pub(crate) fn same_path(left: &Path, right: &Path) -> Result<bool> {
    Ok(projected(left)? == projected(right)?)
}

pub(crate) fn check_target(path: &Path, protected: &[PathBuf]) -> Result<()> {
    let (target, guard, present) = inspect(path)?;
    if present {
        guard.verify_replaceable_file(path)?;
    }
    for input in protected {
        let source = projected(input)?;
        ensure!(
            !target.starts_with(&source) && !source.starts_with(&target),
            "配置输出与受保护输入重叠"
        );
    }
    Ok(())
}

fn parent_guard(path: &Path) -> Result<HostOutputGuard> {
    local_path(path)?;
    let parent = path.parent().context("配置输出缺少父目录")?;
    let (_, mut guard, present) = inspect(parent)?;
    if present {
        ensure!(guard.output_root() == parent, "配置输出的父路径不是目录");
        return Ok(guard);
    }
    let mut current = guard.output_root().to_path_buf();
    let missing = parent.strip_prefix(&current)?.to_path_buf();
    for part in missing.components() {
        guard.verify()?;
        current.push(part);
        match fs::create_dir(&current) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e).context("创建配置父目录失败"),
        }
        guard = HostOutputGuard::new(&current)?;
    }
    Ok(guard)
}

impl Snapshot {
    pub fn capture(path: &Path) -> Result<Self> {
        let (_, guard, present) = inspect(path)?;
        if !present {
            return Ok(Self {
                path: path.into(),
                bytes: None,
            });
        }
        guard.verify_replaceable_file(path)?;
        use std::os::windows::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .custom_flags(0x00200000)
            .open(path)?;
        let metadata = file.metadata()?;
        ensure!(metadata.len() <= MAX_JSON, "配置或密钥 JSON 超过大小限制");
        let mut bytes = Zeroizing::new(Vec::new());
        (&file).take(MAX_JSON + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == metadata.len() && bytes.len() as u64 <= MAX_JSON,
            "读取期间配置文件变化或超过大小限制"
        );
        ensure!(
            same_file::Handle::from_file(file.try_clone()?)? == same_file::Handle::from_path(path)?,
            "配置文件身份变化"
        );
        guard.verify_replaceable_file(path)?;
        Ok(Self {
            path: path.into(),
            bytes: Some(bytes),
        })
    }

    pub fn existed(&self) -> bool {
        self.bytes.is_some()
    }

    pub fn verify(&self) -> Result<()> {
        let now = Self::capture(&self.path)?;
        ensure!(
            self.bytes.as_ref().map(|b| b.as_slice()) == now.bytes.as_ref().map(|b| b.as_slice()),
            "配置或密钥文件已被其他操作修改，请重新运行"
        );
        Ok(())
    }

    /// 调用者持有 ConfigLock；仅逐文件原子发布，不声称两个文件构成事务。
    pub fn write_json(&self, value: &Value, protected: &[PathBuf]) -> Result<()> {
        check_target(&self.path, protected)?;
        let guard = parent_guard(&self.path)?;
        let bytes = Zeroizing::new(serde_json::to_vec_pretty(value)?);
        ensure!(
            bytes.len() as u64 <= MAX_JSON,
            "配置或密钥 JSON 超过大小限制"
        );
        let mut temporary = tempfile::NamedTempFile::new_in(guard.output_root())?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        self.verify()?;
        check_target(&self.path, protected)?;
        guard.verify_replaceable_file(&self.path)?;
        // 首次创建禁止覆盖并发出现的文件；现有文件通过锁和内容快照防止陈旧写入。
        if self.existed() {
            temporary.persist(&self.path).map_err(|e| e.error)?;
        } else {
            temporary
                .persist_noclobber(&self.path)
                .map_err(|e| e.error)?;
        }
        Ok(())
    }
}

pub(crate) fn text<'a>(value: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match value.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => anyhow::bail!("配置字段 {key} 必须为字符串"),
    }
}

impl ConfigDocument {
    pub fn load(path: &Path) -> Result<Self> {
        let snapshot = Snapshot::capture(path).context("无法安全读取选中配置")?;
        let value = match &snapshot.bytes {
            None => json!({}),
            Some(bytes) => serde_json::from_slice(bytes)
                .map_err(|_| anyhow::anyhow!("选中配置 JSON 无效，未覆盖文件"))?,
        };
        ensure!(value.is_object(), "选中配置必须为 JSON 对象，未覆盖文件");
        for key in ["db_dir", "keys_file", "decrypted_dir", "wechat_process"] {
            if let Some(value) = text(&value, key)? {
                ensure!(
                    key == "db_dir" || !value.trim().is_empty(),
                    "配置字段 {key} 不能为空"
                );
            }
        }
        Ok(Self { snapshot, value })
    }

    pub fn base(&self) -> &Path {
        self.snapshot.path.parent().expect("配置路径已经验证")
    }

    pub fn path_field(&self, value: &Value, field: &str, default: &str) -> Result<PathBuf> {
        resolve(
            self.base(),
            Path::new(text(value, field)?.unwrap_or(default)),
        )
    }

    pub fn configured_db(&self) -> Result<Option<PathBuf>> {
        text(&self.value, "db_dir")?
            .filter(|s| !s.is_empty() && !s.contains("your_wxid"))
            .map(|s| resolve(self.base(), Path::new(s)))
            .transpose()
    }

    pub fn ensure_account(&self, db: &Path) -> Result<()> {
        let _guard = HostOutputGuard::new(db).context("选中账号目录不安全或不存在")?;
        if let Some(previous) = self.configured_db()? {
            ensure!(same_path(&previous, db)?, "现有配置已绑定另一账号；请使用另一 WX_CLI_CONFIG 或 --config-path，不能原地串号覆盖");
        } else {
            let keys = self.path_field(&self.value, "keys_file", "all_keys.json")?;
            ensure!(
                !exists(&keys)? && !exists(&self.base().join("account_key.dpapi"))?,
                "配置尚未绑定账号但已有密钥文件；无法确认归属，请为该账号选择独立配置目录"
            );
        }
        Ok(())
    }

    pub fn with_db(&self, db: &Path) -> Result<Value> {
        self.ensure_account(db)?;
        let mut value = self.value.clone();
        let map = value.as_object_mut().expect("已验证对象");
        map.insert(
            "db_dir".into(),
            Value::String(db.to_str().context("账号路径编码无效")?.into()),
        );
        map.entry("keys_file")
            .or_insert_with(|| json!("all_keys.json"));
        map.entry("decrypted_dir")
            .or_insert_with(|| json!("decrypted"));
        Ok(value)
    }

    fn lock_path(&self) -> Result<PathBuf> {
        let name = self
            .snapshot
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .context("配置文件名无效")?;
        Ok(self.base().join(format!(".{name}.wx-setup.lock")))
    }

    pub fn validate_targets(&self, value: &Value) -> Result<InitPaths> {
        let db = self.path_field(value, "db_dir", "")?;
        self.ensure_account(&db)?;
        let account = if db
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("db_storage"))
        {
            db.parent().context("账号路径缺少父目录")?.to_path_buf()
        } else {
            db.clone()
        };
        let protected = vec![
            account,
            self.path_field(value, "decrypted_dir", "decrypted")?,
            std::path::absolute(crate::config::cli_dir())?.join("accounts"),
        ];
        let keys_file = self.path_field(value, "keys_file", "all_keys.json")?;
        let account_key_file = self.base().join("account_key.dpapi");
        let targets = [&self.snapshot.path, &keys_file, &account_key_file];
        for (index, target) in targets.iter().enumerate() {
            check_target(target, &protected)?;
            ensure!(
                !same_path(target, &self.lock_path()?)?,
                "配置输出与锁文件冲突"
            );
            for other in &targets[..index] {
                ensure!(
                    !same_path(target, other)?,
                    "配置、密钥及账号密钥不能共用文件"
                );
            }
        }
        Ok(InitPaths {
            keys_file,
            account_key_file,
            protected,
        })
    }

    pub fn lock(&self) -> Result<ConfigLock> {
        let path = self.lock_path()?;
        let guard = parent_guard(&path)?;
        guard.verify_replaceable_file(&path)?;
        use std::os::windows::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .custom_flags(0x00200000)
            .open(&path)
            .context("选中配置正在被另一个初始化操作使用")?;
        use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
        use windows::Win32::{
            Foundation::HANDLE,
            Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // 独占句柄固定锁文件身份；再次拒绝打开前被换入的重解析点或硬链接。
        unsafe {
            GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?;
        }
        ensure!(
            file.metadata()?.file_attributes() & 0x400 == 0 && info.nNumberOfLinks == 1,
            "配置锁文件不安全"
        );
        guard.verify()?;
        self.snapshot.verify()?;
        Ok(ConfigLock {
            _file: file,
            _guard: guard,
        })
    }
}

pub(crate) fn valid_env_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 128
            && name.bytes().enumerate().all(|(i, b)| b == b'_'
                || b.is_ascii_alphabetic()
                || (i > 0 && b.is_ascii_digit())),
        "凭据环境变量名无效；此参数只接受变量名，不接受 API key"
    );
    Ok(())
}

fn env_present(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(Zeroizing::new)
        .is_some_and(|value| !value.trim().is_empty())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn invalid_documents_are_preserved_byte_for_byte() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        for original in [
            "{broken",
            "[]",
            "null",
            r#"{"keys_file":17}"#,
            r#"{"db_dir":false}"#,
        ] {
            fs::write(&path, original).unwrap();
            assert!(ConfigDocument::load(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
        }
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn stale_snapshot_never_replaces_new_or_modified_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        let missing = ConfigDocument::load(&path).unwrap();
        let _lock = missing.lock().unwrap();
        fs::write(&path, b"concurrent creator").unwrap();
        assert!(missing
            .snapshot
            .write_json(&json!({"replace": true}), &[])
            .is_err());
        assert_eq!(fs::read(&path).unwrap(), b"concurrent creator");
        let existing = Snapshot::capture(&path).unwrap();
        fs::write(&path, b"concurrent editor").unwrap();
        assert!(existing.write_json(&json!({"replace": true}), &[]).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"concurrent editor");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    }

    #[test]
    fn init_paths_resolve_custom_keys_and_publish_only_selected_file() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("account/db_storage");
        fs::create_dir_all(&db).unwrap();
        let config = root.path().join("config.json");
        let original = serde_json::to_vec(&json!({
            "db_dir": db, "keys_file": "private/selected.json", "unknown": {"keep": true}
        }))
        .unwrap();
        fs::write(&config, &original).unwrap();
        let document = ConfigDocument::load(&config).unwrap();
        let value = document.with_db(&db).unwrap();
        let paths = document.validate_targets(&value).unwrap();
        assert_eq!(paths.keys_file, root.path().join("private/selected.json"));
        assert_eq!(
            paths.account_key_file,
            root.path().join("account_key.dpapi")
        );
        assert_eq!(value["unknown"], json!({"keep": true}));
        let _lock = document.lock().unwrap();
        let snapshot = Snapshot::capture(&paths.keys_file).unwrap();
        let mut protected = paths.protected;
        protected.extend([config.clone(), paths.account_key_file]);
        let synthetic = json!({"fixture.db": {"enc_key": "SYNTHETIC_NOT_A_REAL_KEY"}});
        snapshot.write_json(&synthetic, &protected).unwrap();
        let saved: Value = serde_json::from_slice(&fs::read(paths.keys_file).unwrap()).unwrap();
        assert_eq!(saved, synthetic);
        assert_eq!(fs::read(config).unwrap(), original);
        assert!(!root.path().join("all_keys.json").exists());
        assert!(!root.path().join("account_key.dpapi").exists());
        assert_eq!(fs::read_dir(db).unwrap().count(), 0);
    }

    #[test]
    fn account_switch_and_output_aliases_are_rejected_without_writes() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("account/db_storage");
        let other = root.path().join("other/db_storage");
        fs::create_dir_all(&db).unwrap();
        fs::create_dir_all(&other).unwrap();
        let config = root.path().join("config.json");
        let original = serde_json::to_vec(&json!({"db_dir": db})).unwrap();
        fs::write(&config, &original).unwrap();
        let document = ConfigDocument::load(&config).unwrap();
        assert!(document.with_db(&other).is_err());
        for target in [
            config.clone(),
            db.join("keys.json"),
            root.path().join("account_key.dpapi"),
        ] {
            let mut value = document.with_db(&db).unwrap();
            value["keys_file"] = json!(target);
            assert!(document.validate_targets(&value).is_err());
        }
        assert_eq!(fs::read(config).unwrap(), original);
        assert!(!root.path().join(".config.json.wx-setup.lock").exists());
        assert_eq!(fs::read_dir(db).unwrap().count(), 0);
    }

    #[test]
    fn environment_report_and_validation_errors_do_not_echo_credentials() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.json");
        let secret = "sk-SYNTHETIC-SECRET-DO-NOT-ECHO";
        let value = json!({
            "transcription_backend": "openai", "openai_api_key_env": "WX_SETUP_SYNTHETIC_CREDENTIAL",
            "openai_api_key": secret, "unknown": {"secret": secret},
            "whisper_cpp_binary": root.path().join("absent.exe"),
            "whisper_cpp_model": root.path().join("absent-model.bin")
        });
        let report = environment(&config, &value, false).unwrap();
        assert!(!serde_json::to_string(&report).unwrap().contains(secret));
        assert_eq!(report["openai"]["legacy_inline_credential_present"], true);
        assert_eq!(
            report["openai"]["credential_environment"],
            "WX_SETUP_SYNTHETIC_CREDENTIAL"
        );
        assert!(report["openai"]["environment_present"].is_boolean());
        assert_eq!(report["openai"]["network"], "not_checked");
        assert_eq!(report["scanner"], "not_run");
        assert_eq!(report["downloads"], "not_run");
        let error = valid_env_name(secret).unwrap_err();
        assert!(!format!("{error:#}").contains(secret));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

fn file_available(path: &Path) -> bool {
    inspect(path)
        .is_ok_and(|(_, guard, present)| present && guard.verify_replaceable_file(path).is_ok())
}

fn binary_available(base: &Path, configured: &Path) -> bool {
    if configured.is_absolute() || configured.components().count() != 1 {
        return resolve(base, configured).is_ok_and(|p| file_available(&p));
    }
    let Some(search) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&search)
        .take(512)
        .filter(|p| p.is_absolute())
        .any(|directory| {
            [configured.to_path_buf(), configured.with_extension("exe")]
                .iter()
                .any(|name| file_available(&directory.join(name)))
        })
}

/// 只检查声明及本地文件是否存在，不运行解释器、导入包、加载模型或测试凭据。
pub(crate) fn environment(config_path: &Path, value: &Value, config_exists: bool) -> Result<Value> {
    let base = config_path.parent().context("配置缺少父目录")?;
    let python = std::env::var_os("WX_WECHAT_DECRYPT_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| "python.exe".into());
    let cpp = text(value, "whisper_cpp_binary")?;
    let cpp_model = text(value, "whisper_cpp_model")?;
    let backend = text(value, "transcription_backend")?.unwrap_or("local");
    let backend_supported = ["local", "whisper_cpp", "openai"].contains(&backend);
    let credential = text(value, "openai_api_key_env")?.unwrap_or("OPENAI_API_KEY");
    valid_env_name(credential)?;
    Ok(json!({
        "platform": std::env::consts::OS, "architecture": std::env::consts::ARCH,
        "config_path": config_path, "config_exists": config_exists,
        "config_status": if config_exists { "valid" } else { "missing" },
        "native_setup": true, "keys_required": false, "runtime_required": false,
        "transcription_backend": if backend_supported { backend } else { "unsupported" },
        "backend_supported": backend_supported,
        "python": {"executable_found": binary_available(base, &python), "packages": "not_checked", "venv_env_present": env_present("VIRTUAL_ENV")},
        "whisper_cpp": {"binary_found": cpp.map(|p| binary_available(base, Path::new(p))).unwrap_or_else(|| binary_available(base, Path::new("whisper-cli.exe")) || binary_available(base, Path::new("whisper-cpp.exe"))),
            "model_file_found": cpp_model.and_then(|p| resolve(base, Path::new(p)).ok()).is_some_and(|p| file_available(&p)), "inference": "not_checked"},
        "openai": {"credential_environment": credential, "environment_present": env_present(credential),
            "legacy_inline_credential_present": text(value, "openai_api_key")?.is_some_and(|s| !s.trim().is_empty()),
            "network": "not_checked", "env_reference_consumer": "pending_integration", "upload_authorized": false},
        "account_discovery": "not_run", "scanner": "not_run", "downloads": "not_run"
    }))
}
