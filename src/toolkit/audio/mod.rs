//! 原生 SILK SDK 解码与 ffmpeg MP3 编码；Python 仅用于离线差分测试。

pub mod batch;
pub mod publish;

use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

pub const SAMPLE_RATE: u32 = 24_000;
const HEADER: &[u8] = b"#!SILK_V3";
const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_PACKETS: usize = 6_000;

#[derive(Debug, Serialize)]
pub struct Conversion {
    pub input: PathBuf,
    pub output: PathBuf,
    pub size: u64,
}

/// 去掉腾讯前缀，校验完整封包并补齐结束标记，不修改源数据。
/// 尾标记按封包边界识别，不能把压缩载荷中的 ffff 误当成文件尾。
pub fn normalize_silk(data: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        data.len() <= MAX_INPUT_BYTES,
        "SILK input exceeds 16 MiB limit"
    );
    let data = data.strip_prefix(&[2]).unwrap_or(data);
    ensure!(data.starts_with(HEADER), "not a SILK_V3 file");
    let mut cursor = HEADER.len();
    let mut packets = 0;
    while cursor < data.len() {
        ensure!(data.len() - cursor >= 2, "truncated SILK packet length");
        let length = i16::from_le_bytes([data[cursor], data[cursor + 1]]);
        cursor += 2;
        if length == -1 {
            ensure!(cursor == data.len(), "data after SILK end marker");
            ensure!(packets != 0, "SILK contains no audio packets");
            return Ok(data.to_vec());
        }
        ensure!(
            (1..=1024).contains(&length),
            "invalid SILK packet length: {length}"
        );
        ensure!(
            data.len() - cursor >= length as usize,
            "truncated SILK payload"
        );
        cursor += length as usize;
        packets += 1;
        ensure!(packets <= MAX_PACKETS, "SILK exceeds 6000 packet limit");
    }
    ensure!(packets != 0, "SILK contains no audio packets");
    let mut result = data.to_vec();
    result.extend_from_slice(&[255, 255]);
    Ok(result)
}

/// 输出固定为 24kHz、单声道、有符号 16 位小端 PCM。
pub fn decode_silk_to_pcm(data: &[u8]) -> Result<Vec<u8>> {
    let normalized = normalize_silk(data)?;
    let mut decoder = silk_codec::DecoderBuilder::default()
        .output_sample_rate(SAMPLE_RATE as i32)
        .build()
        .context("initialize SILK decoder")?;
    let pcm = decoder
        .decode_bytes(&normalized)
        .context("decode SILK payload")?;
    ensure!(
        !pcm.is_empty() && pcm.len() % 2 == 0,
        "SILK decoder returned invalid PCM"
    );
    Ok(pcm)
}

pub fn convert_silk_to_mp3(input: &Path, output: &Path) -> Result<Conversion> {
    convert_silk_to_mp3_with_ffmpeg(input, output, Path::new("ffmpeg"))
}

/// ffmpeg 可传绝对路径，便于主程序配置和测试；命令不经过 shell。
pub fn convert_silk_to_mp3_with_ffmpeg(
    input: &Path,
    output: &Path,
    ffmpeg: &Path,
) -> Result<Conversion> {
    ensure!(input.is_file(), "input is not a file: {}", input.display());
    ensure!(
        fs::metadata(input)?.len() <= MAX_INPUT_BYTES as u64,
        "SILK input exceeds 16 MiB limit"
    );
    let output = std::path::absolute(output)?;
    protect_source(input, &output)?;
    let pcm = decode_silk_to_pcm(&fs::read(input).context("read SILK input")?)?;
    let parent = output.parent().context("output has no parent")?;
    fs::create_dir_all(parent).context("create audio output directory")?;
    let mut pcm_file = tempfile::NamedTempFile::new_in(parent)?;
    pcm_file.write_all(&pcm)?;
    pcm_file.flush()?;
    // 先关闭输出临时文件句柄，让 Windows 上的 ffmpeg 独占打开它。
    let encoded = tempfile::NamedTempFile::new_in(parent)?.into_temp_path();
    let mut command = Command::new(ffmpeg);
    command
        .args([
            "-nostdin",
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "s16le",
            "-ar",
            "24000",
            "-ac",
            "1",
            "-i",
        ])
        .arg(pcm_file.path())
        .args(["-f", "mp3"])
        .arg(&encoded);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW：禁止弹出控制台。
    }
    let result = command.output().context("start ffmpeg")?;
    if !result.status.success() {
        bail!(
            "ffmpeg failed ({}): {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    let size = fs::metadata(&encoded)?.len();
    ensure!(size > 0, "ffmpeg produced an empty MP3");
    fs::OpenOptions::new()
        .write(true)
        .open(&encoded)?
        .sync_all()?;
    protect_source(input, &output)?;
    // 同目录、同卷的原子替换；失败时 RAII 清理临时文件，保留原输出。
    encoded
        .persist(&output)
        .map_err(|error| error.error)
        .context("publish MP3 atomically")?;
    Ok(Conversion {
        input: input.to_path_buf(),
        output,
        size,
    })
}

fn protect_source(input: &Path, output: &Path) -> Result<()> {
    if output.exists() {
        ensure!(
            !same_file::is_same_file(input, output)?,
            "output must not replace the SILK source"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
