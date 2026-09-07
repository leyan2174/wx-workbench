// 仅模拟缓存公开接口，不读取凭据、不解密、不访问真实账号。
pub mod daemon {
    pub mod cache {
        use std::{
            collections::HashMap,
            path::{Path, PathBuf},
        };

        pub struct DbCache {
            pub root: PathBuf,
            pub keys: Vec<String>,
            pub paths: HashMap<String, PathBuf>,
        }

        impl DbCache {
            pub fn media_db_keys(&self) -> Vec<String> {
                self.keys.clone()
            }
            pub fn db_dir(&self) -> &Path {
                &self.root
            }
            pub async fn get(&self, key: &str) -> anyhow::Result<Option<PathBuf>> {
                Ok(self.paths.get(key).cloned())
            }
        }
    }
}

#[path = "../../../src/toolkit/asr/database_media.rs"]
pub mod database_media;
#[path = "../../../src/daemon/query/mcp_voice.rs"]
pub mod mcp_voice;

#[cfg(test)]
mod security_tests;
