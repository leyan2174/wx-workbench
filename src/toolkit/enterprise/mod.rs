//! 企业微信离线 wxSQLite3 AES-128-CBC 主库解密；不扫描进程、不读取密钥文件。
//! 仅支持旧脚本实际使用的 4096 字节页，支持头片段模式和整页旧模式。
//! 此格式没有 MAC；SQLite integrity_check 是结构校验，不是密码学认证。
//! 输出必须不存在；同目录临时文件校验成功后以硬链接原子发布，不覆盖任何文件。

pub mod queries;

use aes::Aes128;
use anyhow::{ensure, Context, Result};
use cbc::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
use rusqlite::{Connection, OpenFlags};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use zeroize::Zeroizing;

pub const PAGE_SIZE: usize = 4096;
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseFormat {
    PlainSqlite,
    WxSqlite3Aes128Header,
    WxSqlite3Aes128Legacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecryptReport {
    pub pages: u32,
    pub bytes: u64,
    pub format: DatabaseFormat,
}

pub fn parse_key_hex(value: &str) -> Result<[u8; 16]> {
    let value = value.trim();
    let value = value
        .strip_prefix("x'")
        .and_then(|v| v.strip_suffix('\''))
        .unwrap_or(value);
    ensure!(
        value.len() == 32 && value.is_ascii(),
        "Expected a 16-byte raw key as 32 hex characters"
    );
    let mut key = Zeroizing::new([0u8; 16]);
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16)
            .context("Invalid raw key hex encoding")?;
    }
    Ok(*key)
}

pub fn derive_page_key(raw_key: &[u8; 16], page_no: u32) -> Result<[u8; 16]> {
    ensure!(page_no != 0, "Page numbers start at one");
    let mut material = Zeroizing::new([0u8; 24]);
    material[..16].copy_from_slice(raw_key);
    material[16..20].copy_from_slice(&page_no.to_le_bytes());
    material[20..].copy_from_slice(b"sAlT");
    Ok(md5::compute(&material[..]).0)
}

pub fn generate_initial_vector(page_no: u32) -> Result<[u8; 16]> {
    ensure!(page_no != 0, "Page numbers start at one");
    // 使用宽整数避免 page_no + 1 溢出，保持 Python 整数递推语义。
    let mut z = i64::from(page_no) + 1;
    let mut material = [0u8; 16];
    for word in material.chunks_exact_mut(4) {
        let q = z / 52774;
        z = 40692 * (z - 52774 * q) - 3791 * q;
        if z < 0 {
            z += 2147483399;
        }
        word.copy_from_slice(&(z as u32).to_le_bytes());
    }
    Ok(md5::compute(material).0)
}

fn has_header_fragment(page: &[u8]) -> bool {
    if page.len() < 24 {
        return false;
    }
    let size = u16::from_be_bytes([page[16], page[17]]);
    (size == 1 || (size >= 512 && size.is_power_of_two())) && page[21..24] == [64, 32, 32]
}

pub fn decrypt_page(raw_key: &[u8; 16], page: &[u8], page_no: u32) -> Result<Vec<u8>> {
    ensure!(
        page.len() == PAGE_SIZE,
        "Expected exactly 4096 bytes per page"
    );
    let key = Zeroizing::new(derive_page_key(raw_key, page_no)?);
    let iv = generate_initial_vector(page_no)?;
    let mut result = page.to_vec();
    let fragment = page_no == 1 && has_header_fragment(page);
    let offset = if fragment {
        ensure!(
            page[16..18] == [0x10, 0],
            "Unsupported SQLite page size; expected 4096"
        );
        result.copy_within(8..16, 16);
        16
    } else {
        0
    };
    cbc::Decryptor::<Aes128>::new((&*key).into(), (&iv).into())
        .decrypt_padded_mut::<NoPadding>(&mut result[offset..])
        .map_err(|_| anyhow::anyhow!("Invalid AES page length"))?;
    if fragment {
        ensure!(
            result[16..24] == page[16..24],
            "Page-one key validation failed"
        );
        result[..16].copy_from_slice(SQLITE_HEADER);
    }
    Ok(result)
}

fn validate_header(page: &[u8]) -> Result<()> {
    ensure!(
        page.len() == PAGE_SIZE && &page[..16] == SQLITE_HEADER,
        "Invalid SQLite header or raw key"
    );
    ensure!(
        page[16..18] == [0x10, 0],
        "Unsupported SQLite page size; expected 4096"
    );
    ensure!(
        (1..=2).contains(&page[18]) && (1..=2).contains(&page[19]),
        "Invalid SQLite read/write version"
    );
    ensure!(
        page[20..24] == [0, 64, 32, 32],
        "Unsupported SQLite reserved bytes or payload fractions"
    );
    ensure!(
        matches!(page[100], 2 | 5 | 10 | 13),
        "Invalid SQLite page-one btree type"
    );
    Ok(())
}

