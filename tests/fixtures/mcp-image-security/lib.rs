pub mod attachment {
    pub(crate) use crate::local_files;
    pub use crate::native_image;
    pub use native_image_fixture::decoder;
}
#[path = "../../../src/attachment/local_files.rs"]
pub(crate) mod local_files;
pub use native_image_fixture::{decoder, resolver};
#[path = "../../../src/crypto/mod.rs"]
pub mod crypto;
#[path = "../../../src/attachment/native_image.rs"]
pub mod native_image;
pub mod publish_probe;
pub mod mcp_voice;
#[path = "../../../src/daemon/cache.rs"]
pub mod real_cache;
pub mod real_cache_query;
#[allow(dead_code)]
pub mod instrumented_image {
    include!(concat!(env!("OUT_DIR"), "/instrumented_image.rs"));
}
pub mod daemon {
    pub use mcp_readonly_security_harness::meta;
}
pub use mcp_readonly_security_harness::Names;
use std::{path::PathBuf, sync::OnceLock};

// Only the cache container is synthetic; SQLite and query code are production.
pub struct DbCache(
    pub mcp_readonly_security_harness::DbCache,
    pub  std::sync::Mutex<
        Option<(
            std::sync::Arc<tokio::sync::Notify>,
            std::sync::Arc<tokio::sync::Notify>,
        )>,
    >,
    pub Vec<PathBuf>,
);
impl std::ops::Deref for DbCache {
    type Target = mcp_readonly_security_harness::DbCache;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DbCache {
    pub async fn get(&self, key: &str) -> anyhow::Result<Option<PathBuf>> {
        let gate = self.1.lock().unwrap().take();
        if let Some((entered, release)) = gate {
            entered.notify_one();
            release.notified().await;
        }
        self.0.get(key).await
    }
    pub fn output_protection_paths(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut paths: Vec<_> = self
            .paths
            .values()
            .filter_map(|p| p.parent().map(PathBuf::from))
            .collect();
        paths.extend(self.2.iter().cloned());
        Ok(paths)
    }
    pub fn raw_db_keys(&self) -> Vec<String> {
        self.paths.keys().cloned().collect()
    }
    pub fn new(root: PathBuf) -> Self {
        Self(
            mcp_readonly_security_harness::DbCache::new(root),
            std::sync::Mutex::new(None),
            Vec::new(),
        )
    }
}
fn msg_table_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^Msg_[0-9a-f]{32}$").unwrap())
}
fn ensure_complete_message_inventory(db: &DbCache, names: &Names) -> anyhow::Result<()> {
    anyhow::ensure!(
        daemon::meta::discover_unknown_shards_checked(db.db_dir(), &names.msg_db_keys)?.is_empty(),
        "unknown message shards; complete inventory required"
    );
    Ok(())
}
#[path = "../../../src/daemon/query/mcp_image.rs"]
pub mod mcp_image;

pub mod toolkit;
pub mod config {
    pub struct Config {
        pub db_dir: std::path::PathBuf,
    }
    pub fn load_config() -> anyhow::Result<Config> {
        panic!("forbidden automatic config lookup")
    }
}

pub mod ipc_reader {
    use crate::ipc::Response;
    use anyhow::{ensure, Context, Result};
    // Build-time AST extraction keeps the private production function unchanged.
    include!(concat!(env!("OUT_DIR"), "/ipc_reader.rs"));
    pub async fn read(bytes: &[u8], limit: usize) -> Result<Response> {
        read_response(bytes, Some(limit)).await
    }
}

#[path = "../../../src/ipc.rs"]
pub mod ipc;
#[path = "../../../src/mcp/protocol.rs"]
pub mod protocol;
pub mod mcp {
    pub use crate::protocol;
}
#[path = "../../../src/cli/mcp.rs"]
pub mod host;

// Explicit test doubles: no real account discovery, provider or pipe process.
pub mod runtime {
    use std::path::PathBuf;
    pub struct Config {
        pub db_dir: PathBuf,
        pub keys_file: PathBuf,
        pub decrypted_dir: PathBuf,
        pub wechat_process: String,
    }
    pub struct RuntimeContext {
        pub id: String,
        pub config_path: PathBuf,
        pub root: PathBuf,
        pub config: Config,
    }
    impl RuntimeContext {
        pub fn cache_dir(&self) -> PathBuf {
            self.root.join("cache")
        }
        pub fn mtime_file(&self) -> PathBuf {
            self.root.join("cache/_mtimes.json")
        }
        pub fn load() -> anyhow::Result<Self> {
            eprintln!("AUDIT_RUNTIME_REACHED");
            let path = PathBuf::from(std::env::var_os("WX_CLI_CONFIG").unwrap()).canonicalize()?;
            Ok(Self {
                id: "synthetic".into(),
                config_path: path.clone(),
                root: path.parent().unwrap().into(),
                config: Config {
                    db_dir: "synthetic".into(),
                    keys_file: "synthetic".into(),
                    decrypted_dir: "synthetic".into(),
                    wechat_process: "synthetic".into(),
                },
            })
        }
    }
}
pub mod transport {
    pub fn send_with_limits(
        _: &crate::runtime::RuntimeContext,
        request: crate::ipc::Request,
        _: std::time::Duration,
        _: usize,
    ) -> anyhow::Result<crate::ipc::Response> {
        eprintln!("AUDIT_IPC_REACHED");
        if let Some(path) = std::env::var_os("AUDIT_CAPTURE_REQUEST") {
            std::fs::write(path, serde_json::to_vec(&request)?)?;
        }
        if std::env::var_os("AUDIT_SECRET_ERROR").is_some() {
            anyhow::bail!("SYNTHETIC_SECRET_KEY_abcdef012345");
        }
        Ok(crate::ipc::Response::ok(
            serde_json::json!({"exit_code":0,"status":"published","image":{"path":"synthetic.jpg"}}),
        ))
    }
}
#[path = "../../../src/daemon/query/strict_message.rs"]
mod strict_message;
