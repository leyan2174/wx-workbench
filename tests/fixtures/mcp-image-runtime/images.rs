use crate::support::encrypted_sqlite::{encrypt, sqlite};
use crate::support::Account;
use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
use serde_json::{json, Value};
use std::fs;

pub fn bitmap(marker: &str, id: i64) -> Vec<u8> {
    // 标准 1x1、24 位无压缩 BMP；末行按四字节对齐，A/B 像素不同。
    let mut b = vec![0u8; 58];
    b[..2].copy_from_slice(b"BM");
    b[2..6].copy_from_slice(&58u32.to_le_bytes());
    b[10..14].copy_from_slice(&54u32.to_le_bytes());
    b[14..18].copy_from_slice(&40u32.to_le_bytes());
    b[18..22].copy_from_slice(&1u32.to_le_bytes());
    b[22..26].copy_from_slice(&1u32.to_le_bytes());
    b[26..28].copy_from_slice(&1u16.to_le_bytes());
    b[28..30].copy_from_slice(&24u16.to_le_bytes());
    b[34..38].copy_from_slice(&4u32.to_le_bytes());
    b[54..57].copy_from_slice(if marker == "A" {
        &[0, 0, 255]
    } else {
        &[255, 0, 0]
    });
    if id == 701 {
        b[55] = 127;
    }
    b
}

fn v1(plain: &[u8]) -> Vec<u8> {
    let mut head = [13u8; 16];
    head[..3].copy_from_slice(&plain[..3]);
    let mut block = GenericArray::clone_from_slice(&head);
    aes::Aes128::new(b"cfcd208495d565ef".into()).encrypt_block(&mut block);
    let mut bytes = b"\x07\x08V1\x08\x07".to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&block);
    bytes.extend_from_slice(&plain[3..plain.len() - 4]);
    bytes.extend(plain[plain.len() - 4..].iter().map(|b| b ^ 0x88));
    bytes
}

pub fn seed(account: &Account) {
    let root = account.root();
    let plain = root.join("image-build.db");
    let conn = sqlite(&plain);
    let table = format!("Msg_{:x}", md5::compute(b"peer"));
    conn.execute_batch(&format!("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES('peer'); CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER); INSERT INTO [{table}] VALUES(700,3,70000,1,'',0),(701,3,70100,1,'',0),(702,3,70200,1,'',0),(702,1,70200,1,'',0)")).unwrap();
    drop(conn);
    encrypt(&plain, &root.join("db_storage/message/message_2.db"));
    let conn = sqlite(&plain);
    conn.execute_batch("CREATE TABLE ChatName2Id(user_name TEXT); INSERT INTO ChatName2Id(rowid,user_name) VALUES(7,'peer'); CREATE TABLE MessageResourceInfo(chat_id INTEGER,message_local_id INTEGER,message_local_type INTEGER,message_create_time INTEGER,packed_info BLOB)").unwrap();
    for (id, hash) in [
        (700, "11111111111111111111111111111111"),
        (701, "22222222222222222222222222222222"),
        (702, "33333333333333333333333333333333"),
    ] {
        let image = bitmap(account.marker, id);
        let mut packed = vec![0x12, 0x22, 0x0a, 0x20];
        packed.extend_from_slice(hash.as_bytes());
        conn.execute(
            "INSERT INTO MessageResourceInfo VALUES(7,?1,3,?2,?3)",
            rusqlite::params![id, id * 100, packed],
        )
        .unwrap();
        let dat = if id == 701 {
            v1(&image)
        } else {
            image.iter().map(|b| b ^ 0x5a).collect()
        };
        let path = root.join(format!(
            "msg/attach/{:x}/2026-09/Img/{hash}.dat",
            md5::compute(b"peer")
        ));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, dat).unwrap();
    }
    drop(conn);
    encrypt(&plain, &root.join("db_storage/message/message_resource.db"));
    let keys_path = root.join("keys.json");
    let mut keys: Value = serde_json::from_slice(&fs::read(&keys_path).unwrap()).unwrap();
    for key in ["message/message_2.db", "message/message_resource.db"] {
        keys[key] = json!("11".repeat(32));
    }
    fs::write(keys_path, serde_json::to_vec(&keys).unwrap()).unwrap();
    account.seed_keys(&keys);
}
