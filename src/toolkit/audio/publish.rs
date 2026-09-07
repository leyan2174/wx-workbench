//! 已校验 WAV 的本地无覆盖发布；身份、取消状态和响应预算由宿主回调检查。

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::PathBuf,
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
    let mut staged = tempfile::NamedTempFile::new_in(guard.output_root())?;
    staged.write_all(wav)?;
    staged.as_file().sync_all()?;
    verify_staged(&staged, wav)?;
    guard.verify()?;
    before_commit(&published)?;
    guard.verify()?;
    // reopen 核对临时文件身份；回调期间发生的替换或内容变化不能发布。
    verify_staged(&staged, wav)?;
    guard.verify()?;
    staged
        .persist_noclobber(&published.path)
        .map_err(|error| error.error)
        .context("WAV output already exists or publication failed")?;
    Ok(published)
}

fn verify_staged(staged: &tempfile::NamedTempFile, expected: &[u8]) -> Result<()> {
    let mut file = staged
        .reopen()
        .context("WAV temporary file identity changed")?;
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
