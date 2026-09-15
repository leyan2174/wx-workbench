//! Real emoticon slice, publication core and managed process runner.
#[path = "../../../src/attachment/local_files.rs"]
pub mod local_files;
pub mod attachment {
    pub use crate::local_files;
}
#[path = "../../../src/config.rs"]
pub mod config;
#[path = "../../../src/crypto/mod.rs"]
pub mod crypto;
#[path = "../../../src/daemon/cache.rs"]
pub mod production_cache;
#[path = "../../../src/runtime.rs"]
pub mod runtime;
pub mod daemon {
    pub use crate::production_cache as cache;
}
pub mod business;
#[path = "../../../src/adapters/wechat/emoticons/mod.rs"]
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
pub mod files;
#[path = "../../../src/key_store/mod.rs"]
pub mod key_store;
#[path = "../../../src/toolkit/private_file.rs"]
pub mod private_file;
#[path = "../../../src/toolkit/setup.rs"]
pub mod setup;
pub mod toolkit {
    pub use crate::{files, private_file, setup};
}
#[path = "../../../src/toolkit/emoticons/download.rs"]
pub mod download;
#[path = "../../support/managed_process.rs"]
pub mod windows_process;

/// Test entry calling the unchanged production materialization path.
pub fn convert_download(
    root: &std::path::Path,
    url: &str,
    executable: &std::path::Path,
    timeout: std::time::Duration,
) -> anyhow::Result<download::Downloaded> {
    let guard = local_files::HostOutputGuard::new(root)?;
    download::download(
        "0123456789abcdef0123456789abcdef",
        &emoticon_adapter::types::EmojiInfo {
            cdn_url: url.into(),
            ..Default::default()
        },
        &guard,
        &download::DownloadOptions {
            ffmpeg: Some(executable.into()),
            timeout,
            ..Default::default()
        },
    )
}
