//! Current-user-only protected kernel-object security, shared with local RPC.
use anyhow::{ensure, Result};
use std::mem::size_of;
use windows::Win32::{
    Foundation::{BOOL, HANDLE},
    Security::*,
};

// The closure cannot outlive any SID, ACL, or descriptor backing storage.
pub(crate) fn with_user_security<T>(
    action: impl FnOnce(PSID, &mut SECURITY_ATTRIBUTES) -> Result<T>,
) -> Result<T> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut raw = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw)?;
        let _token = OwnedHandle::from_raw_handle(raw.0);
        let mut length = 0;
        let _ = GetTokenInformation(raw, TokenUser, None, 0, &mut length);
        ensure!(length > 0, "cannot obtain task service user SID");
        let mut user = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        GetTokenInformation(
            raw,
            TokenUser,
            Some(user.as_mut_ptr().cast()),
            length,
            &mut length,
        )?;
        let sid = (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid;
        let length = size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>()
            + GetLengthSid(sid) as usize;
        let mut storage = vec![0usize; length.div_ceil(size_of::<usize>())];
        let acl = storage.as_mut_ptr().cast::<ACL>();
        InitializeAcl(acl, length as u32, ACL_REVISION)?;
        AddAccessAllowedAce(acl, ACL_REVISION, 0x001f01ff, sid)?;
        let mut descriptor = SECURITY_DESCRIPTOR::default();
        let descriptor = PSECURITY_DESCRIPTOR((&mut descriptor as *mut SECURITY_DESCRIPTOR).cast());
        InitializeSecurityDescriptor(descriptor, 1)?;
        SetSecurityDescriptorOwner(descriptor, sid, false)?;
        SetSecurityDescriptorDacl(descriptor, true, Some(acl), false)?;
        SetSecurityDescriptorControl(descriptor, SE_DACL_PROTECTED, SE_DACL_PROTECTED)?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: BOOL(0),
        };
        action(sid, &mut attributes)
    }
}
