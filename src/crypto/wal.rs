use anyhow::{ensure, Result};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::{atomic, auth::PageAuth, decrypt_layout, PAGE_SZ};

pub const WAL_HDR_SZ: usize = 32;
pub const WAL_FRAME_HDR: usize = 24;
const FRAME_SIZE: usize = WAL_FRAME_HDR + PAGE_SZ;
const MAX_PAGE: u32 = 1_000_000;

fn be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes[..4].try_into().unwrap())
}

fn checksum(bytes: &[u8], little: bool, mut sum: [u32; 2]) -> [u32; 2] {
    for pair in bytes.chunks_exact(8) {
        let word = |p: &[u8]| {
            if little {
                u32::from_le_bytes(p[..4].try_into().unwrap())
            } else {
                be(p)
            }
        };
        sum[0] = sum[0].wrapping_add(word(pair)).wrapping_add(sum[1]);
        sum[1] = sum[1].wrapping_add(word(&pair[4..])).wrapping_add(sum[0]);
    }
    sum
}

// 只记录连续有效前缀内最后一次提交。不能跳过损坏帧继续寻找“看起来有效”的
// 后续帧：滚动校验和绑定整条链，旧代次尾部与未提交事务都不能进入缓存。
fn committed(data: &[u8]) -> Result<Option<(usize, u32)>> {
    ensure!(data.len() >= WAL_HDR_SZ, "WAL 头不完整");
    let magic = be(data);
    ensure!(matches!(magic, 0x377f0682 | 0x377f0683), "WAL magic 无效");
    ensure!(be(&data[4..]) == 3_007_000, "WAL 版本不支持");
    ensure!(be(&data[8..]) == PAGE_SZ as u32, "WAL 页面大小不支持");
    let little = magic == 0x377f0682;
    let mut sum = checksum(&data[..24], little, [0; 2]);
    ensure!(sum == [be(&data[24..]), be(&data[28..])], "WAL 头校验失败");
    let mut last = None;
    for (index, frame) in data[WAL_HDR_SZ..].chunks_exact(FRAME_SIZE).enumerate() {
        let pgno = be(frame);
        if pgno == 0 || frame[8..16] != data[16..24] {
            break;
        }
        let next = checksum(&frame[..8], little, sum);
        let next = checksum(&frame[WAL_FRAME_HDR..], little, next);
        if next != [be(&frame[16..]), be(&frame[20..])] {
            break;
        }
        // 保留既有资源上限，但不再静默跳过合法却超限的页面。
        ensure!(pgno <= MAX_PAGE, "WAL 页号超过资源上限");
        sum = next;
        let pages = be(&frame[4..]);
        ensure!(pages <= MAX_PAGE, "WAL 提交大小超过资源上限");
        if pages != 0 {
            last = Some((index + 1, pages));
        }
    }
    Ok(last)
}

/// 认证后将最后完整提交应用到副本；任何认证/写入失败均保留原输出。
/// source_db 必须由固定账号上下文显式传入，绝不从缓存路径推导原库。
pub fn apply_wal(
    wal_path: &Path,
    out_path: &Path,
    enc_key: &[u8; 32],
    source_db: &Path,
) -> Result<()> {
    let mut wal = match atomic::open_source(wal_path) {
        Ok(file) => file,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(())
        }
        Err(error) => return Err(error),
    };
    let before = wal.metadata()?;
    let mut data = Vec::new();
    wal.read_to_end(&mut data)?;
    if data.is_empty() {
        return Ok(());
    }
    let Some((frames, pages)) = committed(&data)? else {
        return Ok(());
    };
    let mut source = atomic::open_source(source_db)?;
    let source_before = source.metadata()?;
    let mut page1 = [0u8; PAGE_SZ];
    source.read_exact(&mut page1)?;
    let auth = PageAuth::from_page1(enc_key, &page1)?;
    let selected = &data[WAL_HDR_SZ..WAL_HDR_SZ + frames * FRAME_SIZE];
    // 认证实际页号，包括页 1。沿用原实现的 WAL 全密文布局，不因旧密钥导致
    // 的失败猜测另一种格式。即使某帧稍后被覆盖或截断，也不接受认证失败。
    for frame in selected.chunks_exact(FRAME_SIZE) {
        auth.verify(&frame[WAL_FRAME_HDR..], be(frame), false)?;
    }
    let mut output = atomic::Output::new(out_path, &[&source, &wal])?;
    output.copy_old()?;
    for frame in selected.chunks_exact(FRAME_SIZE) {
        let pgno = be(frame);
        let plain = decrypt_layout(enc_key, &frame[WAL_FRAME_HDR..], false)?;
        output
            .file()
            .seek(SeekFrom::Start((pgno as u64 - 1) * PAGE_SZ as u64))?;
        output.file().write_all(&plain)?;
        let commit_pages = be(&frame[4..]);
        if commit_pages != 0 {
            // 中间提交的收缩也必须作用于副本，避免之后扩容重新暴露已删除页面。
            output
                .file()
                .set_len(commit_pages as u64 * PAGE_SZ as u64)?;
        }
    }
    output.file().set_len(pages as u64 * PAGE_SZ as u64)?;
    let after = wal.metadata()?;
    let source_after = source.metadata()?;
    ensure!(
        before.len() == after.len()
            && before.modified()? == after.modified()?
            && source_before.len() == source_after.len()
            && source_before.modified()? == source_after.modified()?,
        "应用 WAL 期间输入发生变化，未发布结果"
    );
    output.publish()
}
