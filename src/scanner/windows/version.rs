use anyhow::{bail, Context, Result};
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
};
use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct WechatVersion {
    pub major: u16,
    pub minor: u16,
    pub build: u16,
    pub revision: u16,
}

impl std::fmt::Display for WechatVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}.{}.{}.{}",
            self.major, self.minor, self.build, self.revision
        )
    }
}

pub(super) fn detect(process: HANDLE) -> Result<(WechatVersion, PathBuf)> {
    let image_path = process_image_path(process)?;
    let version = file_version(&image_path)?;
    Ok((version, image_path))
}

fn process_image_path(process: HANDLE) -> Result<PathBuf> {
    let mut buffer = vec![0u16; 32_768];
    let mut length = buffer.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
        .context("读取 Weixin.exe 路径失败")?;
    }
    buffer.truncate(length as usize);
    if buffer.is_empty() {
        bail!("Weixin.exe 路径为空");
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer)))
}

fn file_version(path: &std::path::Path) -> Result<WechatVersion> {
    let wide_path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut ignored_handle = 0u32;
    let size =
        unsafe { GetFileVersionInfoSizeW(PCWSTR(wide_path.as_ptr()), Some(&mut ignored_handle)) };
    if size == 0 {
        bail!("无法读取文件版本: {}", path.display());
    }

    let mut data = vec![0u8; size as usize];
    unsafe {
        GetFileVersionInfoW(
            PCWSTR(wide_path.as_ptr()),
            0,
            size,
            data.as_mut_ptr() as *mut _,
        )
        .with_context(|| format!("读取文件版本资源失败: {}", path.display()))?;
    }

    let root: [u16; 2] = ['\\' as u16, 0];
    let mut value: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut value_len = 0u32;
    let ok = unsafe {
        VerQueryValueW(
            data.as_ptr() as *const _,
            PCWSTR(root.as_ptr()),
            &mut value,
            &mut value_len,
        )
        .as_bool()
    };
    if !ok || value.is_null() || value_len < std::mem::size_of::<VS_FIXEDFILEINFO>() as u32 {
        bail!("文件版本资源格式无效: {}", path.display());
    }

    let info = unsafe { std::ptr::read_unaligned(value as *const VS_FIXEDFILEINFO) };
    if info.dwSignature != 0xfeef04bd {
        bail!("文件版本签名无效: {}", path.display());
    }
    Ok(WechatVersion {
        major: (info.dwFileVersionMS >> 16) as u16,
        minor: info.dwFileVersionMS as u16,
        build: (info.dwFileVersionLS >> 16) as u16,
        revision: info.dwFileVersionLS as u16,
    })
}
