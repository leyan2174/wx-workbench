//! 仅处理账号显式提供的 URL；本模块不发现账号或密钥。
use crate::adapters::wechat::emoticons::types::EmojiInfo;
use crate::attachment::local_files::HostOutputGuard;
use aes::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
use anyhow::{ensure, Context, Result};
use std::{
    fs,
    io::Read,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct DownloadOptions {
    pub timeout: Duration,
    pub max_bytes: u64,
    pub max_redirects: usize,
    /// None 禁用转换；可执行文件来自受信任的宿主配置。
    pub ffmpeg: Option<PathBuf>,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            max_bytes: 32 * 1024 * 1024,
            max_redirects: 5,
            ffmpeg: Some("ffmpeg".into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloaded {
    pub filename: String,
    pub size: u64,
    pub cached: bool,
    pub conversion_fallback: bool,
    pub converted: bool,
}

pub(crate) fn export_from(
    source: &crate::adapters::wechat::emoticons::CatalogSource,
    reference: &crate::business::emoticons::CatalogMediaRef,
    guard: &HostOutputGuard,
    opts: &DownloadOptions,
    protected: &[PathBuf],
) -> Result<(crate::business::emoticons::Exported, Downloaded), crate::business::media::Error> {
    use crate::business::emoticons::{Error, Exported, Failure, Materialization, Stage};
    let row = source.material(reference)?;
    let downloaded = download_with_protection(&row.md5, &row.info, guard, opts, protected)
        .map_err(|error| {
            error
                .downcast_ref::<Error>()
                .copied()
                .unwrap_or(Error::new(Stage::Download, Failure::Unavailable))
        })?;
    let materialization = if downloaded.cached {
        Materialization::LegacyCache
    } else if downloaded.conversion_fallback {
        Materialization::ConversionFallback
    } else if downloaded.converted {
        Materialization::Converted
    } else {
        Materialization::Downloaded
    };
    Ok((
        Exported {
            bytes: downloaded.size,
            materialization,
        },
        downloaded,
    ))
}

#[cfg(test)]
pub(crate) fn download(
    md5: &str,
    info: &EmojiInfo,
    guard: &HostOutputGuard,
    opts: &DownloadOptions,
) -> Result<Downloaded> {
    download_with_protection(md5, info, guard, opts, &[])
}

fn download_with_protection(
    md5: &str,
    info: &EmojiInfo,
    guard: &HostOutputGuard,
    opts: &DownloadOptions,
    protected: &[PathBuf],
) -> Result<Downloaded> {
    ensure!(
        md5.len() == 32 && md5.bytes().all(|b| b.is_ascii_hexdigit()),
        "emoji MD5 must be 32 hexadecimal characters"
    );
    guard.verify()?;
    for ext in ["gif", "png", "jpg", "webp"] {
        let filename = format!("{md5}.{ext}");
        let path = guard.output_root().join(&filename);
        guard.verify_replaceable_file(&path)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                // 共享守卫检查别名期间，持有禁止写入和删除的 Windows 只读句柄。
                let mut options = fs::OpenOptions::new();
                options.read(true);
                #[cfg(windows)]
                {
                    use std::os::windows::fs::OpenOptionsExt;
                    options.share_mode(1).custom_flags(0x00200000);
                }
                let file = options.open(&path)?;
                guard.verify_replaceable_file(&path)?;
                ensure!(
                    same_file::Handle::from_file(file.try_clone()?)?
                        == same_file::Handle::from_path(&path)?,
                    "emoji cache identity changed"
                );
                let size = file.metadata()?.len();
                guard.verify()?;
                return Ok(Downloaded {
                    filename,
                    size,
                    cached: true,
                    conversion_fallback: false,
                    converted: false,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    ensure!(
        !opts.timeout.is_zero() && opts.max_bytes >= 4 && opts.max_bytes < u64::MAX,
        "invalid emoji download limits"
    );
    let bin_path = guard.output_root().join(format!("{md5}.bin"));
    let bin_target = guard
        .verify_replaceable_file(&bin_path)
        .and_then(|_| crate::toolkit::files::ExportTarget::capture_paths(&bin_path, protected));
    let redirects = opts.max_redirects;
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(opts.timeout)
        .connect_timeout(opts.timeout)
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if !matches!(attempt.url().scheme(), "http" | "https")
                || attempt.previous().len() > redirects
            {
                attempt.error("emoji redirect rejected")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| anyhow::anyhow!("emoji HTTP client unavailable"))?;
    let mut data = fetch(&client, &info.cdn_url, opts).unwrap_or_default();
    if data.is_empty() && !info.encrypt_url.is_empty() && !info.aes_key.is_empty() {
        let encrypted = fetch(&client, &info.encrypt_url, opts)?;
        data = decrypt(&encrypted, &info.aes_key).context(crate::business::media::Error::new(
            crate::business::media::Stage::Decode,
            crate::business::media::Failure::InvalidMaterial,
        ))?;
    }
    ensure!(
        data.len() >= 4,
        "emoji download failed or payload too short"
    );
    let mut ext = detect(&data);
    let mut conversion_fallback = false;
    let mut converted = false;
    if ext == "hevc" {
        match convert_hevc_to_jpeg(&data, guard, opts) {
            Ok(jpeg) => {
                data = jpeg;
                ext = "jpg";
                converted = true;
            }
            Err(_) => {
                ext = "bin";
                conversion_fallback = true;
            }
        }
    }
    let filename = format!("{md5}.{ext}");
    let path = guard.output_root().join(&filename);
    guard.verify_replaceable_file(&path)?;
    let target = if ext == "bin" {
        bin_target
    } else {
        crate::toolkit::files::ExportTarget::new_file(&path, protected)
    }
    .context(crate::business::media::Error::new(
        crate::business::media::Stage::Publication,
        crate::business::media::Failure::Refused,
    ))?;
    target
        .write_bytes_checked(&data, || {
            guard.verify()?;
            guard.verify_replaceable_file(&path)
        })
        .context(crate::business::media::Error::new(
            crate::business::media::Stage::Publication,
            crate::business::media::Failure::Refused,
        ))?;
    Ok(Downloaded {
        filename,
        size: data.len() as u64,
        cached: false,
        conversion_fallback,
        converted,
    })
}

fn fetch(client: &reqwest::blocking::Client, url: &str, opts: &DownloadOptions) -> Result<Vec<u8>> {
    let url = reqwest::Url::parse(url).map_err(|_| anyhow::anyhow!("invalid emoji URL"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
        "emoji URL must use HTTP(S)"
    );
    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, "Mozilla/5.0")
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|_| anyhow::anyhow!("emoji HTTP request failed"))?;
    ensure!(
        response
            .content_length()
            .is_none_or(|n| n <= opts.max_bytes),
        "emoji download exceeds byte limit"
    );
    let mut bytes = Vec::new();
    response
        .take(opts.max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("emoji body read failed"))?;
    ensure!(
        bytes.len() as u64 <= opts.max_bytes,
        "emoji download exceeds byte limit"
    );
    Ok(bytes)
}

fn decrypt(data: &[u8], key: &str) -> Result<Vec<u8>> {
    // CBC 的 IV 等于密钥，因此仅兼容 16 字节 AES 密钥。
    // bytes.fromhex 仅在完整字节之间接受六种 ASCII 空白，不能拆开高低半字节。
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
    let mut output = cbc::Decryptor::<aes::Aes128>::new_from_slices(&bytes[..], &bytes[..])
        .map_err(|_| anyhow::anyhow!("invalid emoji AES key"))?
        .decrypt_padded_vec_mut::<NoPadding>(data)
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
fn detect(data: &[u8]) -> &'static str {
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

fn convert_hevc_to_jpeg(
    data: &[u8],
    guard: &HostOutputGuard,
    opts: &DownloadOptions,
) -> Result<Vec<u8>> {
    let executable = opts.ffmpeg.as_ref().context("emoji conversion disabled")?;
    let start = find(data, VPS)
        .or_else(|| find(data, SPS))
        .context("emoji HEVC stream missing")?;
    guard.verify()?;
    let scratch = tempfile::tempdir_in(guard.output_root())?;
    let input = scratch.path().join("input.h265");
    let output = scratch.path().join("frame.jpg");
    fs::write(&input, &data[start..])?;
    let mut command = Command::new(executable);
    command
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-n",
            "-protocol_whitelist",
            "file",
            "-f",
            "hevc",
            "-i",
        ])
        .arg(&input)
        .args([
            "-frames:v",
            "1",
            "-threads",
            "1",
            "-c:v",
            "mjpeg",
            "-q:v",
            "2",
            "-f",
            "image2",
            "-update",
            "1",
            "-fs",
        ])
        .arg(opts.max_bytes.to_string())
        .arg(&output);
    let deadline = Instant::now()
        .checked_add(opts.timeout)
        .context("emoji conversion timeout invalid")?;
    let result =
        crate::windows_process::managed::output(&mut command, deadline, 64 * 1024, || false)
            .context("emoji converter failed")?;
    ensure!(result.status.success(), "emoji conversion failed");
    guard.verify()?;
    let mut data = Vec::new();
    fs::File::open(output)?
        .take(opts.max_bytes + 1)
        .read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 <= opts.max_bytes
            && data.starts_with(b"\xff\xd8\xff")
            && data.ends_with(b"\xff\xd9"),
        "invalid emoji JPEG output"
    );
    guard.verify()?;
    Ok(data)
}

#[cfg(test)]
#[path = "download_tests.rs"]
mod tests;
