//! Pure decoding of remote WeChat emoticon material.
use aes::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
use anyhow::{ensure, Context, Result};

pub(crate) fn decrypt(data: &[u8], key: &str) -> Result<Vec<u8>> {
    // IV = key requires exactly 16 bytes; whitespace may only separate hex bytes.
    let mut bytes = zeroize::Zeroizing::new([0u8; 16]);
    let mut source = key.bytes().peekable();
    let mut count = 0;
    loop {
        while source
            .peek()
            .is_some_and(|b| matches!(b, b' ' | b'\t'..=b'\r'))
        {
            source.next();
        }
        let Some(high) = source.next() else { break };
        let low = source.next().context("invalid emoji AES key")?;
        ensure!(
            count < bytes.len() && high.is_ascii_hexdigit() && low.is_ascii_hexdigit(),
            "invalid emoji AES key"
        );
        bytes[count] =
            ((high as char).to_digit(16).unwrap() * 16 + (low as char).to_digit(16).unwrap()) as u8;
        count += 1;
    }
    ensure!(count == bytes.len(), "invalid emoji AES key");
    let mut output = data.to_vec();
    cbc::Decryptor::<aes::Aes128>::new_from_slices(&bytes[..], &bytes[..])
        .map_err(|_| anyhow::anyhow!("invalid emoji AES key"))?
        .decrypt_padded_mut::<NoPadding>(&mut output)
        .map_err(|_| anyhow::anyhow!("invalid encrypted emoji blocks"))?;
    if let Some(&pad) = output.last() {
        let pad = pad as usize;
        if (1..=16).contains(&pad)
            && output.len() >= pad
            && output[output.len() - pad..]
                .iter()
                .all(|&b| b as usize == pad)
        {
            output.truncate(output.len() - pad);
        }
    }
    Ok(output)
}

const VPS: &[u8] = b"\x00\x00\x00\x01\x40\x01";
const SPS: &[u8] = b"\x00\x00\x00\x01\x42\x01";

fn find(data: &[u8], signature: &[u8]) -> Option<usize> {
    data.windows(signature.len()).position(|w| w == signature)
}

pub(crate) fn detect(data: &[u8]) -> &'static str {
    if data.starts_with(b"\xff\xd8\xff") {
        "jpg"
    } else if data.starts_with(b"\x89PNG") {
        "png"
    } else if data.starts_with(b"GIF") {
        "gif"
    } else if data.starts_with(b"RIFF") {
        "webp"
    } else if data.starts_with(b"WXGF") || find(&data[..data.len().min(256)], VPS).is_some() {
        "hevc"
    } else {
        "bin"
    }
}

pub(crate) fn hevc_stream(data: &[u8]) -> Result<&[u8]> {
    // Prefer VPS even when SPS occurs earlier in the payload.
    let start = find(data, VPS)
        .or_else(|| find(data, SPS))
        .context("emoji HEVC stream missing")?;
    Ok(&data[start..])
}

#[cfg(test)]
#[path = "remote_format_tests.rs"]
mod tests;
