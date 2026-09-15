// 仅替身验证 DbCache 公开接口；不读取密钥，不冒充真实解密集成。
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
            pub add_shard_on_get: Option<PathBuf>,
        }
        impl DbCache {
            pub(crate) fn media_db_keys(&self) -> Vec<String> {
                let mut keys = self.keys.clone();
                keys.sort();
                keys
            }
            pub fn db_dir(&self) -> &Path {
                &self.root
            }
            pub async fn get(&self, key: &str) -> anyhow::Result<Option<PathBuf>> {
                if let Some(path) = &self.add_shard_on_get {
                    std::fs::write(path, b"synthetic shard added during query")?;
                }
                Ok(self.paths.get(key).cloned())
            }
        }
    }
}
#[path = "../../../src/daemon/query/mcp_voice.rs"]
pub mod mcp_voice;
#[path = "../../../src/business/voice/mod.rs"]
#[allow(unfulfilled_lint_expectations)] // Public fixture visibility differs from the private production domain.
pub mod voice_business;
pub mod business {
    pub use crate::voice_business as voice;
}

#[cfg(test)]
mod adapter_tests {
    use super::*;
    #[tokio::test]
    async fn complete_inventory_and_undecrypted_shards_are_strict() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("message");
        std::fs::create_dir(&media).unwrap();
        let raw = media.join("media_0.db");
        std::fs::write(&raw, b"synthetic encrypted placeholder").unwrap();
        let plain = dir.path().join("plain.db");
        let conn = rusqlite::Connection::open(&plain).unwrap();
        conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES ('alice');
            CREATE TABLE VoiceInfo(chat_name_id INTEGER, local_id INTEGER, create_time INTEGER, voice_data BLOB);
            INSERT INTO VoiceInfo VALUES(1,1,100,zeroblob(17));").unwrap();
        drop(conn);
        let key = "message/media_0.db".to_owned();
        let mut db = daemon::cache::DbCache {
            root: dir.path().to_owned(),
            keys: vec![key.clone()],
            paths: Default::default(),
            add_shard_on_get: None,
        };
        let q = business::voice::catalog::Query {
            username: "alice".into(),
            limit: 20,
            offset: 0,
            since: None,
            until: None,
        };
        assert!(mcp_voice::q_voice_messages(&db, &q).await.is_err());
        db.paths.insert(key.clone(), plain);
        let rows = mcp_voice::q_voice_messages(&db, &q).await.unwrap();
        assert_eq!(rows.entries[0].byte_len, Some(17));
        // 生产 DbCache 不规范化 all_keys；适配器必须用原始键调用 get。
        let original = "message\\MEDIA_0.DB".to_owned();
        db.keys = vec![original.clone()];
        let plain = db.paths.remove(&key).unwrap();
        db.paths.insert(original.clone(), plain);
        std::fs::write(media.join("media_cache.db"), b"unrelated database").unwrap();
        let rows = mcp_voice::q_voice_messages(&db, &q).await.unwrap();
        assert_eq!(
            voice_catalog::legacy_rows(&rows).unwrap()[0].source,
            "message/media_0.db"
        );
        db.keys.push(key.clone());
        assert!(mcp_voice::q_voice_messages(&db, &q).await.is_err());
        db.keys = vec![original];
        db.add_shard_on_get = Some(media.join("media_1.db"));
        let error = mcp_voice::q_voice_messages(&db, &q).await.unwrap_err();
        assert_eq!(error.to_string(), "media inventory changed during query");
        db.add_shard_on_get = None;
        std::fs::write(media.join("media_1.db"), b"unknown synthetic").unwrap();
        assert!(mcp_voice::q_voice_messages(&db, &q).await.is_err());
        assert_eq!(
            std::fs::read(&raw).unwrap(),
            b"synthetic encrypted placeholder"
        );
    }
}
#[path = "../../../src/adapters/wechat/media/voice_catalog.rs"]
pub mod voice_catalog;
pub mod adapters {
    pub mod wechat {
        pub mod media {
            pub use crate::voice_catalog;
        }
    }
}
