//! 合成账号与真实 DbCache/SQLite 的音频适配测试；不访问私人账号或网络。
#![allow(dead_code)]
#[path = "support/voice_media_adapters.rs"]
pub mod adapters;
#[path = "support/media_business.rs"]
pub mod business;

#[path = "../src/attachment/local_files.rs"]
pub mod local_files;
mod attachment {
    pub use crate::local_files;
}

#[path = "../src/daemon/cache.rs"]
#[allow(unused_imports)] // 音频适配测试不调用图片资源快照接口。
pub mod cache;
mod daemon {
    pub use crate::cache;
}
#[path = "../src/config.rs"]
mod config;
#[path = "../src/crypto/mod.rs"]
mod crypto;
#[path = "../src/toolkit/asr/database_media.rs"]
pub mod database_media;
#[path = "../src/daemon/meta.rs"]
mod meta;
#[path = "../src/toolkit/asr/prepared_audio.rs"]
pub mod prepared_audio;
#[path = "../src/runtime.rs"]
mod runtime;
mod toolkit {
    pub mod asr {
        pub use crate::database_media;
        pub use crate::prepared_audio;
    }
}
use cache::DbCache;
use rusqlite::Connection;
use serde_json::json;
use std::{collections::HashMap, fs, path::PathBuf};
pub struct Names {
    map: HashMap<String, String>,
}
#[path = "fixtures/mcp-audio/tests.rs"]
mod audio_tests;
#[path = "../src/daemon/query/mcp_audio.rs"]
mod mcp_audio;
#[path = "../src/daemon/query/mcp_voice.rs"]
mod mcp_voice;
