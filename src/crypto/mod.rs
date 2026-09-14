mod atomic;
mod auth;
pub mod wal;

#[cfg(test)]
mod safety_tests;
#[cfg(test)]
pub(crate) mod test_support;

use aes::Aes256;
use anyhow::{bail, ensure, Result};
use cbc::cipher::{BlockDecryptMut, KeyIvInit};
use cbc::Decryptor;
use std::io::{Read, Write};
use std::path::Path;

type Block = aes::cipher::Block<Aes256>;

pub const PAGE_SZ: usize = 4096;
pub const SALT_SZ: usize = 16;
pub const RESERVE_SZ: usize = 80; // IV(16) + HMAC(64)

/// SQLite 文件头魔数（16字节）
pub const SQLITE_HDR: &[u8] = b"SQLite format 3\x00";

pub fn verify_page1(enc_key: &[u8; 32], page: &[u8]) -> bool {
    if page.len() < PAGE_SZ {
        return false;
    }
    auth::PageAuth::from_page1(enc_key, &page[..PAGE_SZ]).is_ok()
}

type Aes256CbcDec = Decryptor<Aes256>;

/// 解密单个 SQLCipher 4 页
///
/// - `enc_key`: 32字节 AES 密钥
/// - `page_data`: 原始加密页面数据（PAGE_SZ 字节）
/// - `pgno`: 页码（从1开始）
///
/// 返回解密后的完整页面（PAGE_SZ 字节）
pub fn decrypt_page(enc_key: &[u8; 32], page_data: &[u8], pgno: u32) -> Result<Vec<u8>> {
    decrypt_layout(enc_key, page_data, pgno == 1)
}

// 布局与认证页号分开：WAL 首页沿用完整密文布局，但 HMAC 必须绑定真实页号 1。
fn decrypt_layout(enc_key: &[u8; 32], page_data: &[u8], salt_header: bool) -> Result<Vec<u8>> {
    if page_data.len() < PAGE_SZ {
        bail!("页面数据不足 {} 字节", PAGE_SZ);
    }

    // IV 位于页面末尾 RESERVE_SZ 区域的前16字节
    let iv_offset = PAGE_SZ - RESERVE_SZ;
    let iv: &[u8; 16] = page_data[iv_offset..iv_offset + 16]
        .try_into()
        .expect("IV 长度固定为 16");

    let mut result = vec![0u8; PAGE_SZ];

    if salt_header {
        // 第一页：跳过 salt(16字节)，解密 [SALT_SZ..PAGE_SZ-RESERVE_SZ]
        let enc = &page_data[SALT_SZ..PAGE_SZ - RESERVE_SZ];
        let dec = aes_cbc_decrypt(enc_key, iv, enc)?;
        // 写入 SQLite 文件头
        result[..16].copy_from_slice(SQLITE_HDR);
        // 写入解密数据（从第16字节开始）
        result[16..PAGE_SZ - RESERVE_SZ].copy_from_slice(&dec);
        // 末尾 RESERVE_SZ 字节补零
        // （已经是零，无需显式操作）
    } else {
        // 其他页：解密 [0..PAGE_SZ-RESERVE_SZ]
        let enc = &page_data[..PAGE_SZ - RESERVE_SZ];
        let dec = aes_cbc_decrypt(enc_key, iv, enc)?;
        result[..PAGE_SZ - RESERVE_SZ].copy_from_slice(&dec);
        // 末尾 RESERVE_SZ 字节补零
    }

    Ok(result)
}

