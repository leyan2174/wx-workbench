//! 合成账号与真实 DbCache/SQLite 的音频适配测试；不访问私人账号或网络。
#![allow(dead_code)]
#[path = "support/voice_media_adapters.rs"]
pub mod adapters;
#[path = "support/media_business.rs"]
pub mod business;

#[path = "../src/service/monitor.rs"]
pub mod monitor_contract;
mod service {
    pub mod protocol {
        pub use crate::monitor_contract as monitor;
    }
}

#[path = "../src/daemon/cache.rs"]
#[allow(unused_imports)] // 音频适配测试不调用图片资源快照接口。
pub mod cache;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/crypto/mod.rs"]
mod crypto;
pub use adapters::wechat::media::voice as database_media;
#[path = "../src/daemon/meta.rs"]
mod meta;
#[path = "../src/runtime.rs"]
mod runtime;
use cache::DbCache;
use rusqlite::Connection;
use serde_json::json;
use std::{collections::HashMap, fs, path::PathBuf};
#[path = "fixtures/mcp-audio/tests.rs"]
mod audio_tests;
