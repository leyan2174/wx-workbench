use super::output::{emit_warnings, print_response, OutputOpts};
use super::transport;
use crate::ipc::Request;
use crate::runtime::RuntimeContext;
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

fn state_file(runtime: &RuntimeContext) -> std::path::PathBuf {
    runtime.directory.join("last_check.json")
}

/// 加载上次的 per-session 时间戳快照
/// 格式：{ "sessions": { "username": timestamp, ... } }
/// 旧格式（只有 timestamp 字段）直接丢弃，重新全量获取
fn load_state(path: &Path) -> Option<HashMap<String, i64>> {
    let data = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&data).ok()?;
    // 旧格式（只有 timestamp 字段）没有 sessions key → 返回 None 触发首次运行逻辑
    let map: HashMap<String, i64> = v
        .get("sessions")?
        .as_object()?
        .iter()
        .filter_map(|(k, v)| v.as_i64().map(|ts| (k.clone(), ts)))
        .collect();
    // 空 map 也是合法状态（账号无任何会话），返回 Some(empty) 而非 None
    // 这样不会误触发全量历史拉取
    Some(map)
}

fn save_state(path: &Path, new_state: &HashMap<String, i64>) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut staged = tempfile::NamedTempFile::new_in(path.parent().expect("account directory"))?;
    staged.write_all(
        serde_json::to_string(&serde_json::json!({ "sessions": new_state }))?.as_bytes(),
    )?;
    staged.as_file().sync_all()?;
    staged.persist(path)?;
    Ok(())
}

pub fn cmd_new_messages(limit: usize, opts: OutputOpts) -> Result<()> {
    let runtime = RuntimeContext::load()?;
    let _lock = runtime.lock("new-messages.lock")?;
    let path = state_file(&runtime);
    let state = load_state(&path);
    let (with_meta, debug_source) = opts.request_flags();
    let resp = transport::send_for(
        &runtime,
        Request::NewMessages {
            state,
            limit,
            with_meta,
            debug_source,
        },
    )?;

    // 保存 daemon 返回的 new_state
    if let Some(obj) = resp.data.get("new_state").and_then(|v| v.as_object()) {
        let map: HashMap<String, i64> = obj
            .iter()
            .filter_map(|(k, v)| v.as_i64().map(|ts| (k.clone(), ts)))
            .collect();
        save_state(&path, &map)?;
    }

    emit_warnings(&resp.data);
    print_response(&resp.data, &opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_roundtrip_preserves_empty_and_replaces_previous_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("last_check.json");
        assert!(load_state(&path).is_none());
        let previous = HashMap::from([("peer".to_owned(), 123)]);
        save_state(&path, &previous).unwrap();
        assert_eq!(load_state(&path), Some(previous));
        save_state(&path, &HashMap::new()).unwrap();
        assert_eq!(load_state(&path), Some(HashMap::new()));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