fn require_no_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        ensure!(
            !Path::new(&name).try_exists()?,
            "Offline database required: sidecar exists"
        );
    }
    Ok(())
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn temporary(parent: &Path) -> Result<(Temporary, File)> {
    for _ in 0..100 {
        let seq = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".wx-enterprise-{}-{seq}.db", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((Temporary(path), file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("Cannot reserve temporary output")
}

fn validate_sqlite(path: &Path) -> Result<()> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut stmt = db.prepare("PRAGMA integrity_check")?;
    let mut rows = stmt.query([])?;
    let row = rows
        .next()?
        .context("SQLite integrity_check returned no result")?;
    ensure!(
        row.get::<_, String>(0)? == "ok",
        "SQLite integrity_check failed"
    );
    ensure!(rows.next()?.is_none(), "SQLite integrity_check failed");
    Ok(())
}

/// 解密离线主库或复制已明文主库；调用者负责 raw_key 的来源及使用后清零。
/// 拒绝已有输出（包括硬链接/符号链接）、截断页及任何旁路日志；失败不修改源文件。
/// 不创建父目录；目标文件系统必须支持硬链接，不支持时保留源且返回错误。
pub fn decrypt_database(source: &Path, output: &Path, raw_key: &[u8; 16]) -> Result<DecryptReport> {
    ensure!(
        fs::symlink_metadata(source)?.file_type().is_file(),
        "Source must be a regular file, not a symlink"
    );
    match fs::symlink_metadata(output) {
        Ok(_) => anyhow::bail!("Output already exists; refusing to overwrite"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    require_no_sidecars(source)?;
    require_no_sidecars(output)?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent).context("Output parent directory must exist")?;
    let output = parent.join(output.file_name().context("Output requires a filename")?);
    let mut options = OpenOptions::new();
    options.read(true);
    // Windows 打开期间仅共享读取，防止另一个进程改写或替换源主库。
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let mut input = options.open(source).context("Cannot open offline source")?;
    let before = input.metadata()?;
    let size = before.len();
    ensure!(
        size > 0 && size % PAGE_SIZE as u64 == 0,
        "Empty or truncated database; size must be a multiple of 4096"
    );
    let pages = u32::try_from(size / PAGE_SIZE as u64).context("Too many database pages")?;
    let mut page = [0u8; PAGE_SIZE];
    input.read_exact(&mut page)?;
    let format = if page.starts_with(SQLITE_HEADER) {
        DatabaseFormat::PlainSqlite
    } else if has_header_fragment(&page) {
        DatabaseFormat::WxSqlite3Aes128Header
    } else {
        DatabaseFormat::WxSqlite3Aes128Legacy
    };
    let first = if format == DatabaseFormat::PlainSqlite {
        page.to_vec()
    } else {
        decrypt_page(raw_key, &page, 1)?
    };
    validate_header(&first)?;
    // 头部计数器一致时，SQLite 声明的页数有效；拒绝整页截断及尾部追加。
    let declared_pages = u32::from_be_bytes(first[28..32].try_into()?);
    if declared_pages != 0 && first[24..28] == first[92..96] {
        ensure!(
            declared_pages == pages,
            "SQLite header page count does not match file size"
        );
    }
    let (temp, mut writer) = temporary(&parent)?;
    writer.write_all(&first)?;
    for page_no in 2..=pages {
        input.read_exact(&mut page)?;
        if format == DatabaseFormat::PlainSqlite {
            writer.write_all(&page)?;
        } else {
            writer.write_all(&decrypt_page(raw_key, &page, page_no)?)?;
        }
    }
    ensure!(
        input.read(&mut [0u8; 1])? == 0,
        "Source grew during decryption"
    );
    let after = input.metadata()?;
    ensure!(
        after.len() == size && before.modified()? == after.modified()?,
        "Source changed during decryption"
    );
    writer.sync_all()?;
    drop(writer);
    validate_sqlite(&temp.0).context("Decrypted database is not structurally valid")?;
    require_no_sidecars(source)?;
    require_no_sidecars(&output)?;
    // create-hard-link 不覆盖目标；即使检查后目标被创建，也不会损坏任何已有文件。
    fs::hard_link(&temp.0, &output)
        .context("Atomic no-clobber publish failed (hard-link support required)")?;
    Ok(DecryptReport {
        pages,
        bytes: size,
        format,
    })
}

#[cfg(test)]
mod tests;
