//! 私有文件基础设施：调用者必须在首次写入敏感内容前限制已打开文件的权限。

#[cfg(windows)]
use anyhow::ensure;
use anyhow::Result;
use std::fs::File;

#[cfg(unix)]
pub(crate) fn restrict(file: &File) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn restrict(file: &File) -> Result<()> {
    use std::{mem::size_of, os::windows::io::AsRawHandle};
    use windows::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::*,
        Storage::FileSystem::{
            ReOpenFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, WRITE_DAC,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    struct Token(HANDLE);
    impl Drop for Token {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    let mut raw = HANDLE::default();
    // TokenUser 和 ACL 缓冲区按机器字对齐，调用期间 SID 与安全描述符保持有效。
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw)?;
        let token = Token(raw);
        let mut length = 0;
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut length);
        ensure!(length > 0, "无法取得当前用户身份");
        let mut user = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(user.as_mut_ptr().cast()),
            length,
            &mut length,
        )?;
        let sid = (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid;
        let acl_length = size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>()
            + GetLengthSid(sid) as usize;
        let mut storage = vec![0usize; acl_length.div_ceil(size_of::<usize>())];
        let acl = storage.as_mut_ptr().cast::<ACL>();
        InitializeAcl(acl, acl_length as u32, ACL_REVISION)?;
        AddAccessAllowedAce(acl, ACL_REVISION, 0x001f01ff, sid)?;
        let mut descriptor = SECURITY_DESCRIPTOR::default();
        let descriptor = PSECURITY_DESCRIPTOR((&mut descriptor as *mut SECURITY_DESCRIPTOR).cast());
        InitializeSecurityDescriptor(descriptor, 1)?;
        SetSecurityDescriptorDacl(descriptor, true, Some(acl), false)?;
        // 同时设置描述符控制位，不能仅依赖 SetKernelObjectSecurity 的信息标志。
        SetSecurityDescriptorControl(descriptor, SE_DACL_PROTECTED, SE_DACL_PROTECTED)?;
        let security = Token(ReOpenFile(
            HANDLE(file.as_raw_handle()),
            WRITE_DAC.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAGS_AND_ATTRIBUTES::default(),
        )?);
        SetKernelObjectSecurity(
            security.0,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
        .map_err(|_| anyhow::anyhow!("无法设置当前用户专用密钥文件权限"))?;
    }
    Ok(())
}

/// 读取实际文件 ACL：只允许当前用户一个显式访问项，且禁止继承父目录权限。
#[cfg(all(test, windows))]
pub(crate) fn assert_private_acl(path: &std::path::Path) {
    use std::{
        mem::size_of,
        os::windows::{
            ffi::OsStrExt,
            io::{FromRawHandle, OwnedHandle},
        },
    };
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{BOOL, HANDLE},
            Security::*,
            System::Threading::{GetCurrentProcess, OpenProcessToken},
        },
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut length = 0;
        let _ = GetFileSecurityW(
            PCWSTR(wide.as_ptr()),
            DACL_SECURITY_INFORMATION.0,
            PSECURITY_DESCRIPTOR::default(),
            0,
            &mut length,
        );
        assert!(length > 0);
        let mut storage = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        let descriptor = PSECURITY_DESCRIPTOR(storage.as_mut_ptr().cast());
        assert!(GetFileSecurityW(
            PCWSTR(wide.as_ptr()),
            DACL_SECURITY_INFORMATION.0,
            descriptor,
            length,
            &mut length,
        )
        .as_bool());
        let mut present = BOOL::default();
        let mut defaulted = BOOL::default();
        let mut acl = std::ptr::null_mut();
        GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted).unwrap();
        assert!(present.as_bool() && !acl.is_null());
        assert_eq!((*acl).AceCount, 1);
        let mut control = 0u16;
        let mut revision = 0;
        GetSecurityDescriptorControl(descriptor, &mut control, &mut revision).unwrap();
        assert_ne!(control & SE_DACL_PROTECTED.0, 0);

        let mut raw = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw).unwrap();
        let _token = OwnedHandle::from_raw_handle(raw.0);
        let mut length = 0;
        let _ = GetTokenInformation(raw, TokenUser, None, 0, &mut length);
        assert!(length > 0);
        let mut user = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        GetTokenInformation(
            raw,
            TokenUser,
            Some(user.as_mut_ptr().cast()),
            length,
            &mut length,
        )
        .unwrap();
        let sid = (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid;
        let mut ace = std::ptr::null_mut();
        GetAce(acl, 0, &mut ace).unwrap();
        let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
        assert_eq!((*ace).Header.AceType, 0); // ACCESS_ALLOWED_ACE_TYPE
        assert_eq!((*ace).Header.AceFlags, 0);
        assert_eq!((*ace).Mask, 0x001f01ff);
        EqualSid(PSID(std::ptr::addr_of_mut!((*ace).SidStart).cast()), sid).unwrap();
    }
}
