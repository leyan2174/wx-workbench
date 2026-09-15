pub mod attachment {
    pub use native_image_fixture::AttachmentKind;
    pub(crate) use crate::local_files;
    pub use crate::native_image;
    pub use native_image_fixture::decoder;
    pub use native_image_fixture::resolver;
}
#[path = "../../../src/attachment/local_files.rs"]
#[allow(dead_code)] // This slice uses publication guards but omits other media scan/proof APIs.
pub(crate) mod local_files;
pub use native_image_fixture::{decoder, resolver};
#[path = "../../../src/crypto/mod.rs"]
pub mod crypto;
#[path = "../../../src/attachment/native_image.rs"]
#[allow(dead_code)] // The security fixture uses checked material publication, not every export wrapper.
pub mod native_image;
pub mod publish_probe;
#[path = "../../../src/daemon/cache.rs"]
#[allow(dead_code)] // The image query slice omits unrelated cache lifecycle entry points.
pub mod real_cache;
pub mod real_cache_query;
#[allow(dead_code)]
pub mod instrumented_image {
    include!(concat!(env!("OUT_DIR"), "/instrumented_image.rs"));
}
pub mod daemon {
    pub use crate::real_cache as cache;
    pub use mcp_readonly_security_harness::meta;
}
pub use mcp_readonly_security_harness::Names;
#[path = "../../../src/message/xml.rs"]
#[allow(dead_code)] // Only the shared XML scanner is used in this slice.
pub mod message_xml;
pub mod message {
    pub use crate::message_xml as xml;
    pub use mcp_readonly_security_harness::message::split_group_content;
}
use std::path::PathBuf;

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
fn ensure_complete_message_inventory(db: &DbCache, names: &Names) -> anyhow::Result<()> {
    anyhow::ensure!(
        daemon::meta::discover_unknown_shards_checked(db.db_dir(), &names.msg_db_keys)?.is_empty(),
        "unknown message shards; complete inventory required"
    );
    Ok(())
}
#[path = "../../../src/daemon/query/mcp_image.rs"]
#[allow(dead_code)] // The instrumented counterpart owns some audit-only call paths.
pub mod mcp_image;

// Integration tests cross the fixture crate boundary, not the production API.
pub async fn decode_with_material(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
    output_root: &std::path::Path,
    material: decoder::V2KeyMaterial<'_>,
) -> anyhow::Result<serde_json::Value> {
    mcp_image::q_decode_image_with_material(
        db, names, chat, local_id, create_time, output_root, material,
    )
    .await
}

pub mod toolkit;
pub use native_image_fixture::{config, runtime};

#[path = "../../../src/service/transport/framing.rs"]
#[allow(dead_code)] // The audit probes bounded line reads, not all frame transports.
mod ipc_framing;
pub mod ipc_reader {
    pub async fn read(mut bytes: &[u8], limit: usize) -> anyhow::Result<()> {
        // The same bounded frame implementation used by both production clients.
        crate::ipc_framing::line(&mut bytes, limit).await?;
        Ok(())
    }
}

#[path = "../../../src/ipc.rs"]
pub mod ipc;
#[path = "../../../src/mcp/protocol.rs"]
pub mod protocol;
#[path = "../../../src/service/message_filter.rs"]
pub mod message_filter;
pub mod service {
    pub use crate::message_filter;
}
pub mod mcp {
    pub use crate::protocol;
}
pub use wx_mcp_cli_harness::cli_mcp as host;

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
#[allow(dead_code)] // Image queries do not consume every strict message getter.
mod strict_message;
#[path = "../../support/image_media_adapters.rs"]
pub mod adapters;
#[path = "../../support/media_business.rs"]
pub mod business;

#[path = "../../../src/toolkit/files.rs"]
#[allow(dead_code)] // Image fixture uses publication guards, not directory collection.
pub mod files;
#[path = "../../../src/toolkit/setup.rs"]
#[allow(dead_code)] // Only fixed-path configuration support is needed; setup orchestration is tested at root.
pub mod setup;
#[path = "../../../src/toolkit/private_file.rs"]
pub mod private_file;
