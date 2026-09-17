//! 复用 voice_runtime 的两段 SQLCipher 合成算法；不复制其账号、查询或测试逻辑。
//! 为运行时测试构造隔离的合成加密 SQLite 数据库。
use aes::cipher::{block_padding::NoPadding, BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use rusqlite::Connection;
use sha2::Sha512;
use std::{fs, path::Path};

pub fn sqlite(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA page_size=4096").unwrap();
    let mut reserve = 80i32;
    // 使用 SQLite 官方文件控制接口预留 SQLCipher IV/HMAC 页尾。
    let rc = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            conn.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_RESERVE_BYTES,
            (&mut reserve as *mut i32).cast(),
        )
    };
    assert_eq!(rc, rusqlite::ffi::SQLITE_OK);
    conn
}

pub fn encrypt(plain: &Path, output: &Path) {
    let bytes = fs::read(plain).unwrap();
    assert_eq!(bytes[20], 80);
    assert_eq!(bytes.len() % 4096, 0);
    let key = [0x11u8; 32];
    let salt = [0x42u8; 16];
    let mac_salt: Vec<_> = salt.iter().map(|b| b ^ 0x3a).collect();
    let mut mac_key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha512>(&key, &mac_salt, 2, &mut mac_key);
    let mut encrypted = Vec::with_capacity(bytes.len());
    for (index, plain) in bytes.chunks_exact(4096).enumerate() {
        let start = if index == 0 { 16 } else { 0 };
        let mut page = [0u8; 4096];
        if index == 0 {
            page[..16].copy_from_slice(&salt);
        }
        let iv = [0x55u8; 16];
        let cipher = cbc::Encryptor::<aes::Aes256>::new((&key).into(), (&iv).into())
            .encrypt_padded_vec_mut::<NoPadding>(&plain[start..4016]);
        page[start..4016].copy_from_slice(&cipher);
        page[4016..4032].copy_from_slice(&iv);
        let mut mac = Hmac::<Sha512>::new_from_slice(&mac_key).unwrap();
        mac.update(&page[start..4032]);
        mac.update(&((index + 1) as u32).to_le_bytes());
        page[4032..].copy_from_slice(&mac.finalize().into_bytes());
        encrypted.extend_from_slice(&page);
    }
    fs::write(output, encrypted).unwrap();
    // 生产配置只指向加密 db_storage；不留下可被意外回退的明文数据库。
    fs::remove_file(plain).unwrap();
}
