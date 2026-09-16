//! 相册图片独立下载层；账号隔离及源路径保护由调用者配置的守卫负责。
use super::decode::html_unescape;
use crate::adapters::wechat::media::sns_keystream::SnsKeystream;
use crate::attachment::local_files::HostOutputGuard;
use crate::infrastructure::publication::ExportTarget;
use regex::Regex;
use std::{fs, io::Read, path::Path, sync::OnceLock, time::Duration};

const MAX_BYTES: usize = 25 * 1024 * 1024;
const MAX_URL_BYTES: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSource {
    Existing,
    Remote,
    RemoteDecrypted,
}
impl ImageSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::Remote => "remote",
            Self::RemoteDecrypted => "remote_decrypted",
        }
    }
}

/// filename 是输出目录直属文件名；编排层自行添加 images/ 前缀。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub filename: String,
    pub source: ImageSource,
    pub bytes: u64,
}

/// 不携带底层错误链，避免 HTTP 错误中的 URL、token 或 key 外泄。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    MissingUrl,
    InvalidUrl,
    ResponseSize,
    Http,
    HttpStatus,
    BodyRead,
    UnsupportedFormat,
    EngineUnavailable,
    Decrypt,
    Output,
}
impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MissingUrl => "missing image URL",
            Self::InvalidUrl => "invalid image URL",
            Self::ResponseSize => "image response is empty or too large",
            Self::Http => "image HTTP request failed",
            Self::HttpStatus => "image HTTP status rejected",
            Self::BodyRead => "image body read failed",
            Self::UnsupportedFormat => "decrypted image format is not supported",
            Self::EngineUnavailable => "image engine unavailable",
            Self::Decrypt => "image decryption failed or exceeded engine limits",
            Self::Output => "image output rejected or unavailable",
        })
    }
}
impl std::error::Error for ImageError {}

/// 顺序与 URL 候选一致；不记录候选地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub errors: Vec<ImageError>,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, error) in self.errors.iter().enumerate() {
            if i > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{error}")?;
        }
        Ok(())
    }
}
impl std::error::Error for Failure {}
impl From<ImageError> for Failure {
    fn from(e: ImageError) -> Self {
        Self { errors: vec![e] }
    }
}
type Result<T> = std::result::Result<T, ImageError>;

fn trim(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

pub fn sns_image_url_candidates(url: &str, token: &str) -> Vec<String> {
    let value = html_unescape(url);
    let mut value = trim(&value).to_owned();
    if value.is_empty() {
        return Vec::new();
    }
    if value
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
    {
        value.replace_range(..7, "https://");
    }
    let token = trim(token);
    let signed = |mut candidate: String| {
        static TOKEN: OnceLock<Regex> = OnceLock::new();
        if !token.is_empty()
            && !TOKEN
                .get_or_init(|| Regex::new(r"(?i)(?:\?|&)token=").unwrap())
                .is_match(&candidate)
        {
            candidate.push(if candidate.contains('?') { '&' } else { '?' });
            candidate.push_str("token=");
            for byte in token.bytes() {
                if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                    candidate.push(byte as char);
                } else {
                    use std::fmt::Write;
                    write!(candidate, "%{byte:02X}").unwrap();
                }
            }
            candidate.push_str("&idx=1");
        }
        candidate
    };
    // Python 正则亦作用于查询字符串，不能用 URL 规范化改变历史候选顺序。
    static SIZE: OnceLock<Regex> = OnceLock::new();
    let full = SIZE
        .get_or_init(|| Regex::new(r"/(?:150|200|480)(\?|$)").unwrap())
        .replace_all(&value, "/0$1");
    let original = signed(value.clone());
    let full = signed(full.into_owned());
    if full == original {
        vec![full]
    } else {
        vec![full, original]
    }
}

#[cfg(test)]
pub fn fix_sns_image_url(url: &str, token: &str) -> String {
    sns_image_url_candidates(url, token)
        .into_iter()
        .next()
        .unwrap_or_default()
}

fn detect(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("gif")
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        Some("webp")
    } else {
        None
    }
}

fn destination(guard: &HostOutputGuard, name: &str, ext: &str) -> Result<std::path::PathBuf> {
    if name.is_empty() || name.contains(['/', '\\', ':']) || matches!(name, "." | "..") {
        return Err(ImageError::Output);
    }
    let path = guard
        .output_root()
        .join(Path::new(name).with_extension(ext));
    guard
        .verify_replaceable_file(&path)
        .map_err(|_| ImageError::Output)?;
    Ok(path)
}

pub(crate) fn reuse_existing_image(name: &str, guard: &HostOutputGuard) -> Result<Option<Outcome>> {
    for ext in ["jpg", "png", "gif", "webp"] {
        let path = destination(guard, name, ext)?;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1).custom_flags(0x00200000);
        }
        let file = match options.open(&path) {
            Ok(file) => file,
            Err(_) => continue,
        };
        guard
            .verify_replaceable_file(&path)
            .map_err(|_| ImageError::Output)?;
        let read = || -> anyhow::Result<Option<Outcome>> {
            anyhow::ensure!(
                same_file::Handle::from_file(file.try_clone()?)?
                    == same_file::Handle::from_path(&path)?,
                "image identity changed"
            );
            let mut header = Vec::with_capacity(16);
            (&file).take(16).read_to_end(&mut header)?;
            if detect(&header) != Some(ext) {
                return Ok(None);
            }
            let bytes = file.metadata()?.len();
            guard.verify()?;
            Ok(Some(Outcome {
                filename: path.file_name().unwrap().to_string_lossy().into_owned(),
                source: ImageSource::Existing,
                bytes,
            }))
        };
        if let Some(outcome) = read().map_err(|_| ImageError::Output)? {
            return Ok(Some(outcome));
        }
    }
    Ok(None)
}

