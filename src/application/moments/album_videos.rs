//! 相册视频媒体层：显式源文件、惰性前缀解密及有界流式发布；不负责扫描或渲染。
use crate::adapters::wechat::media::sns_keystream::{SnsKeystream, VIDEO_PREFIX_BYTES};
use crate::attachment::local_files::HostOutputGuard;
use crate::infrastructure::publication::ExportTarget;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub const MAX_VIDEO_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_URL_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoSource {
    Existing,
    Cache,
    Remote,
}
impl VideoSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::Cache => "cache",
            Self::Remote => "remote",
        }
    }
}

/// filename 仅含直属文件名，videos/ 前缀及 JSON 字段由编排层添加。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub filename: String,
    pub source: VideoSource,
    /// 沿用旧扩展名及下载完成语义，不声称已验证完整 MP4 容器。
    pub complete: bool,
    pub bytes: u64,
}

/// 错误不保留 URL、密钥、源路径或底层错误链。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoError {
    InvalidUrl,
    Http,
    HttpStatus,
    Size,
    BodyRead,
    MissingKey,
    EngineUnavailable,
    Decrypt,
    InvalidMp4,
    Source,
    Output,
}
impl std::fmt::Display for VideoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidUrl => "invalid video URL",
            Self::Http => "video HTTP request failed",
            Self::HttpStatus => "video HTTP status rejected",
            Self::Size => "video is empty, truncated or exceeds size limit",
            Self::BodyRead => "video body read failed",
            Self::MissingKey => "encrypted video requires a key",
            Self::EngineUnavailable => "video engine unavailable",
            Self::Decrypt => "video prefix decryption failed",
            Self::InvalidMp4 => "video header is not MP4",
            Self::Source => "video source rejected or unavailable",
            Self::Output => "video output rejected or unavailable",
        })
    }
}
impl std::error::Error for VideoError {}
type Result<T> = std::result::Result<T, VideoError>;

fn mp4(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp"
}

fn destination(name: &str, guard: &HostOutputGuard) -> Result<PathBuf> {
    if name.is_empty()
        || name.len() > 240
        || name.contains(['/', '\\', ':'])
        || name.ends_with([' ', '.'])
        || name.chars().any(char::is_control)
    {
        return Err(VideoError::Output);
    }
    let path = guard
        .output_root()
        .join(Path::new(name).with_extension("mp4"));
    guard
        .verify_replaceable_file(&path)
        .map_err(|_| VideoError::Output)?;
    Ok(path)
}

// Windows 只读共享固定句柄，禁止写入/删除；句柄元数据再拒绝硬链接和重解析点。
fn open_video(path: &Path) -> Result<Option<File>> {
    use std::os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
    };
    let file = match OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(VideoError::Source),
    };
    let check = || -> anyhow::Result<()> {
        let meta = file.metadata()?;
        anyhow::ensure!(
            meta.is_file() && meta.file_attributes() & 0x400 == 0,
            "unsafe file"
        );
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // 句柄在系统调用期间有效，缓冲区类型与 Windows API 一致。
        unsafe {
            GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?;
        }
        anyhow::ensure!(info.nNumberOfLinks == 1, "hardlink rejected");
        anyhow::ensure!(
            same_file::Handle::from_file(file.try_clone()?)? == same_file::Handle::from_path(path)?,
            "identity changed"
        );
        Ok(())
    };
    check().map_err(|_| VideoError::Source)?;
    Ok(Some(file))
}

fn outcome(path: &Path, source: VideoSource, complete: bool, bytes: u64) -> Outcome {
    Outcome {
        filename: path.file_name().unwrap().to_string_lossy().into_owned(),
        source,
        complete,
        bytes,
    }
}

