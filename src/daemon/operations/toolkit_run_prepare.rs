//! 一键工作流的账号固定、进程检查与密钥准备；不负责参数解析或导出。

use anyhow::{Context, Result};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use crate::{crypto, runtime::RuntimeContext, scanner};

pub(super) struct Prepared {
    pub(super) runtime: RuntimeContext,
    pub(super) keys: HashMap<String, String>,
}

/// 调用方必须先解析参数及处理帮助；返回后直接消费快照，不重新发现配置。
pub(super) fn prepare() -> Result<Prepared> {
    prepare_with(RuntimeContext::load()?, check_wechat_running, |runtime| {
        scanner::scan_keys_with_options(&runtime.config.db_dir, &runtime.config.wechat_process)
    })
}

fn prepare_with(
    runtime: RuntimeContext,
    check: impl FnOnce(&str) -> Result<()>,
    scan: impl FnOnce(&RuntimeContext) -> Result<Vec<scanner::KeyEntry>>,
) -> Result<Prepared> {
    check(&runtime.config.wechat_process)?;
    anyhow::ensure!(
        runtime.config.db_dir.is_dir(),
        "配置中的账号数据库目录不存在"
    );
    let keys = match load_saved(&runtime) {
        Ok(keys) => keys,
        Err(error) => {
            // 路径歧义不能以自动提取掩盖，必须由用户修复。
            if error.downcast_ref::<UnsafeKeys>().is_some() {
                return Err(error);
            }
            eprintln!("已有密钥缺失或失效，正在为配置中的账号提取密钥...");
            let entries = scan(&runtime).context("当前账号密钥提取失败；旧密钥文件未修改")?;
            let mut keys = HashMap::new();
            for entry in entries {
                if keys.insert(entry.db_name, entry.enc_key).is_some() {
                    return Err(UnsafeKeys("扫描结果存在重复键").into());
                }
            }
            validate_keys(&runtime, &keys)?;
            save_keys(&runtime, &keys)?;
            keys
        }
    };
    Ok(Prepared { runtime, keys })
}

/// 离线入口只读加载，保留原始相对路径；不检查进程、不提取或写入密钥。
pub(super) fn load_saved(runtime: &RuntimeContext) -> Result<HashMap<String, String>> {
    let bytes = snapshot_keys(&runtime.config.keys_file)?.context("账号密钥文件不存在")?;
    let raw: RawKeys = serde_json::from_slice(&bytes).context("账号密钥文件格式错误")?;
    let mut seen = HashSet::new();
    let mut keys = HashMap::new();
    for (name, value) in raw.0 {
        if !seen.insert(name.clone()) {
            return Err(UnsafeKeys("密钥文件存在重复 JSON 属性").into());
        }
        if name == "_db_dir" {
            if value.is_null() || value.as_str() == Some("") || value == false || value == 0 {
                continue;
            }
            let saved = value.as_str().context("密钥目录元数据格式错误")?;
            // 相对目录元数据按配置所在目录解析，与配置解析规则一致。
            let path = PathBuf::from(saved);
            let path = if path.is_absolute() {
                path
            } else {
                runtime
                    .config_path
                    .parent()
                    .context("配置缺少父目录")?
                    .join(path)
            };
            anyhow::ensure!(
                same_path(&path, &runtime.config.db_dir)?,
                "密钥文件属于不同的数据库目录"
            );
        } else if !name.starts_with('_') {
            let key = value
                .as_str()
                .or_else(|| value.get("enc_key").and_then(|v| v.as_str()))
                .context("数据库密钥记录格式错误")?;
            keys.insert(name, key.to_owned());
        }
    }
    validate_paths(runtime, &keys)?;
    Ok(keys)
}

// 不先反序列化为 Map，避免重复 JSON 属性被 serde_json 静默覆盖。
struct RawKeys(Vec<(String, serde_json::Value)>);

impl<'de> Deserialize<'de> for RawKeys {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Entries;
        impl<'de> Visitor<'de> for Entries {
            type Value = RawKeys;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("密钥对象")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<RawKeys, M::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(RawKeys(entries))
            }
        }
        deserializer.deserialize_map(Entries)
    }
}

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

fn same_path(a: &Path, b: &Path) -> Result<bool> {
    Ok(a.canonicalize()?
        .as_os_str()
        .eq_ignore_ascii_case(b.canonicalize()?.as_os_str()))
}

fn validate_paths(runtime: &RuntimeContext, keys: &HashMap<String, String>) -> Result<()> {
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

pub(super) fn snapshot_keys(path: &Path) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("读取账号密钥文件失败"),
    };
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= 16 * 1024 * 1024,
        "账号密钥文件超过 16 MiB 限制"
    );
    Ok(Some(bytes))
}