pub(crate) fn save_image(data: &[u8], name: &str, guard: &HostOutputGuard) -> Result<Outcome> {
    if data.is_empty() || data.len() > MAX_BYTES {
        return Err(ImageError::ResponseSize);
    }
    let ext = detect(data).ok_or(ImageError::UnsupportedFormat)?;
    let path = destination(guard, name, ext)?;
    let publish = || -> anyhow::Result<()> {
        ExportTarget::new_file(&path, &[])?
            .write_bytes_checked(data, || guard.verify_replaceable_file(&path))
    };
    publish().map_err(|_| ImageError::Output)?;
    Ok(Outcome {
        filename: path.file_name().unwrap().to_string_lossy().into_owned(),
        source: ImageSource::Remote,
        bytes: data.len() as u64,
    })
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .referer(false)
        .timeout(TIMEOUT)
        .connect_timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.url().as_str().len() > MAX_URL_BYTES
                || !matches!(a.url().scheme(), "http" | "https")
                || a.previous().len() > 5
                || (a.previous().iter().any(|u| u.scheme() == "https")
                    && a.url().scheme() != "https")
                || !a.url().username().is_empty()
                || a.url().password().is_some()
            {
                a.error("image redirect rejected")
            } else {
                a.follow()
            }
        }))
        .build()
        .map_err(|_| ImageError::Http)
}

fn fetch(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>> {
    if url.len() > MAX_URL_BYTES {
        return Err(ImageError::InvalidUrl);
    }
    let url = reqwest::Url::parse(url).map_err(|_| ImageError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ImageError::InvalidUrl);
    }
    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, "MicroMessenger Client")
        .header(reqwest::header::ACCEPT, "*/*")
        .header(reqwest::header::REFERER, "https://mp.weixin.qq.com/")
        .send()
        .map_err(|_| ImageError::Http)?;
    if !response.status().is_success() {
        return Err(ImageError::HttpStatus);
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err(ImageError::ResponseSize);
    }
    let mut data = Vec::new();
    response
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut data)
        .map_err(|_| ImageError::BodyRead)?;
    if data.is_empty() || data.len() > MAX_BYTES {
        return Err(ImageError::ResponseSize);
    }
    Ok(data)
}

/// engine 可闭包惰性初始化；已有图片或明文响应不会调用它。
/// 当前 SnsKeystream 的密钥流上限由 runtime 自身执行，不重复生成分块流。
pub(crate) fn download_sns_image<'a>(
    url: &str,
    key: &str,
    token: &str,
    name: &str,
    guard: &HostOutputGuard,
    mut engine: impl FnMut() -> Result<&'a SnsKeystream>,
) -> std::result::Result<Outcome, Failure> {
    let mut http = None;
    download_with(
        url,
        key,
        token,
        name,
        guard,
        |url| {
            if http.is_none() {
                http = Some(client()?);
            }
            fetch(http.as_ref().unwrap(), url)
        },
        |key, size| {
            engine()?
                .keystream(key, size)
                .map_err(|_| ImageError::Decrypt)
        },
    )
}

// 私有注入仅分离网络和密钥流副作用；候选、复用、XOR、发布均运行真实控制流。
fn download_with(
    url: &str,
    key: &str,
    token: &str,
    name: &str,
    guard: &HostOutputGuard,
    mut fetch: impl FnMut(&str) -> Result<Vec<u8>>,
    mut stream: impl FnMut(&str, usize) -> Result<Vec<u8>>,
) -> std::result::Result<Outcome, Failure> {
    if let Some(existing) = reuse_existing_image(name, guard)? {
        return Ok(existing);
    }
    if url.len() > MAX_URL_BYTES || token.len() > MAX_URL_BYTES {
        return Err(ImageError::InvalidUrl.into());
    }
    let candidates = sns_image_url_candidates(url, token);
    if candidates.is_empty() {
        return Err(ImageError::MissingUrl.into());
    }
    let mut errors = Vec::new();
    for candidate in candidates {
        let mut attempt = || -> Result<Outcome> {
            let mut data = fetch(&candidate)?;
            if data.is_empty() || data.len() > MAX_BYTES {
                return Err(ImageError::ResponseSize);
            }
            let encrypted = detect(&data).is_none();
            let key = trim(key);
            if encrypted && !key.is_empty() && key != "0" {
                let stream = zeroize::Zeroizing::new(stream(key, data.len())?);
                if stream.len() != data.len() {
                    return Err(ImageError::Decrypt);
                }
                for (byte, mask) in data.iter_mut().zip(stream.iter()) {
                    *byte ^= mask;
                }
            }
            let mut outcome = save_image(&data, name, guard)?;
            outcome.source = if encrypted {
                ImageSource::RemoteDecrypted
            } else {
                ImageSource::Remote
            };
            Ok(outcome)
        };
        match attempt() {
            Ok(outcome) => return Ok(outcome),
            Err(error) => errors.push(error),
        }
    }
    Err(Failure { errors })
}

#[cfg(test)]
#[path = "album_images_tests.rs"]
mod tests;
