//! Current-user DPAPI. This does not isolate processes belonging to that user.
use anyhow::{ensure, Result};
use windows::Win32::{
    Foundation::{LocalFree, HLOCAL},
    Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    },
};
use zeroize::{Zeroize, Zeroizing};

pub(crate) fn transform(data: &[u8], decrypt: bool) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(
        !data.is_empty() && data.len() <= 16 * 1024 * 1024,
        "Invalid protected data size"
    );
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        let result = if decrypt {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptProtectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        result.map_err(|_| anyhow::anyhow!("Current-user DPAPI operation failed"))?;
        if output.pbData.is_null() || output.cbData == 0 {
            if !output.pbData.is_null() {
                let _ = LocalFree(HLOCAL(output.pbData.cast()));
            }
            anyhow::bail!("Current-user DPAPI returned empty data");
        }
        let bytes = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
        let protected = Zeroizing::new(bytes.to_vec());
        bytes.zeroize();
        let _ = LocalFree(HLOCAL(output.pbData.cast()));
        Ok(protected)
    }
}
