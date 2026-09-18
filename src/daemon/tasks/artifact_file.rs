//! Bounded reads of one pinned, independently published Windows file.
use crate::{attachment::local_files::HostOutputGuard, service::protocol::ServiceError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
};
use windows::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
};

pub(super) fn error(code: &'static str) -> ServiceError {
    let message = match code {
        "invalid_artifact_request" => "Invalid artifact request",
        "artifact_changed" => "Published artifact identity or content changed",
        "artifact_unsafe" => "Artifact failed file safety checks",
        "artifact_unavailable" => "Published artifact cannot be opened safely",
        "artifact_limit_exceeded" => "Artifact registration limit exceeded",
        "artifact_finalization_cancelled" => "Artifact finalization cancelled",
        "artifact_finalization_timeout" => "Artifact finalization deadline exceeded",
        _ => "Verified task result unavailable",
    };
    ServiceError::new(code, message)
}

pub(crate) struct FinalizeControl {
    cancel: tokio::sync::watch::Receiver<bool>,
    shutdown: tokio::sync::watch::Receiver<bool>,
    deadline: tokio::time::Instant,
    #[cfg(test)]
    pub(super) cancel_after_blocks: Option<(usize, tokio::sync::watch::Sender<bool>)>,
    #[cfg(test)]
    pub(super) blocks_read: usize,
}

impl FinalizeControl {
    pub(crate) fn new(
        cancel: tokio::sync::watch::Receiver<bool>,
        shutdown: tokio::sync::watch::Receiver<bool>,
        deadline: tokio::time::Instant,
    ) -> Self {
        Self {
            cancel,
            shutdown,
            deadline,
            #[cfg(test)]
            cancel_after_blocks: None,
            #[cfg(test)]
            blocks_read: 0,
        }
    }

    /// The task worker is already inside the daemon's cancellable Windows Job.
    pub(crate) fn supervised_worker() -> Self {
        Self::new(
            tokio::sync::watch::channel(false).1,
            tokio::sync::watch::channel(false).1,
            tokio::time::Instant::now() + std::time::Duration::from_secs(24 * 60 * 60),
        )
    }

    pub(crate) fn interrupted() -> Self {
        Self::new(
            tokio::sync::watch::channel(true).1,
            tokio::sync::watch::channel(false).1,
            tokio::time::Instant::now(),
        )
    }

    pub(super) fn check(&self) -> Result<(), ServiceError> {
        if *self.cancel.borrow() || *self.shutdown.borrow() {
            return Err(error("artifact_finalization_cancelled"));
        }
        if tokio::time::Instant::now() >= self.deadline {
            return Err(error("artifact_finalization_timeout"));
        }
        Ok(())
    }

    fn block_read(&mut self) -> Result<(), ServiceError> {
        #[cfg(test)]
        {
            self.blocks_read += 1;
            if let Some((remaining, sender)) = &mut self.cancel_after_blocks {
                *remaining = remaining.saturating_sub(1);
                if *remaining == 0 {
                    let _ = sender.send(true);
                }
            }
        }
        self.check()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    volume: u32,
    index_high: u32,
    index_low: u32,
    pub size: u64,
    modified_high: u32,
    modified_low: u32,
}

pub(super) struct Reader {
    file: File,
    path: PathBuf,
    parent: HostOutputGuard,
    pub identity: Identity,
}

fn identity(file: &File) -> Result<Identity, ServiceError> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
        .map_err(|_| error("artifact_unavailable"))?;
    if info.nNumberOfLinks != 1 || info.dwFileAttributes & (0x400 | 0x10) != 0 {
        return Err(error("artifact_unsafe"));
    }
    Ok(Identity {
        volume: info.dwVolumeSerialNumber,
        index_high: info.nFileIndexHigh,
        index_low: info.nFileIndexLow,
        size: ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64,
        modified_high: info.ftLastWriteTime.dwHighDateTime,
        modified_low: info.ftLastWriteTime.dwLowDateTime,
    })
}

impl Reader {
    pub fn open(path: &Path) -> Result<Self, ServiceError> {
        let parent = HostOutputGuard::new(path.parent().ok_or_else(|| error("artifact_unsafe"))?)
            .map_err(|_| error("artifact_unsafe"))?;
        let file = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .custom_flags(0x00200000)
            .open(path)
            .map_err(|_| error("artifact_unavailable"))?;
        let reader = Self {
            identity: identity(&file)?,
            file,
            parent,
            path: path.into(),
        };
        reader.verify()?;
        Ok(reader)
    }

