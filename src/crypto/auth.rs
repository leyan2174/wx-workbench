use anyhow::{ensure, Result};
use hmac::{Hmac, Mac};
use sha2::Sha512;
use zeroize::Zeroizing;

use super::{PAGE_SZ, SALT_SZ};

pub(super) struct PageAuth {
    key: Zeroizing<[u8; 32]>,
}

impl PageAuth {
    pub(super) fn from_page1(key: &[u8; 32], page: &[u8]) -> Result<Self> {
        ensure!(page.len() == PAGE_SZ, "数据库首页长度无效");
        // SQLCipher 的认证盐来自主库首页；WAL 头中的两个 salt 仅标识 WAL
        // 代次，不能用于派生页面认证密钥，更不能从输出明文猜测源库位置。
        let mut salt = [0u8; SALT_SZ];
        for (out, byte) in salt.iter_mut().zip(page) {
            *out = byte ^ 0x3a;
        }
        let mut mac_key = Zeroizing::new([0u8; 32]);
        pbkdf2::pbkdf2_hmac::<Sha512>(key, &salt, 2, &mut *mac_key);
        let auth = Self { key: mac_key };
        auth.verify(page, 1, true)?;
        Ok(auth)
    }

    pub(super) fn verify(&self, page: &[u8], pgno: u32, salt_header: bool) -> Result<()> {
        ensure!(page.len() == PAGE_SZ && pgno != 0, "认证页面长度或页号无效");
        let mut mac = Hmac::<Sha512>::new_from_slice(&*self.key)?;
        let start = if salt_header { SALT_SZ } else { 0 };
        mac.update(&page[start..PAGE_SZ - 64]);
        mac.update(&pgno.to_le_bytes());
        ensure!(
            mac.verify_slice(&page[PAGE_SZ - 64..]).is_ok(),
            "数据库页面认证失败（页号 {pgno}），未发布结果"
        );
        Ok(())
    }
}
