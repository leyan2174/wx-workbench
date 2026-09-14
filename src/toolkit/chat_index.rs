//! 聊天导出文件按 username 归属；兼容旧索引，避免同名联系人互相覆盖。
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::BufReader,
    path::{Path, PathBuf},
};

const INDEX_FILE: &str = "_export_index.json";

#[derive(Default, Deserialize)]
#[serde(default)]
struct Identity {
    username: String,
    chat: String,
    is_group: bool,
    exported_at: String,
    date_first_msg: String,
    date_last_msg: String,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
struct Entry {
    username: String,
    is_group: bool,
    current_chat_name: String,
    current_file: String,
    previous_files: Vec<String>,
    last_exported_at: String,
    date_first_msg: String,
    date_last_msg: String,
}

#[derive(Serialize)]
struct Index {
    version: u32,
    chats: BTreeMap<String, Entry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_binding: Option<RuntimeBinding>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeBinding {
    runtime_id: String,
    legacy_unverified: bool,
}

pub(crate) struct ChatIndex {
    root: PathBuf,
    data: Index,
    legacy_source: bool,
    // 在整个批次持锁，文件选择和发布之间不能插入另一个导出进程。
    _lock: fs::File,
}

fn regular_file(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.file_attributes() & 0x400 == 0)
}

fn read_identity(path: &Path) -> Option<Identity> {
    if !regular_file(path) {
        return None;
    }
    // 未声明的 messages 由 serde 流式跳过，不构建整份消息数组。
    let identity: Identity =
        serde_json::from_reader(BufReader::new(fs::File::open(path).ok()?)).ok()?;
    (!identity.username.is_empty()).then_some(identity)
}

fn safe_filename(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.eq_ignore_ascii_case(INDEX_FILE)
        && !name
            .chars()
            .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
        && !name.ends_with([' ', '.'])
        && name.to_ascii_lowercase().ends_with(".json")
}

fn filename_part(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_control() || "\\/:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let value = cleaned.trim().trim_end_matches('.');
    if value.is_empty() {
        "unknown".into()
    } else {
        value.into()
    }
}

fn entry(filename: String, identity: Identity, previous_files: Vec<String>) -> Entry {
    Entry {
        username: identity.username,
        is_group: identity.is_group,
        current_chat_name: identity.chat,
        current_file: filename,
        previous_files,
        last_exported_at: identity.exported_at,
        date_first_msg: identity.date_first_msg,
        date_last_msg: identity.date_last_msg,
    }
}

impl ChatIndex {
    pub(crate) fn open_for(root: &Path, runtime_id: &str) -> Result<Self> {
        ensure!(!runtime_id.is_empty(), "导出运行上下文不能为空");
        let mut index = Self::load(root, Some(runtime_id))?;
        if index.data.runtime_binding.is_none() {
            let next = Index {
                version: index.data.version,
                chats: index.data.chats.clone(),
                runtime_binding: Some(RuntimeBinding {
                    runtime_id: runtime_id.into(),
                    legacy_unverified: index.legacy_source,
                }),
            };
            // Bind before any chat artifact can be published, even if the batch later fails.
            index.persist(&next)?;
            index.data = next;
        }
        Ok(index)
    }

    pub(crate) fn legacy_unverified(&self) -> bool {
        self.data
            .runtime_binding
            .as_ref()
            .is_some_and(|binding| binding.legacy_unverified)
    }

    #[cfg(test)]
    pub(crate) fn open(root: &Path) -> Result<Self> {
        Self::load(root, None)
    }