    pub fn verify(&self) -> Result<(), ServiceError> {
        self.parent.verify().map_err(|_| error("artifact_unsafe"))?;
        let actual = identity(&self.file)?;
        let handle = same_file::Handle::from_file(
            self.file
                .try_clone()
                .map_err(|_| error("artifact_unavailable"))?,
        )
        .map_err(|_| error("artifact_unavailable"))?;
        let current =
            same_file::Handle::from_path(&self.path).map_err(|_| error("artifact_changed"))?;
        if actual != self.identity || handle != current {
            return Err(error("artifact_changed"));
        }
        Ok(())
    }

    pub fn bounded(&mut self, limit: u64) -> Result<Vec<u8>, ServiceError> {
        if self.identity.size > limit {
            return Err(error("result_unavailable"));
        }
        self.range(0, self.identity.size as usize)
    }

    pub fn range(&mut self, offset: u64, length: usize) -> Result<Vec<u8>, ServiceError> {
        self.verify()?;
        if offset
            .checked_add(length as u64)
            .is_none_or(|end| end > self.identity.size)
        {
            return Err(error("invalid_artifact_request"));
        }
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|_| error("artifact_unavailable"))?;
        let mut bytes = vec![0; length];
        self.file
            .read_exact(&mut bytes)
            .map_err(|_| error("artifact_changed"))?;
        self.verify()?;
        Ok(bytes)
    }

    pub fn bounded_controlled(
        &mut self,
        limit: u64,
        chunks_left: &mut usize,
        control: &mut FinalizeControl,
    ) -> Result<Vec<u8>, ServiceError> {
        control.check()?;
        let chunk = crate::service::task_artifacts::CHUNK_BYTES as u64;
        let needed = self.identity.size.div_ceil(chunk);
        if self.identity.size > limit || needed > *chunks_left as u64 {
            return Err(error("artifact_limit_exceeded"));
        }
        *chunks_left -= needed as usize;
        let mut bytes = Vec::with_capacity(self.identity.size as usize);
        while (bytes.len() as u64) < self.identity.size {
            control.check()?;
            let start = bytes.len() as u64;
            bytes.extend(self.range(start, (self.identity.size - start).min(chunk) as usize)?);
            control.block_read()?;
        }
        self.verify()?;
        control.check()?;
        Ok(bytes)
    }

    pub fn hashes(
        &mut self,
        chunks_left: &mut usize,
        control: &mut FinalizeControl,
    ) -> Result<(String, Vec<String>), ServiceError> {
        control.check()?;
        let chunk_size = crate::service::task_artifacts::CHUNK_BYTES as u64;
        if self.identity.size > crate::service::task_artifacts::MAX_SAFE_INTEGER
            || self.identity.size.div_ceil(chunk_size) > *chunks_left as u64
        {
            return Err(error("artifact_limit_exceeded"));
        }
        // Reserve before I/O; failed hashes and interrupted files never refund work.
        *chunks_left -= self.identity.size.div_ceil(chunk_size) as usize;
        let mut whole = Sha256::new();
        let mut chunks = Vec::new();
        let mut offset = 0;
        while offset < self.identity.size {
            control.check()?;
            let bytes = self.range(
                offset,
                (self.identity.size - offset).min(chunk_size) as usize,
            )?;
            whole.update(&bytes);
            chunks.push(format!("{:x}", Sha256::digest(&bytes)));
            offset += bytes.len() as u64;
            control.block_read()?;
        }
        self.verify()?;
        control.check()?;
        Ok((format!("{:x}", whole.finalize()), chunks))
    }
}

pub(super) fn relative(value: &str) -> Result<PathBuf, ServiceError> {
    use std::path::Component;
    if value.is_empty()
        || value.len() > 4096
        || value.contains('\\')
        || value.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with([' ', '.'])
                || part
                    .chars()
                    .any(|c| c.is_control() || "<>:\"|?*".contains(c))
        })
    {
        return Err(error("artifact_unsafe"));
    }
    let path = PathBuf::from(value);
    for component in value.split('/') {
        let stem = component
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let number = stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"));
        if matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || number.is_some_and(|n| {
            matches!(
                n,
                "1" | "2"
                    | "3"
                    | "4"
                    | "5"
                    | "6"
                    | "7"
                    | "8"
                    | "9"
                    | "\u{b9}"
                    | "\u{b2}"
                    | "\u{b3}"
            )
        }) {
            return Err(error("artifact_unsafe"));
        }
    }
    if !path
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(error("artifact_unsafe"));
    }
    Ok(path)
}
