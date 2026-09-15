//! 已校验 WAV 的本地无覆盖发布；身份、取消状态和响应预算由宿主回调检查。

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use crate::{attachment::local_files::HostOutputGuard, toolkit::asr::validate_wav};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedWav {
    pub path: PathBuf,
    pub size: u64,
    pub pcm_bytes: u64,
    pub sample_rate: u32,
}

/// 回调收到尚未发布的描述；应预构造响应并完成账号、取消和预算检查。
/// 提交成功后仅返回内存描述，不再进行可能失败的文件操作。
pub(crate) fn publish_wav_noclobber(
    wav: &[u8],
    guard: &HostOutputGuard,
    before_commit: impl FnOnce(&PublishedWav) -> Result<()>,
) -> Result<PublishedWav> {
    let info = validate_wav(wav)?;
    let published = PublishedWav {
        path: guard
            .output_root()
            .join(format!("{:x}.wav", Sha256::digest(wav))),
        size: wav.len() as u64,
        pcm_bytes: info.pcm_bytes,
        sample_rate: info.sample_rate,
    };
    guard.verify()?;
    let target = crate::toolkit::ExportTarget::new_file(&published.path, &[])?;
    let staged_path = RefCell::new(None);
    target
        .write_with_checked(
            |path| {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(path)?;
                file.write_all(wav)?;
                file.sync_all()?;
                verify_staged(path, wav)?;
                *staged_path.borrow_mut() = Some(path.to_path_buf());
                Ok(())
            },
            || {
                guard.verify()?;
                before_commit(&published)?;
                guard.verify()?;
                // The shared publisher verifies file identity; WAV also verifies callback-time bytes.
                let staged = staged_path.borrow();
                verify_staged(
                    staged.as_deref().context("WAV temporary file missing")?,
                    wav,
                )?;
                guard.verify()?;
                Ok(())
            },
        )
        .context("WAV output already exists or publication failed")?;
    Ok(published)
}

fn verify_staged(staged: &Path, expected: &[u8]) -> Result<()> {
    let mut file = std::fs::File::open(staged).context("WAV temporary file identity changed")?;
    let mut buffer = [0u8; 8192];
    for chunk in expected.chunks(buffer.len()) {
        file.read_exact(&mut buffer[..chunk.len()])?;
        ensure!(
            &buffer[..chunk.len()] == chunk,
            "WAV temporary content changed"
        );
    }
    ensure!(
        file.read(&mut buffer[..1])? == 0,
        "WAV temporary size changed"
    );
    Ok(())
}
