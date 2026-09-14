//! 合成 WAL 构造器，仅用于测试；独立计算校验和，不调用生产验证函数。

/// 连续密文页从页号 1 开始，最后一帧提交整个数据库；空切片生成合法空 WAL 头。
pub(crate) fn wal_bytes(pages: &[u8]) -> Vec<u8> {
    const PAGE_SIZE: usize = 4096;
    assert_eq!(pages.len() % PAGE_SIZE, 0);
    fn checksum(bytes: &[u8], mut sum: [u32; 2]) -> [u32; 2] {
        for pair in bytes.chunks_exact(8) {
            let a = u32::from_le_bytes(pair[..4].try_into().unwrap());
            let b = u32::from_le_bytes(pair[4..].try_into().unwrap());
            sum[0] = sum[0].wrapping_add(a).wrapping_add(sum[1]);
            sum[1] = sum[1].wrapping_add(b).wrapping_add(sum[0]);
        }
        sum
    }
    let mut bytes = vec![0u8; 32];
    bytes[..4].copy_from_slice(&0x377f0682u32.to_be_bytes());
    bytes[4..8].copy_from_slice(&3_007_000u32.to_be_bytes());
    bytes[8..12].copy_from_slice(&(PAGE_SIZE as u32).to_be_bytes());
    bytes[16..24].copy_from_slice(&[7; 8]);
    let mut sum = checksum(&bytes[..24], [0; 2]);
    bytes[24..28].copy_from_slice(&sum[0].to_be_bytes());
    bytes[28..32].copy_from_slice(&sum[1].to_be_bytes());
    for (index, page) in pages.chunks_exact(PAGE_SIZE).enumerate() {
        let mut frame = [0u8; 24];
        frame[..4].copy_from_slice(&((index + 1) as u32).to_be_bytes());
        if index + 1 == pages.len() / PAGE_SIZE {
            frame[4..8].copy_from_slice(&((index + 1) as u32).to_be_bytes());
        }
        frame[8..16].copy_from_slice(&[7; 8]);
        sum = checksum(&frame[..8], sum);
        sum = checksum(page, sum);
        frame[16..20].copy_from_slice(&sum[0].to_be_bytes());
        frame[20..24].copy_from_slice(&sum[1].to_be_bytes());
        bytes.extend(frame);
        bytes.extend(page);
    }
    bytes
}