pub(super) fn save_keys(runtime: &RuntimeContext, keys: &HashMap<String, String>) -> Result<()> {
    let target = &runtime.config.keys_file;
    // 不创建或修改配置，只替换本次配置明确指定的密钥文件。
    let parent = target.parent().context("密钥文件缺少父目录")?;
    let guard = crate::attachment::local_files::HostOutputGuard::new(parent)?;
    guard.verify_replaceable_file(target)?;
    validate_save_target(runtime)?;
    let original = snapshot_keys(target)?;
    let mut document = serde_json::Map::new();
    document.insert(
        "_db_dir".into(),
        serde_json::json!(runtime.config.db_dir.canonicalize()?),
    );
    for (name, key) in keys {
        document.insert(name.clone(), serde_json::json!({"enc_key": key}));
    }
    let bytes = zeroize::Zeroizing::new(serde_json::to_vec_pretty(&document)?);
    use zeroize::Zeroize;
    for value in document.values_mut() {
        if let Some(serde_json::Value::String(key)) = value.get_mut("enc_key") {
            key.zeroize();
        }
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent).context("无法创建密钥临时文件")?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    validate_save_target(runtime)?;
    guard.verify_replaceable_file(target)?;
    anyhow::ensure!(
        snapshot_keys(target)? == original,
        "密钥文件在保存期间变化，拒绝覆盖并发修改"
    );
    temporary
        .persist(target)
        .map_err(|error| error.error)
        .context("原子保存账号密钥失败")?;
    Ok(())
}

