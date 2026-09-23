use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub db_dir: PathBuf,
    pub keys_file: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_store: Option<PathBuf>,
    pub decrypted_dir: PathBuf,
    #[serde(default)]
    pub wechat_process: String,
}

/// 从指定文件加载一次配置，避免后台启动过程中重复发现配置而切换账号。
pub(crate) fn load_config_at(config_path: &Path) -> Result<Config> {
    let file = std::fs::File::open(config_path)
        .with_context(|| format!("读取 config.json 失败: {}", config_path.display()))?;
    anyhow::ensure!(
        file.metadata()
            .with_context(|| format!("读取 config.json 失败: {}", config_path.display()))?
            .len()
            <= MAX_CONFIG_BYTES,
        "配置超过读取限额"
    );
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("读取 config.json 失败: {}", config_path.display()))?;
    anyhow::ensure!(bytes.len() as u64 <= MAX_CONFIG_BYTES, "配置超过读取限额");
    let content = std::str::from_utf8(&bytes)
        .with_context(|| format!("读取 config.json 失败: {}", config_path.display()))?;
    let raw: serde_json::Value =
        serde_json::from_str(content).with_context(|| "config.json 格式错误")?;
    validate_key_configuration(&raw)?;

    let mut db_dir = raw
        .get("db_dir")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(default_db_dir);

    let base_dir = config_path.parent().unwrap_or(Path::new("."));
    if db_dir.is_relative() {
        db_dir = base_dir.join(db_dir);
    }

    let keys_file = raw
        .get("keys_file")
        .and_then(|v| v.as_str())
        .map(|s| {
            let p = PathBuf::from(s);
            if p.is_absolute() {
                p
            } else {
                base_dir.join(p)
            }
        })
        .unwrap_or_else(|| base_dir.join("all_keys.json"));

    let key_store = match raw.get("key_store") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(value)) if !value.is_empty() => {
            let path = PathBuf::from(value);
            Some(if path.is_absolute() {
                path
            } else {
                base_dir.join(path)
            })
        }
        _ => anyhow::bail!("key_store must be a nonempty path reference"),
    };

    let decrypted_dir = raw
        .get("decrypted_dir")
        .and_then(|v| v.as_str())
        .map(|s| {
            let p = PathBuf::from(s);
            if p.is_absolute() {
                p
            } else {
                base_dir.join(p)
            }
        })
        .unwrap_or_else(|| base_dir.join("decrypted"));

    let wechat_process = raw
        .get("wechat_process")
        .and_then(|v| v.as_str())
        .unwrap_or(default_wechat_process())
        .to_string();

    Ok(Config {
        db_dir,
        keys_file,
        key_store,
        decrypted_dir,
        wechat_process,
    })
}

pub(crate) fn validate_key_configuration(raw: &serde_json::Value) -> Result<()> {
    anyhow::ensure!(raw.is_object(), "Configuration must be a JSON object");
    anyhow::ensure!(
        raw.get("image_aes_key").is_none() && raw.get("image_xor_key").is_none(),
        "Legacy inline image keys are unsupported; remove the legacy fields from the selected configuration and initialize image material in key_store. No files were changed"
    );
    Ok(())
}

pub(crate) fn find_config_file() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("WX_CLI_CONFIG") {
        anyhow::ensure!(!path.is_empty(), "WX_CLI_CONFIG 不能为空");
        return Ok(std::path::absolute(PathBuf::from(path))?);
    }
    let cwd_dir = std::env::current_dir().ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from));
    // 自定义运行根目录必须与 init 的写入位置一致，不能悄悄回退到默认账号。
    if std::env::var("WX_CLI_HOME").is_ok_and(|value| !value.trim().is_empty()) {
        if let Some(path) = find_existing_config_path(cwd_dir.as_deref(), exe_dir.as_deref(), None)
        {
            return Ok(path);
        }
        return Ok(cli_dir().join("config.json"));
    }
    let cli_home = cli_home_dir();
    let home_dir = Some(cli_home.as_path());

    if let Some(path) = find_existing_config_path(cwd_dir.as_deref(), exe_dir.as_deref(), home_dir)
    {
        return Ok(path);
    }

    Ok(default_config_path(
        cwd_dir.as_deref(),
        exe_dir.as_deref(),
        home_dir,
    ))
}

fn find_existing_config_path(
    cwd_dir: Option<&Path>,
    exe_dir: Option<&Path>,
    home_dir: Option<&Path>,
) -> Option<PathBuf> {
    let candidates = [
        cwd_dir.map(config_path_in_dir),
        exe_dir.map(config_path_in_dir),
        home_dir.map(home_config_path),
    ];
    candidates.into_iter().flatten().find(|path| path.exists())
}

fn default_config_path(
    cwd_dir: Option<&Path>,
    exe_dir: Option<&Path>,
    home_dir: Option<&Path>,
) -> PathBuf {
    cwd_dir
        .map(config_path_in_dir)
        .or_else(|| exe_dir.map(config_path_in_dir))
        .or_else(|| home_dir.map(home_config_path))
        .unwrap_or_else(|| PathBuf::from("config.json"))
}

