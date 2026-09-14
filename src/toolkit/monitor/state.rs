//! 只读导入带 runtime_id 的游标，损坏/错账号文件必须报错，不回落到首次基线。
use super::{FixedRuntimeContext, MAX_SESSIONS, MAX_STATE_BYTES};
use anyhow::{ensure, Context, Result};
use serde::{
    de::{MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateFile {
    pub version: u32,
    pub runtime_id: String,
    #[serde(deserialize_with = "unique_sessions")]
    pub sessions: HashMap<String, i64>,
}
impl StateFile {
    pub fn new(context: &FixedRuntimeContext, sessions: HashMap<String, i64>) -> Self {
        Self {
            version: 1,
            runtime_id: context.runtime_id().into(),
            sessions,
        }
    }
}

fn unique_sessions<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<HashMap<String, i64>, D::Error> {
    struct SessionsVisitor;
    impl<'de> Visitor<'de> for SessionsVisitor {
        type Value = HashMap<String, i64>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("唯一会话 ID 与整数时间戳")
        }
        fn visit_map<M: MapAccess<'de>>(
            self,
            mut map: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut sessions = HashMap::new();
            while let Some((name, timestamp)) = map.next_entry::<String, i64>()? {
                if sessions.len() >= MAX_SESSIONS || sessions.insert(name, timestamp).is_some() {
                    return Err(serde::de::Error::custom("重复会话或会话数量超过上限"));
                }
            }
            Ok(sessions)
        }
    }
    deserializer.deserialize_map(SessionsVisitor)
}

pub(super) fn validate_entry(user: &str, timestamp: i64, latest: i64) -> Result<()> {
    ensure!(
        !user.trim().is_empty() && user.len() <= 512 && !user.chars().any(char::is_control),
        "游标会话 ID 无效"
    );
    ensure!(
        (0..=latest).contains(&timestamp),
        "游标时间戳必须非负且不得远在未来"
    );
    Ok(())
}

pub(super) fn parse_sessions(value: &Value) -> Result<HashMap<String, i64>> {
    let object = value.as_object().context("游标必须为对象")?;
    ensure!(object.len() <= MAX_SESSIONS, "游标会话数量超过上限");
    let latest = chrono::Utc::now().timestamp().saturating_add(86400);
    let mut sessions = HashMap::new();
    for (user, timestamp) in object {
        let timestamp = timestamp.as_i64().context("游标时间戳必须为整数")?;
        validate_entry(user, timestamp, latest)?;
        sessions.insert(user.clone(), timestamp);
    }
    validate_size(&sessions)?;
    Ok(sessions)
}

pub(super) fn validate_size(sessions: &HashMap<String, i64>) -> Result<()> {
    ensure!(
        serde_json::to_vec(sessions)?.len() as u64 <= MAX_STATE_BYTES,
        "游标编码超过 8 MiB 上限"
    );
    Ok(())
}

pub(super) fn load(context: &FixedRuntimeContext, path: &Path) -> Result<StateFile> {
    let absolute = std::path::absolute(path)?;
    checked_parent(&absolute)?;
    let meta = fs::symlink_metadata(&absolute).context("指定状态文件不存在或不可读")?;
    ensure!(
        meta.is_file() && !is_reparse(&meta) && meta.len() <= MAX_STATE_BYTES,
        "状态文件必须为不超过 8 MiB 的普通文件"
    );
    let canonical = fs::canonicalize(&absolute)?;
    // 显式状态文件也不能被误设为当前账号的配置、密钥、数据库或后台缓存。
    for root in [
        &context.runtime.config.db_dir,
        &context.runtime.config.decrypted_dir,
        &context.runtime.cache_dir(),
    ] {
        if let Ok(root) = root.canonicalize() {
            ensure!(
                !canonical.starts_with(root),
                "状态文件不得位于数据库或缓存目录内"
            );
        }
    }
    for protected in [
        &context.runtime.config_path,
        &context.runtime.config.keys_file,
        &context.runtime.pid_path(),
        &context.runtime.log_path(),
    ] {
        if protected.try_exists()? {
            // 只比较文件身份，不读取这些受保护文件的内容；同时挡住硬链接别名。
            ensure!(
                !same_file::is_same_file(&canonical, protected)?,
                "状态文件指向受保护的账号文件"
            );
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let file = options
        .open(&canonical)
        .context("无法以只读方式打开状态文件")?;
    let handle = same_file::Handle::from_file(file.try_clone()?)?;
    ensure!(
        handle == same_file::Handle::from_path(&canonical)?,
        "状态文件读取前被替换"
    );
    let mut bytes = Vec::new();
    file.take(MAX_STATE_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_STATE_BYTES, "状态文件超过 8 MiB");
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    let state: StateFile = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("状态文件 JSON/版本结构无效，不支持未绑定账号的旧格式"))?;
    ensure!(
        state.version == 1 && state.runtime_id == context.runtime_id(),
        "状态文件版本不支持或不属于当前账号"
    );
    let latest = chrono::Utc::now().timestamp().saturating_add(86400);
    for (user, timestamp) in &state.sessions {
        validate_entry(user, *timestamp, latest)?;
    }
    Ok(state)
}

pub(super) fn checked_parent(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let parent = absolute.parent().context("路径缺少父目录")?;
    for ancestor in parent.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)?;
        ensure!(
            metadata.is_dir() && !is_reparse(&metadata),
            "路径不得经过符号链接或重解析点"
        );
    }
    Ok(fs::canonicalize(parent)?)
}

pub(super) fn is_reparse(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}
