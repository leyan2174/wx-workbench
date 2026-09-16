use anyhow::{ensure, Result};

const MAX_WAV_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo {
    pub pcm_bytes: u64,
    pub sample_rate: u32,
}

pub fn validate_wav(bytes: &[u8]) -> Result<WavInfo> {
    ensure!(
        bytes.len() <= MAX_WAV_BYTES,
        "input exceeds {MAX_WAV_BYTES} byte limit"
    );
    ensure!(
        bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
        "invalid WAV header"
    );
    let u32le = |data: &[u8]| u32::from_le_bytes(data.try_into().unwrap());
    ensure!(
        u32le(&bytes[4..8]) as usize + 8 == bytes.len(),
        "WAV RIFF size mismatch"
    );
    let mut cursor = 12;
    let mut format_seen = false;
    let mut data_seen = false;
    let mut info = WavInfo {
        pcm_bytes: 0,
        sample_rate: 0,
    };
    while cursor < bytes.len() {
        ensure!(bytes.len() - cursor >= 8, "truncated WAV chunk");
        let tag = &bytes[cursor..cursor + 4];
        let size = u32le(&bytes[cursor + 4..cursor + 8]) as usize;
        cursor += 8;
        ensure!(size <= bytes.len() - cursor, "truncated WAV payload");
        let chunk = &bytes[cursor..cursor + size];
        if tag == b"fmt " {
            ensure!(
                !format_seen && size >= 16,
                "invalid or duplicate WAV format"
            );
            ensure!(
                chunk[..4] == [1, 0, 1, 0] && chunk[12..16] == [2, 0, 16, 0],
                "WAV requires PCM16 mono"
            );
            let rate = u32le(&chunk[4..8]);
            ensure!(
                (8_000..=192_000).contains(&rate) && u32le(&chunk[8..12]) == rate * 2,
                "invalid WAV sample/byte rate"
            );
            format_seen = true;
            info.sample_rate = rate;
        } else if tag == b"data" {
            ensure!(
                format_seen && !data_seen && size > 0 && size.is_multiple_of(2),
                "invalid WAV data"
            );
            data_seen = true;
            info.pcm_bytes = size as u64;
        }
        cursor += size + size % 2;
        ensure!(cursor <= bytes.len(), "missing WAV padding");
    }
    ensure!(format_seen && data_seen, "WAV requires fmt and data chunks");
    Ok(info)
}
