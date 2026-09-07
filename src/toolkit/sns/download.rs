//! 仅供调用者明确授权后的联网下载；不发现账号、不读取环境或真实数据。
use crate::attachment::local_files::HostOutputGuard;
use anyhow::{ensure, Context, Result};
use std::{
    io::{Read, Write},
    path::Path,
    time::Duration,
};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const REFERER: &str = "https://weixin.qq.com/";

#[derive(Clone, Debug)]
pub(crate) struct Options {
    /// 整个请求（含重定向与响应体）的超时，不是单次读取超时。
    pub timeout: Duration,
    pub max_bytes: u64,
    pub max_redirects: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            max_bytes: 128 * 1024 * 1024,
            max_redirects: 5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    Jpg,
    Png,
    Gif,
    Webp,
    Mov,
    Mp4,
    Bin,
}

impl Format {
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Jpg => "jpg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::Mov => "mov",
            Self::Mp4 => "mp4",
            Self::Bin => "bin",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// 仅文件名；调用者必须用它更新导出引用，不能继续使用无后缀的输入名称。
    pub actual_filename: String,
    pub bytes: u64,
    pub format: Format,
}

fn allowed_url(url: &reqwest::Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

/// destination 必须是守卫输出根目录的直属文件；已有扩展名按原样保留。
/// 阻塞 API：异步宿主应放在阻塞线程中，授权与输入保护由宿主完成。
pub(crate) fn download(
    url: &str,
    destination: &Path,
    guard: &HostOutputGuard,
    options: &Options,
) -> Result<Outcome> {
    guard.verify_replaceable_file(destination)?;
    // 防止配置意外关闭限制，同时对内存、磁盘、时间和跳转次数设置硬上限。
    ensure!(
        !options.timeout.is_zero()
            && options.timeout <= Duration::from_secs(300)
            && (100..=1024 * 1024 * 1024).contains(&options.max_bytes)
            && options.max_redirects <= 20,
        "invalid SNS download limits"
    );
    ensure!(url.len() <= 16 * 1024, "SNS URL too long");
    let url = reqwest::Url::parse(url).map_err(|_| anyhow::anyhow!("invalid SNS URL"))?;
    ensure!(
        allowed_url(&url),
        "SNS URL requires HTTP(S) without credentials"
    );
    let redirects = options.max_redirects;
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(options.timeout)
        .connect_timeout(options.timeout)
        .referer(false)
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() > redirects
                || !allowed_url(attempt.url())
                || attempt.url().as_str().len() > 16 * 1024
            {
                attempt.error("SNS redirect rejected")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| anyhow::anyhow!("SNS HTTP client unavailable"))?;
    // 不保留 reqwest 错误源；其错误链可能包含签名 URL、查询参数及凭据。
    let mut response = client
        .get(url)
        .timeout(options.timeout)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::REFERER, REFERER)
        .send()
        .map_err(|_| anyhow::anyhow!("SNS HTTP request failed"))?;
    ensure!(
        response.status() == reqwest::StatusCode::OK,
        "SNS HTTP status must be 200"
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|n| n <= options.max_bytes),
        "SNS download exceeds byte limit"
    );
    guard.verify()?;
    let mut staged = tempfile::NamedTempFile::new_in(guard.output_root())?;
    let mut prefix = Vec::with_capacity(12);
    let mut bytes = 0u64;
    let mut buffer = [0u8; 16 * 1024];
    loop {
        // 多读至多一个字节以区分恰好达到上限和超限，不按 Content-Length 分配内存。
        let capacity = (options.max_bytes - bytes + 1).min(buffer.len() as u64) as usize;
        let n = response
            .read(&mut buffer[..capacity])
            .map_err(|_| anyhow::anyhow!("SNS response body read failed"))?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        ensure!(
            bytes <= options.max_bytes,
            "SNS download exceeds byte limit"
        );
        prefix.extend_from_slice(&buffer[..n.min(12 - prefix.len())]);
        staged.write_all(&buffer[..n])?;
    }
    ensure!(bytes >= 100, "SNS payload must contain at least 100 bytes");
    let format = detect(&prefix);
    let mut actual_filename = destination
        .file_name()
        .and_then(|n| n.to_str())
        .context("invalid SNS output filename")?
        .to_owned();
    // 与 Python splitext 一致：纯前导点不是扩展名（例如 .hidden）。
    if !actual_filename.trim_start_matches('.').contains('.') {
        actual_filename.push('.');
        actual_filename.push_str(format.extension());
    }
    let actual = guard.output_root().join(&actual_filename);
    staged.as_file().sync_all()?;
    guard.verify_replaceable_file(destination)?;
    guard.verify_replaceable_file(&actual)?;
    // 复用同卷临时文件发布；失败不会先截断旧文件。提交后不再运行可能失败的检查。
    staged
        .persist(&actual)
        .map_err(|e| e.error)
        .context("SNS output publication failed")?;
    Ok(Outcome {
        actual_filename,
        bytes,
        format,
    })
}

fn detect(data: &[u8]) -> Format {
    if data.starts_with(b"\xff\xd8\xff") {
        Format::Jpg
    } else if data.starts_with(b"\x89PNG") {
        Format::Png
    } else if data.starts_with(b"GIF8") {
        Format::Gif
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        Format::Webp
    } else if data.get(4..8) == Some(b"ftyp") && data.len() >= 12 {
        if data[8..12].eq_ignore_ascii_case(b"qt  ") {
            Format::Mov
        } else {
            Format::Mp4
        }
    } else {
        Format::Bin
    }
}

#[cfg(all(test, windows))]
#[path = "download_tests.rs"]
mod tests;