pub(crate) fn reuse_existing_video(name: &str, guard: &HostOutputGuard) -> Result<Option<Outcome>> {
    let path = destination(name, guard)?;
    let Some(file) = open_video(&path)? else {
        return Ok(None);
    };
    let bytes = file.metadata().map_err(|_| VideoError::Source)?.len();
    if !(12..=MAX_VIDEO_BYTES).contains(&bytes) {
        return Ok(None);
    }
    let mut header = [0; 12];
    (&file)
        .read_exact(&mut header)
        .map_err(|_| VideoError::Source)?;
    guard
        .verify_replaceable_file(&path)
        .map_err(|_| VideoError::Output)?;
    Ok(mp4(&header).then(|| outcome(&path, VideoSource::Existing, true, bytes)))
}

/// 源路径由现有 cache::find_cached_video 提供；不扫描隐式根目录。
/// 源文件及祖先固定至提交结束，部分缓存需调用者显式允许。
pub(crate) fn copy_cached_video(
    source: &Path,
    name: &str,
    guard: &HostOutputGuard,
    allow_partial: bool,
) -> Result<Option<Outcome>> {
    let path = destination(name, guard)?;
    let complete = source
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mp4"));
    if !complete && !allow_partial {
        return Ok(None);
    }
    let mut protection =
        HostOutputGuard::new(guard.output_root()).map_err(|_| VideoError::Output)?;
    protection.protect(source).map_err(|_| VideoError::Source)?;
    let Some(mut file) = open_video(source)? else {
        return Ok(None);
    };
    let metadata = file.metadata().map_err(|_| VideoError::Source)?;
    let bytes = metadata.len();
    if bytes < 12 {
        return Ok(None);
    }
    if bytes > MAX_VIDEO_BYTES {
        return Err(VideoError::Size);
    }
    let mut header = [0; 12];
    file.read_exact(&mut header)
        .map_err(|_| VideoError::Source)?;
    if !mp4(&header) {
        return Ok(None);
    }
    let target = ExportTarget::capture_paths(&path, &[source.to_path_buf()])
        .map_err(|_| VideoError::Output)?;
    // Keep the pinned source available to both streaming and the commit check.
    let mut input = &file;
    target
        .write_with_checked(
            |temporary| {
                let mut staged = OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(temporary)?;
                staged.write_all(&header)?;
                let total = copy_tail(&mut input, &mut staged, 12, MAX_VIDEO_BYTES)?;
                if total != bytes {
                    return Err(VideoError::Size.into());
                }
                staged
                    .set_times(
                        fs::FileTimes::new()
                            .set_modified(metadata.modified().map_err(|_| VideoError::Source)?),
                    )
                    .map_err(|_| VideoError::Source)?;
                Ok(())
            },
            || {
                let verify = || -> anyhow::Result<()> {
                    protection.verify()?;
                    anyhow::ensure!(
                        same_file::Handle::from_file(file.try_clone()?)?
                            == same_file::Handle::from_path(source)?,
                        "source changed"
                    );
                    let now = file.metadata()?;
                    anyhow::ensure!(
                        now.len() == bytes && now.modified()? == metadata.modified()?,
                        "source changed"
                    );
                    Ok(())
                };
                verify().map_err(|_| VideoError::Source)?;
                guard.verify_replaceable_file(&path)?;
                Ok(())
            },
        )
        .map_err(publication_error)?;
    Ok(Some(outcome(&path, VideoSource::Cache, complete, bytes)))
}

fn publication_error(error: anyhow::Error) -> VideoError {
    error
        .downcast_ref::<VideoError>()
        .copied()
        .unwrap_or(VideoError::Output)
}

fn copy_tail(
    input: &mut impl Read,
    output: &mut impl Write,
    mut total: u64,
    limit: u64,
) -> Result<u64> {
    let mut buffer = [0; 64 * 1024];
    loop {
        let capacity = (limit.saturating_sub(total) + 1).min(buffer.len() as u64) as usize;
        let n = match input.read(&mut buffer[..capacity]) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result.map_err(|_| VideoError::BodyRead)?,
        };
        if n == 0 {
            return Ok(total);
        }
        total += n as u64;
        if total > limit {
            return Err(VideoError::Size);
        }
        output
            .write_all(&buffer[..n])
            .map_err(|_| VideoError::Output)?;
    }
}

