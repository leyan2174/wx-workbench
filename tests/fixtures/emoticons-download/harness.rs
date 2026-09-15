//! Real emoticon slice, publication core and managed process runner.
#[path = "../../../src/attachment/local_files.rs"]
#[allow(dead_code)] // This slice uses publication guards but omits other media scan/proof APIs.
pub mod local_files;
pub mod attachment {
    pub use crate::local_files;
}
#[path = "../../../src/config.rs"]
pub mod config;
#[path = "../../../src/crypto/mod.rs"]
pub mod crypto;
#[path = "../../../src/daemon/cache.rs"]
#[allow(dead_code, unused_imports)] // This slice omits snapshot re-exports and unrelated cache lifecycle entry points.
pub mod production_cache;
#[path = "../../../src/runtime.rs"]
#[allow(dead_code)] // This fixed-account fixture omits bootstrap and other operation lifecycle entry points.
pub mod runtime;
pub mod daemon {
    pub use crate::production_cache as cache;
}
pub mod business;
#[path = "../../../src/adapters/wechat/emoticons/mod.rs"]
#[allow(dead_code)] // Download tests omit the separate catalog listing/export entry points.
pub mod emoticon_adapter;
#[path = "../../../src/adapters/wechat/messages/probe.rs"]
pub mod message_probe;
pub mod adapters {
    pub mod wechat {
        pub mod messages {
            pub use crate::message_probe as probe;
        }
        pub use crate::emoticon_adapter as emoticons;
    }
}
#[path = "../../../src/toolkit/files.rs"]
#[allow(dead_code)] // Download fixture uses publication guards, not directory collection.
pub mod files;
#[path = "../../../src/key_store/mod.rs"]
#[allow(dead_code)] // Store is embedded for fixed-runtime protection, not migration orchestration.
pub mod key_store;
#[path = "../../../src/toolkit/private_file.rs"]
pub mod private_file;
#[path = "../../../src/toolkit/setup.rs"]
#[allow(dead_code)] // Only fixed-path configuration support is needed; setup orchestration is tested at root.
pub mod setup;
pub mod toolkit {
    pub use crate::{files, private_file, setup};
}
#[path = "../../../src/toolkit/emoticons/download.rs"]
pub mod download;
#[path = "../../support/managed_process.rs"]
pub mod windows_process;

/// Test entry through a real synthetic catalog and the checked export path.
pub fn convert_download(
    root: &std::path::Path,
    url: &str,
    executable: &std::path::Path,
    timeout: std::time::Duration,
) -> anyhow::Result<download::Downloaded> {
    let catalog_dir = tempfile::tempdir()?;
    let catalog_path = catalog_dir.path().join("catalog.db");
    let connection = rusqlite::Connection::open(&catalog_path)?;
    connection.execute_batch(include_str!("../emoticons-catalog/schema.sql"))?;
    let md5 = "0123456789abcdef0123456789abcdef";
    connection.execute(
        "INSERT INTO kNonStoreEmoticonTable VALUES(?1,'',?2,'','')",
        rusqlite::params![md5, url],
    )?;
    drop(connection);
    let source = emoticon_adapter::CatalogSource::from_path(&catalog_path)?;
    let reference = source
        .find(md5)?
        .ok_or_else(|| anyhow::anyhow!("synthetic catalog item missing"))?;
    let mut guard = local_files::HostOutputGuard::new(root)?;
    guard.pin_input(&catalog_path)?;
    let (_, downloaded) = download::export_from(
        &source,
        &reference,
        &guard,
        &download::DownloadOptions {
            ffmpeg: Some(executable.into()),
            timeout,
            ..Default::default()
        },
        &[catalog_path],
    )?;
    Ok(downloaded)
}
