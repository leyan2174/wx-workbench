#[path = "../mcp-readonly-runtime/encrypted_sqlite.rs"]
mod encrypted_sqlite;
use crate::{artifacts, support::Account};
use encrypted_sqlite::{encrypt, sqlite};
use std::fs;

pub fn seed(account: &Account) {
    let root = account.root();
    let plain = root.join("voice-build.db");
    let table = format!("Msg_{:x}", md5::compute(b"peer"));
    for shard in 0..2 {
        let conn = sqlite(&plain);
        conn.execute_batch(&format!("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id(rowid,user_name) VALUES(7,'peer'),(8,'other'); CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,server_id INTEGER,real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER)")).unwrap();
        if shard == 0 {
            for id in [700, 701] {
                conn.execute(
                    &format!("INSERT INTO [{table}] VALUES(?1,34,?2,?3,7,'',0)"),
                    rusqlite::params![id - 693, artifacts::TIMESTAMP + id - 700, id + 9000],
                )
                .unwrap();
            }
        }
        drop(conn);
        encrypt(
            &plain,
            &root.join(format!("db_storage/message/message_{shard}.db")),
        );
    }
    let conn = sqlite(&plain);
    conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id(rowid,user_name) VALUES(7,'peer'),(8,'other'); CREATE TABLE VoiceInfo(chat_name_id INTEGER,local_id INTEGER,create_time INTEGER,svr_id INTEGER,voice_data BLOB)").unwrap();
    for id in [700, 701] {
        conn.execute("INSERT INTO VoiceInfo(rowid,chat_name_id,local_id,create_time,svr_id,voice_data) VALUES(?1,7,?2,?3,?4,?5)",rusqlite::params![id-699,id,artifacts::TIMESTAMP+id-700,id+9000,artifacts::silk(account.marker,id)]).unwrap();
    }
    drop(conn);
    encrypt(&plain, &root.join("db_storage/message/media_0.db"));
    assert!(!fs::exists(&plain).unwrap());
}