    fn load(root: &Path, runtime_id: Option<&str>) -> Result<Self> {
        fs::create_dir_all(root)?;
        let root = root.canonicalize()?;
        let lock_path = root.join(".wx-export.lock");
        ensure!(
            !lock_path.exists() || regular_file(&lock_path),
            "导出锁不能是重解析点或目录"
        );
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lock.try_lock().context("另一个导出批次正在使用此目录")?;
        let mut data = Index {
            version: 1,
            chats: BTreeMap::new(),
            runtime_binding: None,
        };
        let index_path = root.join(INDEX_FILE);
        ensure!(
            !index_path.exists() || regular_file(&index_path),
            "导出索引不能是重解析点或目录"
        );
        let saved: Option<serde_json::Value> = match fs::read(&index_path) {
            Ok(bytes) => {
                Some(serde_json::from_slice(&bytes).context("导出索引损坏，无法核验归属")?)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error).context("无法读取导出索引"),
        };
        if let Some(value) = &saved {
            ensure!(value.is_object(), "导出索引格式无效");
            ensure!(
                value.get("chats").is_some_and(serde_json::Value::is_object),
                "导出索引损坏，无法核验归属"
            );
            if let Some(version) = value.get("version") {
                ensure!(version.as_u64() == Some(1), "不支持的导出索引版本");
            }
            if let Some(binding) = value.get("runtime_binding") {
                let binding: RuntimeBinding =
                    serde_json::from_value(binding.clone()).context("导出索引运行上下文无效")?;
                ensure!(!binding.runtime_id.is_empty(), "导出索引运行上下文无效");
                if let Some(runtime_id) = runtime_id {
                    ensure!(
                        binding.runtime_id == runtime_id,
                        "导出目录属于另一个运行上下文"
                    );
                }
                data.runtime_binding = Some(binding);
            }
        }
        if let Some(chats) = saved
            .as_ref()
            .and_then(|v| v.get("chats"))
            .and_then(|v| v.as_object())
        {
            for (username, value) in chats {
                if username.is_empty() {
                    continue;
                }
                if let Ok(mut item) = serde_json::from_value::<Entry>(value.clone()) {
                    if !safe_filename(&item.current_file) {
                        continue;
                    }
                    item.username = username.clone();
                    item.previous_files.retain(|f| {
                        safe_filename(f) && !f.eq_ignore_ascii_case(&item.current_file)
                    });
                    item.previous_files.sort();
                    item.previous_files.dedup();
                    data.chats.insert(username.clone(), item);
                }
            }
        } else {
            let mut paths = fs::read_dir(&root)?
                .map(|e| e.map(|e| e.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            // 修改时间相同使用文件名打破平局，确保恢复结果稳定。
            paths.sort_by_key(|p| (fs::metadata(p).and_then(|m| m.modified()).ok(), p.clone()));
            for path in paths {
                let Some(name) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .filter(|n| safe_filename(n))
                else {
                    continue;
                };
                let Some(identity) = read_identity(&path) else {
                    continue;
                };
                let previous = data
                    .chats
                    .remove(&identity.username)
                    .map(|mut e| {
                        e.previous_files.push(e.current_file);
                        e.previous_files
                    })
                    .unwrap_or_default();
                data.chats.insert(
                    identity.username.clone(),
                    entry(name.into(), identity, previous),
                );
            }
        }
        Ok(Self {
            root,
            legacy_source: saved.is_some() || !data.chats.is_empty(),
            data,
            _lock: lock,
        })
    }

    pub(crate) fn choose(&self, username: &str, display: &str, is_group: bool) -> Result<PathBuf> {
        ensure!(!username.is_empty(), "聊天 username 不能为空");
        let prefix = if is_group { "group" } else { "single" };
        let label = filename_part(if display.is_empty() {
            username
        } else {
            display
        });
        let desired = format!("{prefix}_{label}.json");
        for counter in 0..1000 {
            let filename = match counter {
                0 => desired.clone(),
                1 => format!("{prefix}_{label}__{}.json", filename_part(username)),
                n => format!("{prefix}_{label}__{}__{n}.json", filename_part(username)),
            };
            ensure!(safe_filename(&filename), "无法生成有效导出文件名");
            let path = self.root.join(filename);
            if !path.try_exists()? || read_identity(&path).is_some_and(|id| id.username == username)
            {
                return Ok(path);
            }
        }
        anyhow::bail!("无法为聊天生成不冲突的导出文件名")
    }

    /// 增量读取只信任当前索引指向且可核验身份的文件，不静默退回其他历史版本。
    pub(crate) fn current(&self, username: &str) -> Result<Option<PathBuf>> {
        let Some(entry) = self.data.chats.get(username) else {
            return Ok(None);
        };
        let path = self.root.join(&entry.current_file);
        ensure!(
            read_identity(&path).is_some_and(|identity| identity.username == username),
            "索引中的旧导出文件缺失、损坏或身份不匹配，不能增量覆盖"
        );
        Ok(Some(path))
    }

    pub(crate) fn record(&mut self, output: &Path, username: &str) -> Result<()> {
        ensure!(
            output.parent() == Some(self.root.as_path()),
            "导出文件不属于当前索引目录"
        );
        let filename = output
            .file_name()
            .and_then(|n| n.to_str())
            .context("导出文件名无效")?;
        ensure!(safe_filename(filename), "导出文件名无效");
        let identity = read_identity(output).context("无法核验已导出文件的身份")?;
        ensure!(identity.username == username, "导出文件 username 不匹配");
        let mut previous = self
            .data
            .chats
            .get(username)
            .map(|e| {
                let mut files = e.previous_files.clone();
                files.push(e.current_file.clone());
                files
            })
            .unwrap_or_default();
        previous.retain(|f| safe_filename(f) && !f.eq_ignore_ascii_case(filename));
        previous.sort();
        previous.dedup();
        let mut next = Index {
            version: self.data.version,
            chats: self.data.chats.clone(),
            runtime_binding: self.data.runtime_binding.clone(),
        };
        next.chats
            .insert(username.into(), entry(filename.into(), identity, previous));
        self.persist(&next)?;
        self.data = next;
        Ok(())
    }

    fn persist(&self, next: &Index) -> Result<()> {
        super::atomic_output(&self.root.join(INDEX_FILE), |temporary| {
            serde_json::to_writer_pretty(
                fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(temporary)?,
                next,
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "wx-index-{}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn write(path: &Path, username: &str) {
        fs::write(path, serde_json::json!({"username":username,"chat":"same","messages":[{"content":"synthetic"}]}).to_string()).unwrap();
    }

    #[test]
    fn avoids_collisions_and_preserves_unknown_files() {
        let t = Temp::new();
        let mut index = ChatIndex::open(&t.0).unwrap();
        let first = index.choose("alpha", "same", false).unwrap();
        write(&first, "alpha");
        index.record(&first, "alpha").unwrap();
        let second = index.choose("beta", "same", false).unwrap();
        assert_eq!(second.file_name().unwrap(), "single_same__beta.json");
        fs::write(&second, b"not JSON").unwrap();
        let third = index.choose("beta", "same", false).unwrap();
        assert_eq!(third.file_name().unwrap(), "single_same__beta__2.json");
        assert_eq!(fs::read(second).unwrap(), b"not JSON");
        assert_eq!(index.choose("alpha", "same", false).unwrap(), first);
    }

    #[test]
    fn context_binding_precedes_artifacts_and_rejects_foreign_reuse() {
        let t = Temp::new();
        let index = ChatIndex::open_for(&t.0, "account-a").unwrap();
        assert!(index.data.chats.is_empty());
        assert!(!index.legacy_unverified());
        let before = fs::read(t.0.join(INDEX_FILE)).unwrap();
        drop(index);
        assert!(ChatIndex::open_for(&t.0, "account-b").is_err());
        assert_eq!(fs::read(t.0.join(INDEX_FILE)).unwrap(), before);
        assert!(ChatIndex::open_for(&t.0, "account-a").is_ok());
    }

    #[test]
    fn legacy_records_are_marked_unverified_and_corrupt_indices_do_not_fallback() {
        let t = Temp::new();
        let chat = t.0.join("single_old.json");
        write(&chat, "alpha");
        let index = ChatIndex::open_for(&t.0, "account-a").unwrap();
        assert!(index.legacy_unverified());
        assert!(index.current("alpha").unwrap().is_some());
        drop(index);
        assert!(ChatIndex::open_for(&t.0, "account-a")
            .unwrap()
            .legacy_unverified());
        fs::write(t.0.join(INDEX_FILE), b"{broken").unwrap();
        assert!(ChatIndex::open_for(&t.0, "account-b").is_err());
        assert_eq!(fs::read(t.0.join(INDEX_FILE)).unwrap(), b"{broken");
        assert!(chat.exists());
    }

    #[test]
    fn restores_legacy_files_and_tracks_renamed_exports_after_publication() {
        let t = Temp::new();
        write(&t.0.join("single_old.json"), "alpha");
        let mut index = ChatIndex::open(&t.0).unwrap();
        assert_eq!(index.data.chats["alpha"].current_file, "single_old.json");
        assert_eq!(
            index.current("alpha").unwrap(),
            Some(t.0.canonicalize().unwrap().join("single_old.json"))
        );
        assert!(index.current("absent").unwrap().is_none());
        let next = index.choose("alpha", "new", false).unwrap();
        assert!(index.record(&next, "alpha").is_err());
        write(&next, "alpha");
        index.record(&next, "alpha").unwrap();
        assert!(t.0.join("single_old.json").exists());
        assert_eq!(
            index.data.chats["alpha"].previous_files,
            ["single_old.json"]
        );
        drop(index);
        let index = ChatIndex::open(&t.0).unwrap();
        assert_eq!(index.data.chats["alpha"].current_file, "single_new.json");
        fs::write(t.0.join("single_new.json"), b"broken").unwrap();
        assert!(index.current("alpha").is_err());
    }

    #[test]
    fn failed_index_publication_does_not_advance_live_state() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = Temp::new();
        let mut index = ChatIndex::open(&t.0).unwrap();
        let old = index.choose("alpha", "old", false).unwrap();
        write(&old, "alpha");
        index.record(&old, "alpha").unwrap();
        let next = index.choose("alpha", "new", false).unwrap();
        write(&next, "alpha");
        let index_path = t.0.join(INDEX_FILE);
        let before = fs::read(&index_path).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&index_path)
            .unwrap();
        assert!(index.record(&next, "alpha").is_err());
        assert_eq!(index.current("alpha").unwrap(), Some(old));
        assert_eq!(fs::read(&index_path).unwrap(), before);
        drop(lock);
        index.record(&next, "alpha").unwrap();
        assert_eq!(index.current("alpha").unwrap(), Some(next));
    }

    #[test]
    fn rejects_concurrent_batches_and_untrusted_index_paths() {
        let t = Temp::new();
        fs::write(t.0.join(INDEX_FILE), r#"{"version":1,"chats":{"bad":{"current_file":"../outside.json"},"good":{"current_file":"single_ok.json","previous_files":["../x.json","single_ok.json","old.json","old.json"]}}}"#).unwrap();
        let index = ChatIndex::open(&t.0).unwrap();
        assert!(ChatIndex::open(&t.0).is_err());
        assert!(!index.data.chats.contains_key("bad"));
        assert_eq!(index.data.chats["good"].previous_files, ["old.json"]);
        for name in [
            "..",
            "../x.json",
            "C:x.json",
            "_EXPORT_INDEX.JSON",
            "x.json.",
            "x.json ",
        ] {
            assert!(!safe_filename(name));
        }
    }
}
