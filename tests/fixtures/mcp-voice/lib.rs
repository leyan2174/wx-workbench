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
                Ok(self.paths.get(key).cloned())
            }
        }
    }
}
#[path = "../../../src/daemon/query/mcp_voice.rs"]
pub mod mcp_voice;

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
        };
        let q = mcp_voice::VoiceQuery {
            username: "alice".into(),
            limit: 20,
            offset: 0,
            since: None,
            until: None,
        };
        assert!(mcp_voice::q_voice_messages(&db, &q).await.is_err());
        db.paths.insert(key.clone(), plain);
        let rows = mcp_voice::q_voice_messages(&db, &q).await.unwrap();
        assert_eq!(rows[0].voice_data_bytes, Some(17));
        // 生产 DbCache 不规范化 all_keys；适配器必须用原始键调用 get。
        let original = "message\\MEDIA_0.DB".to_owned();
        db.keys = vec![original.clone()];
        let plain = db.paths.remove(&key).unwrap();
        db.paths.insert(original.clone(), plain);
        std::fs::write(media.join("media_cache.db"), b"unrelated database").unwrap();
        let rows = mcp_voice::q_voice_messages(&db, &q).await.unwrap();
        assert_eq!(rows[0].source, "message/media_0.db");
        db.keys.push(key.clone());
        assert!(mcp_voice::q_voice_messages(&db, &q).await.is_err());
        db.keys = vec![original];
        std::fs::write(media.join("media_1.db"), b"unknown synthetic").unwrap();
        assert!(mcp_voice::q_voice_messages(&db, &q).await.is_err());
        assert_eq!(
            std::fs::read(&raw).unwrap(),
            b"synthetic encrypted placeholder"
        );
    }
}