fn valid_url(url: &reqwest::Url) -> bool {
    url.as_str().len() <= MAX_URL_BYTES
        && matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

fn client(timeout: Duration) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .referer(false)
        .timeout(timeout)
        .connect_timeout(timeout)
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if !valid_url(a.url())
                || a.previous().len() > 5
                || (a.previous().iter().any(|u| u.scheme() == "https")
                    && a.url().scheme() != "https")
            {
                a.error("video redirect rejected")
            } else {
                a.follow()
            }
        }))
        .build()
        .map_err(|_| VideoError::Http)
}

/// 明文分支不调用 engine；加密视频只调用 keystream(prefix)，不受 decode 全量输入限制。
pub(crate) fn download_video<'a>(
    url: &str,
    key: &str,
    name: &str,
    guard: &HostOutputGuard,
    engine: impl FnMut() -> Result<&'a SnsKeystream>,
) -> Result<Outcome> {
    download_with(url, key, name, guard, engine, MAX_VIDEO_BYTES, TIMEOUT)
}

fn download_with<'a>(
    url: &str,
    key: &str,
    name: &str,
    guard: &HostOutputGuard,
    mut engine: impl FnMut() -> Result<&'a SnsKeystream>,
    limit: u64,
    timeout: Duration,
) -> Result<Outcome> {
    let path = destination(name, guard)?;
    let target = ExportTarget::capture_paths(&path, &[]).map_err(|_| VideoError::Output)?;
    if url.len() > MAX_URL_BYTES || url.chars().any(char::is_control) {
        return Err(VideoError::InvalidUrl);
    }
    let url = reqwest::Url::parse(url).map_err(|_| VideoError::InvalidUrl)?;
    if !valid_url(&url) {
        return Err(VideoError::InvalidUrl);
    }
    let mut response = client(timeout)?
        .get(url)
        .header(reqwest::header::USER_AGENT, "MicroMessenger Client")
        .header(reqwest::header::ACCEPT, "*/*")
        .send()
        .map_err(|_| VideoError::Http)?;
    // 不将 206 或带 Content-Range 的片段当作完整下载。
    if response.status() != reqwest::StatusCode::OK
        || response
            .headers()
            .contains_key(reqwest::header::CONTENT_RANGE)
    {
        return Err(VideoError::HttpStatus);
    }
    let length = response.content_length();
    if length.is_some_and(|n| n == 0 || n > limit) {
        return Err(VideoError::Size);
    }
    let mut prefix = zeroize::Zeroizing::new(Vec::with_capacity(VIDEO_PREFIX_BYTES));
    // take/read_to_end 会继续短读，直到完整 128 KiB 或 EOF；不能对单次网络 read 假定填满。
    (&mut response)
        .take((VIDEO_PREFIX_BYTES as u64).min(limit + 1))
        .read_to_end(&mut prefix)
        .map_err(|_| VideoError::BodyRead)?;
    if prefix.is_empty() || prefix.len() as u64 > limit {
        return Err(VideoError::Size);
    }
    if !mp4(&prefix) {
        if key.trim().is_empty() {
            return Err(VideoError::MissingKey);
        }
        let mask = zeroize::Zeroizing::new(
            engine()?
                .keystream(key, prefix.len())
                .map_err(|_| VideoError::Decrypt)?,
        );
        for (byte, mask) in prefix.iter_mut().zip(mask.iter()) {
            *byte ^= mask;
        }
        if !mp4(&prefix) {
            return Err(VideoError::InvalidMp4);
        }
    }
    let mut total = 0;
    target
        .write_with_checked(
            |temporary| {
                let mut staged = OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(temporary)?;
                staged.write_all(&prefix)?;
                total = copy_tail(&mut response, &mut staged, prefix.len() as u64, limit)?;
                if length.is_some_and(|n| n != total) {
                    return Err(VideoError::Size.into());
                }
                Ok(())
            },
            || guard.verify_replaceable_file(&path),
        )
        .map_err(publication_error)?;
    Ok(outcome(&path, VideoSource::Remote, true, total))
}

#[cfg(test)]
#[path = "album_videos_tests.rs"]
mod tests;
