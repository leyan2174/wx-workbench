//! Concrete ASR engines and their process, network, and file-identity boundaries.

pub(crate) mod local;
pub(crate) mod local_python;
pub(crate) mod openai;
mod windows_supervision;

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

pub(crate) const MAX_AUDIO_BYTES: usize = 32 * 1024 * 1024;

/// Opens and hashes an engine artifact while retaining the same file handle.
/// Keeping the handle alive prevents replacement during execution on Windows.
pub(crate) fn file_digest(path: &Path) -> Result<(String, File)> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let mut file = options.open(path).context("open backend identity file")?;
    ensure!(
        file.metadata()?.is_file(),
        "backend identity must be a regular file"
    );
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok((format!("{:x}", hash.finalize()), file))
}