/// AES-256-CBC 解密（不去除 padding，SQLCipher 不使用 PKCS#7 padding）
fn aes_cbc_decrypt(key: &[u8; 32], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>> {
    if data.is_empty() || !data.len().is_multiple_of(16) {
        bail!("密文长度不是 AES 块大小的倍数: {}", data.len());
    }
    // 将 &[u8] 复制为 Block 数组，避免 unsafe from_raw_parts_mut
    let mut blocks: Vec<Block> = data.chunks_exact(16).map(Block::clone_from_slice).collect();
    Aes256CbcDec::new(key.into(), iv.into()).decrypt_blocks_mut(&mut blocks);
    Ok(blocks.iter().flat_map(|b| b.iter().copied()).collect())
}

/// 完整解密一个 SQLCipher 数据库文件（流式，逐页读写避免全量载入内存）
///
/// 读取 `db_path`，按 PAGE_SZ 分页解密，写入 `out_path`
pub fn full_decrypt(db_path: &Path, out_path: &Path, enc_key: &[u8; 32]) -> Result<()> {
    let mut input = atomic::open_source(db_path)?;
    let before = input.metadata()?;
    let file_size = before.len();
    ensure!(
        file_size > 0 && file_size % PAGE_SZ as u64 == 0,
        "加密数据库为空或存在不完整页面"
    );
    let total_pages = file_size / PAGE_SZ as u64;
    ensure!(total_pages < u32::MAX as u64, "数据库页数超限");
    let mut page_buf = vec![0u8; PAGE_SZ];
    read_page(&mut input, &mut page_buf, PAGE_SZ)?;
    let auth = auth::PageAuth::from_page1(enc_key, &page_buf)?;
    let mut output = atomic::Output::new(out_path, &[&input])?;
    // 必须使用同一输入句柄认证每个实际读到的页面。仅在调用前验证首页，
    // 无法发现后续页面损坏，也不能保证调用期间账号独立换钥后不发布垃圾。
    for pgno in 1..=total_pages {
        if pgno != 1 {
            read_page(&mut input, &mut page_buf, PAGE_SZ)?;
            auth.verify(&page_buf, pgno as u32, false)?;
        }
        let dec = decrypt_page(enc_key, &page_buf, pgno as u32)?;
        output.file().write_all(&dec)?;
    }
    let after = input.metadata()?;
    ensure!(
        before.len() == after.len() && before.modified()? == after.modified()?,
        "解密期间源数据库发生变化，未发布结果"
    );
    output.publish()
}

/// 将多个已认证步骤组合成一次发布；回调只能写传入的暂存路径。
/// 源路径由固定账号上下文提供，拒绝覆盖源及其硬链接，不推断其他账号文件。
pub fn with_staged_output<T>(
    out_path: &Path,
    sources: &[&Path],
    write: impl FnOnce(&Path) -> Result<T>,
) -> Result<T> {
    let inputs = sources
        .iter()
        .map(|path| atomic::open_source(path))
        .collect::<Result<Vec<_>>>()?;
    let before = inputs
        .iter()
        .map(std::fs::File::metadata)
        .collect::<std::io::Result<Vec<_>>>()?;
    let refs = inputs.iter().collect::<Vec<_>>();
    let output = atomic::Output::new(out_path, &refs)?;
    output.transform(|temporary| {
        let value = write(temporary)?;
        // 单个步骤成功不等于组合成功：DB 解密后如果 WAL 认证失败，或组合期间
        // 源文件变化，正式缓存与其索引都不得提前更新。
        for (file, before) in inputs.iter().zip(&before) {
            let after = file.metadata()?;
            ensure!(
                before.len() == after.len() && before.modified()? == after.modified()?,
                "组合解密期间源文件发生变化，未发布缓存"
            );
        }
        Ok(value)
    })
}

fn read_page(
    input: &mut impl Read,
    page_buf: &mut [u8],
    bytes_remaining: usize,
) -> std::io::Result<usize> {
    let expected = bytes_remaining.min(PAGE_SZ);
    input.read_exact(&mut page_buf[..expected])?;
    if expected < PAGE_SZ {
        page_buf[expected..].fill(0);
    }
    Ok(expected)
}

#[cfg(test)]
mod tests {
    use super::{read_page, PAGE_SZ};
    use std::io::{self, Read};

    struct ChunkedReader {
        chunks: Vec<Vec<u8>>,
        chunk_idx: usize,
        offset: usize,
    }

    impl ChunkedReader {
        fn new(chunks: Vec<Vec<u8>>) -> Self {
            Self {
                chunks,
                chunk_idx: 0,
                offset: 0,
            }
        }
    }

    impl Read for ChunkedReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.chunk_idx >= self.chunks.len() {
                return Ok(0);
            }
            let chunk = &self.chunks[self.chunk_idx];
            let remaining = &chunk[self.offset..];
            let n = remaining.len().min(buf.len());
            buf[..n].copy_from_slice(&remaining[..n]);
            self.offset += n;
            if self.offset == chunk.len() {
                self.chunk_idx += 1;
                self.offset = 0;
            }
            Ok(n)
        }
    }

    #[test]
    fn read_page_reads_across_short_chunks() {
        let mut reader = ChunkedReader::new(vec![vec![1; 32], vec![2; PAGE_SZ - 32]]);
        let mut page_buf = vec![0u8; PAGE_SZ];

        let n = read_page(&mut reader, &mut page_buf, PAGE_SZ).unwrap();

        assert_eq!(n, PAGE_SZ);
        assert_eq!(page_buf[0], 1);
        assert_eq!(page_buf[31], 1);
        assert_eq!(page_buf[32], 2);
        assert_eq!(page_buf[PAGE_SZ - 1], 2);
    }

    #[test]
    fn read_page_zero_pads_last_partial_page() {
        let mut reader = ChunkedReader::new(vec![vec![7; 8], vec![9; 4]]);
        let mut page_buf = vec![0u8; PAGE_SZ];

        let n = read_page(&mut reader, &mut page_buf, 12).unwrap();

        assert_eq!(n, 12);
        assert_eq!(&page_buf[..8], &[7; 8]);
        assert_eq!(&page_buf[8..12], &[9; 4]);
        assert!(page_buf[12..].iter().all(|&b| b == 0));
    }

    #[test]
    fn read_page_errors_on_early_eof() {
        let mut reader = ChunkedReader::new(vec![vec![1; 8]]);
        let mut page_buf = vec![0u8; PAGE_SZ];

        let err = read_page(&mut reader, &mut page_buf, 16).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
