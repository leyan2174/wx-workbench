pub mod cache;
pub mod meta;
pub mod query;
pub mod query_state;
pub mod server;
pub(crate) mod tasks;

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

use crate::runtime::RuntimeContext;

fn normalized_rel_key(rel_key: &str) -> String {
    rel_key.replace('\\', "/")
}

fn is_msg_db_key(rel_key: &str) -> bool {
    let rel_key = normalized_rel_key(rel_key);
    rel_key.starts_with("message/message_")
        && rel_key.ends_with(".db")
        && !rel_key.contains("_fts")
        && !rel_key.contains("_resource")
}

fn is_biz_msg_db_key(rel_key: &str) -> bool {
    let rel_key = normalized_rel_key(rel_key);
    rel_key.starts_with("message/biz_message_")
        && rel_key.ends_with(".db")
        && !rel_key.contains("_fts")
        && !rel_key.contains("_resource")
}

fn collect_db_keys(all_keys: &HashMap<String, String>, predicate: fn(&str) -> bool) -> Vec<String> {
    let mut keys: Vec<String> = all_keys.keys().filter(|k| predicate(k)).cloned().collect();
    keys.sort();
    keys
}

/// daemon 入口
///
/// 当 WX_DAEMON_MODE 环境变量设置时，main() 调用此函数
pub fn run() {
    let rt = tokio::runtime::Runtime::new().expect("无法创建 tokio runtime");
    if let Err(e) = rt.block_on(async_run()) {
        eprintln!("[daemon] 启动失败: {}", e);
        std::process::exit(1);
    }
}

async fn async_run() -> Result<()> {
    let runtime = RuntimeContext::load()?;
    if let Ok(expected) = std::env::var("WX_CLI_EXPECTED_RUNTIME") {
        anyhow::ensure!(
            expected == runtime.id,
            "后台启动期间账号配置发生变化，拒绝连接错误账号"
        );
    }
    // 生命周期锁先于缓存初始化，防止同账号多个后台同时修改缓存。
    let _lifetime = runtime.lock("daemon.lock")?;
    tokio::fs::create_dir_all(runtime.cache_dir()).await?;

    let pid = std::process::id();

    eprintln!("[daemon] wx-daemon 启动 (PID {})", pid);

    // 加载配置
    let cfg = &runtime.config;
    eprintln!("[daemon] DB_DIR: {}", cfg.db_dir.display());

    let query = Arc::new(query_state::QueryState::new(runtime.clone()));
    let (tasks, receiver) = tasks::Service::new(runtime.clone(), query.clone())?;
    let worker = tokio::spawn(tasks.clone().run_worker(receiver));
    let handler_state = tasks.clone();
    let handler = Arc::new(move |call| {
        let state = handler_state.clone();
        async move { state.dispatch(call).await }
    });
    let mut task_server = tokio::spawn(crate::service::transport::serve(
        runtime.clone(), handler, tasks.subscribe_shutdown(),
    ));
    let mut query_server = tokio::spawn(async move { server::serve(query, &runtime.pipe_name()).await });
    let mut stop = tasks.subscribe_shutdown();
    let result = tokio::select! {
        result = &mut task_server => result.map_err(anyhow::Error::from).and_then(|result| result),
        result = &mut query_server => result.map_err(anyhow::Error::from).and_then(|result| result),
        _ = stop.changed() => Ok(()),
    };
    tasks.request_shutdown();
    // Workers first reap their process trees and persist terminal states.
    let _ = worker.await;
    query_server.abort();
    if !query_server.is_finished() { let _ = query_server.await; }
    if !task_server.is_finished() { let _ = task_server.await; }
    result
}

/// 从 all_keys.json 提取 rel_key -> enc_key 映射
///
/// 兼容两种格式：
/// - `{ "rel/path.db": { "enc_key": "hex" } }`（Python 版原生格式）
/// - `{ "rel/path.db": "hex" }`（简化格式）
fn extract_keys(json: &serde_json::Value) -> HashMap<String, String> {
    let mut result = HashMap::new();
    if let Some(obj) = json.as_object() {
        for (k, v) in obj {
            if k.starts_with('_') {
                continue;
            }
            let enc_key = if let Some(s) = v.as_str() {
                s.to_string()
            } else if let Some(obj2) = v.as_object() {
                obj2.get("enc_key")
                    .and_then(|e| e.as_str())
                    .unwrap_or_default()
                    .to_string()
            } else {
                continue;
            };
            if !enc_key.is_empty() {
                // 统一路径分隔符
                let rel = k.replace('\\', "/");
                result.insert(rel, enc_key);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{is_biz_msg_db_key, is_msg_db_key};

    #[test]
    fn message_db_key_filter_ignores_biz_and_auxiliary_files() {
        assert!(is_msg_db_key("message/message_0.db"));
        assert!(is_msg_db_key("message\\message_12.db"));
        assert!(!is_msg_db_key("message/biz_message_0.db"));
        assert!(!is_msg_db_key("message/message_0.db-wal"));
        assert!(!is_msg_db_key("message/message_0_fts.db"));
        assert!(!is_msg_db_key("message/message_0_resource.db"));
    }

    #[test]
    fn biz_message_db_key_filter_matches_only_biz_shards() {
        assert!(is_biz_msg_db_key("message/biz_message_0.db"));
        assert!(is_biz_msg_db_key("message\\biz_message_3.db"));
        assert!(!is_biz_msg_db_key("message/message_0.db"));
        assert!(!is_biz_msg_db_key("message/biz_message_0.db-wal"));
        assert!(!is_biz_msg_db_key("message/biz_message_0_fts.db"));
        assert!(!is_biz_msg_db_key("message/biz_message_0_resource.db"));
    }
}