fn validate_save_target(runtime: &RuntimeContext) -> Result<()> {
    use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
    };
    let target = &runtime.config.keys_file;
    let parent = target
        .parent()
        .context("密钥文件缺少父目录")?
        .canonicalize()?;
    let resolved = parent.join(target.file_name().context("密钥文件缺少文件名")?);
    let db_root = runtime.config.db_dir.canonicalize()?;
    let config_path = runtime.config_path.canonicalize()?;
    anyhow::ensure!(
        !resolved.starts_with(&db_root),
        "密钥输出不得位于源数据库目录内"
    );
    anyhow::ensure!(
        !resolved
            .as_os_str()
            .eq_ignore_ascii_case(config_path.as_os_str()),
        "密钥输出不得覆盖配置文件"
    );
    match fs::symlink_metadata(target) {
        Ok(metadata) => {
            // Windows 重解析点包含符号链接；硬链接也不允许作为替换目标。
            anyhow::ensure!(
                metadata.is_file() && metadata.file_attributes() & 0x400 == 0,
                "密钥输出必须是普通文件，不能是链接"
            );
            anyhow::ensure!(
                !same_file::is_same_file(target, &runtime.config_path)?,
                "密钥输出不得覆盖配置文件别名"
            );
            let file = fs::File::open(target)?;
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }?;
            anyhow::ensure!(info.nNumberOfLinks == 1, "密钥输出不得覆盖硬链接文件");
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn check_wechat_running(process_name: &str) -> Result<()> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_NO_MORE_FILES};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    anyhow::ensure!(!process_name.trim().is_empty(), "wechat_process 不能为空");
    // scanner 的进程枚举为私有接口；这里只做只读名称检查，不打开进程内存。
    let snapshot =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.context("无法枚举微信进程")?;
    let result = (|| -> Result<()> {
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut next = unsafe { Process32FirstW(snapshot, &mut entry) };
        loop {
            if let Err(error) = next {
                if error.code() == ERROR_NO_MORE_FILES.to_hresult() {
                    break;
                }
                return Err(error).context("微信进程枚举失败");
            }
            let end = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..end]).eq_ignore_ascii_case(process_name)
            {
                return Ok(());
            }
            next = unsafe { Process32NextW(snapshot, &mut entry) };
        }
        anyhow::bail!("未检测到微信进程，请先启动微信并登录")
    })();
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

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
    fn valid_saved_keys_preserve_raw_names_and_do_not_scan() {
        let (_dir, runtime) = fixture();
        let e = entry(&runtime, "emoticon\\emoticon.db");
        save_keys(&runtime, &HashMap::from([(e.db_name.clone(), e.enc_key)])).unwrap();
        let bytes = fs::read(&runtime.config.keys_file).unwrap();
        let prepared = prepare_with(
            runtime,
            |name| {
                assert_eq!(name, "SyntheticWeChat.exe");
                Ok(())
            },
            |_| panic!("不应扫描"),
        )
        .unwrap();
        assert!(prepared.keys.contains_key(&e.db_name));
        assert_eq!(fs::read(&prepared.runtime.config.keys_file).unwrap(), bytes);
    }

    #[test]
    fn stopped_wechat_never_scans_or_writes_even_with_valid_keys() {
        let (_dir, runtime) = fixture();
        let e = entry(&runtime, "emoticon/emoticon.db");
        save_keys(&runtime, &HashMap::from([(e.db_name, e.enc_key)])).unwrap();
        let path = runtime.config.keys_file.clone();
        let bytes = fs::read(&path).unwrap();
        assert!(
            prepare_with(runtime, |_| anyhow::bail!("未运行"), |_| panic!("不应扫描")).is_err()
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn missing_malformed_empty_and_wrong_account_keys_are_refreshed() {
        for old in [
            None,
            Some("not json"),
            Some("{}"),
            Some(r#"{"_db_dir":"missing-account"}"#),
        ] {
            let (dir, runtime) = fixture();
            let e = entry(&runtime, "emoticon/emoticon.db");
            if let Some(old) = old {
                fs::write(&runtime.config.keys_file, old).unwrap();
            }
            fs::write(dir.path().join("unrelated-keys.json"), b"unrelated").unwrap();
            let prepared = prepare_with(
                runtime,
                |_| Ok(()),
                |r| {
                    assert_eq!(r.config.keys_file, dir.path().join("custom-keys.json"));
                    Ok(vec![e])
                },
            )
            .unwrap();
            assert_eq!(prepared.keys, load_saved(&prepared.runtime).unwrap());
            assert_eq!(
                fs::read(&prepared.runtime.config_path).unwrap(),
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
    fn failed_empty_or_invalid_scan_preserves_old_file() {
        for mode in 0..3 {
            let (_dir, runtime) = fixture();
            let mut e = entry(&runtime, "emoticon/emoticon.db");
            e.enc_key = "00".repeat(32);
            let path = runtime.config.keys_file.clone();
            fs::write(&path, b"old invalid file").unwrap();
            assert!(prepare_with(
                runtime,
                |_| Ok(()),
                |_| match mode {
                    0 => anyhow::bail!("合成扫描失败"),
                    1 => Ok(vec![]),
                    _ => Ok(vec![e]),
                }
            )
            .is_err());
            assert_eq!(fs::read(path).unwrap(), b"old invalid file");
        }
    }

    #[test]
    fn duplicate_and_unsafe_paths_are_errors_not_silent_refresh() {
        for names in [
            vec!["emoticon/emoticon.db", "EMOTICON\\emoticon.db"],
            vec!["same.db", "same.db"],
            vec!["../other.db"],
            vec!["C:\\other.db"],
            vec!["emoticon/../other.db"],
        ] {
            let (_dir, runtime) = fixture();
            let records: Vec<_> = names
                .iter()
                .map(|n| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(n).unwrap(),
                        serde_json::to_string(&"42".repeat(32)).unwrap()
                    )
                })
                .collect();
            fs::write(
                &runtime.config.keys_file,
                format!("{{{}}}", records.join(",")),
            )
            .unwrap();
            let result = prepare_with(runtime, |_| Ok(()), |_| panic!("不应扫描"));
            assert!(result.err().unwrap().downcast_ref::<UnsafeKeys>().is_some());
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
    fn existing_keys_with_missing_unrelated_db_and_empty_metadata_are_reused() {
        for metadata in [
            serde_json::Value::Null,
            serde_json::json!(""),
            serde_json::json!(false),
            serde_json::json!(0),
        ] {
            let (_dir, runtime) = fixture();
            let document = serde_json::json!({
                "_db_dir": metadata,
                "unrelated\\missing.db": {"enc_key": "old-unverified-value"}
            });
            fs::write(
                &runtime.config.keys_file,
                serde_json::to_vec(&document).unwrap(),
            )
            .unwrap();
            let prepared =
                prepare_with(runtime, |_| Ok(()), |_| panic!("已有映射不可触发扫描")).unwrap();
            assert_eq!(
                prepared.keys["unrelated\\missing.db"],
                "old-unverified-value"
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
            runtime.config.keys_file = match mode {
                0 => runtime.config_path.clone(),
                1 => source.clone(),
                _ => {
                    let alias = dir.path().join("alias.json");
                    fs::hard_link(&source, &alias).unwrap();
                    alias
                }
            };
            assert!(save_keys(&runtime, &HashMap::from([(e.db_name, e.enc_key)])).is_err());
            assert_eq!(fs::read(&runtime.config_path).unwrap(), config_before);
            assert_eq!(fs::read(source).unwrap(), source_before);
        }
    }
}
