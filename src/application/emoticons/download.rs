//! 仅处理账号显式提供的 URL；本模块不发现账号或密钥。
use crate::adapters::wechat::emoticons::{remote_format, types::EmojiInfo};
use crate::attachment::local_files::HostOutputGuard;
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
    let bin_target = guard.verify_replaceable_file(&bin_path).and_then(|_| {
        crate::infrastructure::publication::ExportTarget::capture_paths(&bin_path, protected)
    });
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
        data = remote_format::decrypt(&encrypted, &info.aes_key).context(
            crate::business::media::Error::new(
                crate::business::media::Stage::Decode,
                crate::business::media::Failure::InvalidMaterial,
            ),
        )?;
    }
    ensure!(
        data.len() >= 4,
        "emoji download failed or payload too short"
    );
    let mut ext = remote_format::detect(&data);
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
        crate::infrastructure::publication::ExportTarget::new_file(&path, protected)
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

fn convert_hevc_to_jpeg(
    data: &[u8],
    guard: &HostOutputGuard,
    opts: &DownloadOptions,
) -> Result<Vec<u8>> {
    let executable = opts.ffmpeg.as_ref().context("emoji conversion disabled")?;
    let stream = remote_format::hevc_stream(data)?;
    guard.verify()?;
    let scratch = tempfile::tempdir_in(guard.output_root())?;
    let input = scratch.path().join("input.h265");
    let output = scratch.path().join("frame.jpg");
    fs::write(&input, stream)?;
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
        crate::windows_process::managed::output(&mut command, true, deadline, 64 * 1024, || false)
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