fn config_path_in_dir(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

fn home_config_path(home_dir: &Path) -> PathBuf {
    home_dir.join(".wx-cli").join("config.json")
}

pub fn cli_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("WX_CLI_HOME") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    cli_home_dir().join(".wx-cli")
}

fn cli_home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(std::env::temp_dir)
}

fn default_db_dir() -> PathBuf {
    PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("Tencent/xwechat")
}

fn default_wechat_process() -> &'static str {
    "Weixin.exe"
}

/// 自动检测微信 db_storage 目录
pub fn auto_detect_db_dir() -> Option<PathBuf> {
    detect_db_dir_impl()
}

/// 递归查找 db_storage 目录下所有 .db 文件的最新 mtime
fn latest_db_mtime(dir: &Path) -> Option<std::time::SystemTime> {
    let mut latest = None;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let mtime = if path.is_dir() {
                latest_db_mtime(&path).unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            } else if path.extension().and_then(|s| s.to_str()) == Some("db") {
                entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            } else {
                continue;
            };
            latest = Some(latest.map_or(mtime, |cur| if mtime > cur { mtime } else { cur }));
        }
    }
    latest
}

#[cfg(target_os = "windows")]
fn detect_db_dir_impl() -> Option<PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    let config_dir = PathBuf::from(&appdata).join("Tencent/xwechat/config");
    if !config_dir.exists() {
        return None;
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&config_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "ini").unwrap_or(false) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let Some(data_root) = resolve_windows_data_root(content.trim()) else {
                        continue;
                    };
                    if data_root.is_dir() {
                        let pattern = data_root.join("xwechat_files");
                        if let Ok(entries2) = std::fs::read_dir(&pattern) {
                            for entry2 in entries2.flatten() {
                                let storage = entry2.path().join("db_storage");
                                if storage.is_dir() {
                                    candidates.push(storage);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    candidates.sort_by_key(|p| latest_db_mtime(p).unwrap_or(std::time::SystemTime::UNIX_EPOCH));
    candidates.into_iter().next_back()
}

/// Resolve the data-root path that Weixin writes to its `*.ini` file under
/// `%APPDATA%\Tencent\xwechat\config\`.
///
/// Observed forms in the wild:
///   - A plain absolute path, e.g. `D:\WeChatFiles`.
///   - The literal token `MyDocument:` (sometimes with a trailing slash),
///     which is not a real filesystem path. Empirically this denotes
///     "the current user's Documents folder"; users who relocated
///     Documents to e.g. `D:\Documents` saw auto-detect fail silently
///     because `PathBuf::from("MyDocument:").is_dir()` is false.
///
/// We accept either form. For the `MyDocument:` token we resolve via
/// `SHGetKnownFolderPath(FOLDERID_Documents)`, which respects the standard
/// shell-folder redirect at
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders\Personal`.
#[cfg(target_os = "windows")]
fn resolve_windows_data_root(content: &str) -> Option<PathBuf> {
    let trimmed = content.trim();
    // Strip an optional trailing slash so `MyDocument:\` and `MyDocument:/` also match.
    let stripped = trimmed.strip_suffix(['\\', '/']).unwrap_or(trimmed);
    if stripped.eq_ignore_ascii_case("MyDocument:") {
        return known_documents_dir();
    }
    Some(PathBuf::from(trimmed))
}

#[cfg(target_os = "windows")]
fn known_documents_dir() -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath, KF_FLAG_DEFAULT};

    // SAFETY: standard Win32 known-folder API. SHGetKnownFolderPath either returns
    // a heap-allocated PWSTR that the caller must free with CoTaskMemFree, or an
    // error — in which case the out-pointer is not allocated. We free on every
    // success path. Passing a null token (HANDLE::default()) means "the calling
    // user", which is exactly what we want.
    unsafe {
        let pwstr =
            SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, HANDLE::default()).ok()?;
        if pwstr.0.is_null() {
            return None;
        }
        // Walk the NUL-terminated wide string to compute its length.
        let mut len = 0usize;
        while *pwstr.0.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(pwstr.0, len);
        let os_str = OsString::from_wide(slice);
        CoTaskMemFree(Some(pwstr.0 as *const _));
        let path = PathBuf::from(os_str);
        if path.as_os_str().is_empty() {
            None
        } else {
            Some(path)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn oversized_configuration_is_rejected_before_parsing_without_changing_source() {
        use std::io::Read;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        let prefix = b"invalid-json synthetic-secret";
        fs::write(&path, prefix).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(super::MAX_CONFIG_BYTES + 1)
            .unwrap();
        let before = fs::metadata(&path).unwrap();
        let error = super::load_config_at(&path).unwrap_err();
        assert_eq!(error.to_string(), "配置超过读取限额");
        assert!(!format!("{error:#}").contains("synthetic-secret"));
        let after = fs::metadata(&path).unwrap();
        assert_eq!(after.len(), before.len());
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
        let mut actual = vec![0; prefix.len()];
        fs::File::open(&path)
            .unwrap()
            .read_exact(&mut actual)
            .unwrap();
        assert_eq!(actual, prefix);
    }

    #[test]
    fn configuration_at_size_limit_preserves_paths_defaults_and_unrelated_fields() {
        use std::io::{Read, Write};

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        let bytes = br#"{"db_dir":"account/db_storage","key_store":"keys.dpapi","unrelated":{"enabled":true}}"#;
        for length in [bytes.len() as u64, super::MAX_CONFIG_BYTES] {
            let mut file = fs::File::create(&path).unwrap();
            file.write_all(bytes).unwrap();
            std::io::copy(
                &mut std::io::repeat(b' ').take(length - bytes.len() as u64),
                &mut file,
            )
            .unwrap();
            drop(file);
            let before = fs::metadata(&path).unwrap();
            let config = super::load_config_at(&path).unwrap();
            assert_eq!(config.db_dir, root.path().join("account/db_storage"));
            assert_eq!(config.keys_file, root.path().join("all_keys.json"));
            assert_eq!(config.key_store, Some(root.path().join("keys.dpapi")));
            assert_eq!(config.decrypted_dir, root.path().join("decrypted"));
            assert_eq!(config.wechat_process, super::default_wechat_process());
            let after = fs::metadata(&path).unwrap();
            assert_eq!(after.len(), length);
            assert_eq!(after.modified().unwrap(), before.modified().unwrap());
            let mut actual = vec![0; bytes.len()];
            fs::File::open(&path)
                .unwrap()
                .read_exact(&mut actual)
                .unwrap();
            assert_eq!(actual, bytes);
        }
    }

    #[test]
    fn old_inline_material_is_rejected_even_with_a_current_store() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        for field in ["image_aes_key", "image_xor_key"] {
            for material in [
                serde_json::json!("synthetic-secret"),
                serde_json::Value::Null,
            ] {
                let mut value = serde_json::json!({"key_store":"keys.dpapi"});
                value[field] = material;
                let bytes = serde_json::to_vec(&value).unwrap();
                std::fs::write(&path, &bytes).unwrap();
                let error = super::load_config_at(&path).unwrap_err().to_string();
                assert!(error.contains("Legacy inline image keys"));
                assert!(!error.contains("synthetic-secret"));
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
            }
        }
        assert!(!root.path().join("keys.dpapi").exists());
    }

    #[test]
    fn absent_key_store_is_omitted_in_serialized_legacy_configuration() {
        let config: super::Config = serde_json::from_value(serde_json::json!({
            "db_dir":"db_storage", "keys_file":"all_keys.json", "decrypted_dir":"decrypted"
        }))
        .unwrap();
        assert!(serde_json::to_value(config)
            .unwrap()
            .get("key_store")
            .is_none());
    }
    use super::{
        config_path_in_dir, default_config_path, find_existing_config_path, home_config_path,
    };
    #[cfg(target_os = "windows")]
    use super::{known_documents_dir, resolve_windows_data_root};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let unique = format!(
            "wx-cli-config-test-{}-{}-{}",
            name,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn config_path_prefers_cwd_over_exe_and_home() {
        let cwd = temp_dir("cwd");
        let exe = temp_dir("exe");
        let home = temp_dir("home");
        fs::write(config_path_in_dir(&cwd), "{}").unwrap();
        fs::write(config_path_in_dir(&exe), "{}").unwrap();
        fs::create_dir_all(home.join(".wx-cli")).unwrap();
        fs::write(home_config_path(&home), "{}").unwrap();

        let path = find_existing_config_path(Some(&cwd), Some(&exe), Some(&home)).unwrap();
        assert_eq!(path, config_path_in_dir(&cwd));

        fs::remove_dir_all(cwd).unwrap();
        fs::remove_dir_all(exe).unwrap();
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn default_config_path_matches_init_write_order() {
        let cwd = PathBuf::from("/tmp/cwd");
        let exe = PathBuf::from("/tmp/exe");
        let home = PathBuf::from("/tmp/home");

        let path = default_config_path(Some(&cwd), Some(&exe), Some(&home));
        assert_eq!(path, cwd.join("config.json"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn resolve_windows_data_root_passes_through_absolute_path() {
        let p = resolve_windows_data_root("D:\\WeChatFiles").unwrap();
        assert_eq!(p, PathBuf::from("D:\\WeChatFiles"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn resolve_windows_data_root_recognises_mydocument_keyword() {
        // Should match the keyword exactly (case-insensitive, with or without trailing slash)
        // and resolve to a non-empty Documents path via SHGetKnownFolderPath.
        let docs = known_documents_dir().expect("Documents known folder must resolve");
        for keyword in [
            "MyDocument:",
            "mydocument:",
            "MyDocument:\\",
            "MyDocument:/",
        ] {
            let resolved = resolve_windows_data_root(keyword)
                .unwrap_or_else(|| panic!("keyword {keyword:?} should resolve"));
            assert_eq!(resolved, docs, "keyword {keyword:?}");
        }
    }
}
