//! 显式指定当前账号的 V2 图片样本，在提交配置前完成暂存。

use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tempfile::{NamedTempFile, TempPath};
use zeroize::Zeroizing;

use crate::{
    attachment::{
        decoder::{self, V2KeyMaterial, V2_MAGIC},
        image_key::ImageKeyMaterial,
        local_files::HostOutputGuard,
        native_image::MAX_DAT_BYTES,
    },
    runtime::RuntimeContext,
};

#[derive(Debug, Clone, Default, clap::Args, serde::Serialize, serde::Deserialize)]
pub struct SampleArgs {
    /// 显式指定当前账号 msg/attach 目录内的 V2 DAT 文件。
    #[arg(long, requires = "sample_output")]
    pub sample_input: Option<PathBuf>,
    /// 显式指定新输出文件，其父目录必须已存在且位于受保护的账号数据之外。
    #[arg(long, requires = "sample_input")]
    pub sample_output: Option<PathBuf>,
}

impl SampleArgs {
    pub(crate) fn validate_request(&self) -> Result<()> {
        match (&self.sample_input, &self.sample_output) {
            (None, None) => Ok(()),
            (Some(input), Some(output)) => {
                ensure!(input.is_absolute(), "样本输入必须使用绝对路径");
                ensure!(output.is_absolute(), "样本输出必须使用绝对路径");
                ensure!(
                    input
                        .extension()
                        .is_some_and(|s| s.eq_ignore_ascii_case("dat")),
                    "样本输入必须是 DAT 文件"
                );
                ensure!(input != output, "样本输出不能覆盖输入");
                Ok(())
            }
            _ => bail!("--sample-input 与 --sample-output 必须同时提供"),
        }
    }
}

pub(super) struct PreparedSample {
    guard: HostOutputGuard,
    output: PathBuf,
    data: Zeroizing<Vec<u8>>,
}

pub(super) struct StagedSample {
    // 先释放只读句柄，再清理临时文件，最后释放路径固定句柄。
    staged_file: File,
    temporary: TempPath,
    guard: HostOutputGuard,
    output: PathBuf,
    format: &'static str,
    bytes: u64,
    digest: [u8; 32],
}

/// 先验证路径隔离，再读取源文件；不创建或修改输出。
pub(super) fn prepare(
    runtime: &RuntimeContext,
    args: &SampleArgs,
) -> Result<Option<PreparedSample>> {
    let (input, output) = match (&args.sample_input, &args.sample_output) {
        (None, None) => return Ok(None),
        (Some(input), Some(output)) => (input, output),
        _ => bail!("--sample-input 与 --sample-output 必须同时提供"),
    };
    ensure!(input.is_absolute(), "样本输入必须使用绝对路径");
    ensure!(output.is_absolute(), "样本输出必须使用绝对路径");
    ensure!(
        input
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("dat")),
        "样本输入必须是 DAT 文件"
    );
    let parent = output.parent().context("样本输出缺少父目录")?;
    let mut guard = HostOutputGuard::new(parent)?;
    // 同时检查目标文件名和现有父目录的身份。
    guard.verify_replaceable_file(output)?;
    require_absent(output)?;

    let account = runtime.config.db_dir.parent().context("账号根目录缺失")?;
    let attach = account.join("msg").join("attach");
    guard.protect(account)?;
    guard.protect(&runtime.config.db_dir)?;
    // 保护配置的路径边界，不锁定配置文件句柄：调用方会在
    // stage 与 publish 之间原子替换配置。
    guard.protect(&runtime.config_path)?;
    guard.protect(&runtime.config.keys_file)?;
    guard.protect_future(&runtime.config.decrypted_dir)?;
    guard.protect(&attach)?;
    guard.protect(input)?;
    let attach_path = attach.canonicalize().context("样本附件根目录不可用")?;
    let input_path = input.canonicalize().context("样本输入不可用")?;
    ensure!(
        input_path.ancestors().skip(1).any(|ancestor| {
            ancestor
                .as_os_str()
                .eq_ignore_ascii_case(attach_path.as_os_str())
        }),
        "样本输入必须位于当前账号的 msg/attach 目录内"
    );
    guard.pin_input(input)?;
    guard.verify()?;

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    let source = options.open(input).context("无法打开已固定的样本文件")?;
    require_single_link(&source)?;
    let metadata = source.metadata()?;
    ensure!(metadata.is_file(), "样本输入必须是普通文件");
    ensure!(metadata.len() <= MAX_DAT_BYTES, "样本超过 64 MiB 大小限制");
    let mut data = Zeroizing::new(Vec::new());
    (&source).take(MAX_DAT_BYTES + 1).read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 == metadata.len() && data.len() as u64 <= MAX_DAT_BYTES,
        "样本发生变化或超过 64 MiB 大小限制"
    );
    ensure!(data.starts_with(&V2_MAGIC), "样本必须包含 V2 DAT 文件标识");
    guard.verify()?;
    Ok(Some(PreparedSample {
        guard,
        output: output.clone(),
        data,
    }))
}

impl PreparedSample {
    pub(super) fn stage(self, material: &ImageKeyMaterial) -> Result<StagedSample> {
        self.guard.verify()?;
        self.guard.verify_replaceable_file(&self.output)?;
        require_absent(&self.output)?;
        let decoded = decoder::dispatch(
            &self.data,
            V2KeyMaterial {
                aes_key: Some(&material.aes_key),
                xor_key: material.xor_key,
            },
        )
        .map_err(|_| anyhow::anyhow!("V2 样本解码失败"))?;
        let data = Zeroizing::new(decoded.data);
        ensure!(
            data.len() as u64 <= MAX_DAT_BYTES,
            "解码后的样本超过 64 MiB 大小限制"
        );
        self.guard.verify()?;
        let mut temporary = NamedTempFile::new_in(self.guard.output_root())?;
        temporary.write_all(&data)?;
        temporary.as_file().sync_all()?;
        let identity = same_file::Handle::from_path(temporary.path())?;
        let temporary = temporary.into_temp_path();
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // 允许重命名和清理，但禁止暂存期间写入。
            options.share_mode(1 | 4).custom_flags(0x00200000);
        }
        let staged_file = options.open(&temporary)?;
        ensure!(
            identity == same_file::Handle::from_file(staged_file.try_clone()?)?,
            "暂存样本在固定句柄时发生变化"
        );
        self.guard.verify()?;
        Ok(StagedSample {
            staged_file,
            temporary,
            guard: self.guard,
            output: self.output,
            format: decoded.format,
            bytes: data.len() as u64,
            digest: Sha256::digest(data.as_slice()).into(),
        })
    }
}

impl StagedSample {
    /// 仅在调用方成功提交配置后调用。
    pub(super) fn publish(mut self) -> Result<Value> {
        self.guard.verify()?;
        self.guard.verify_replaceable_file(&self.output)?;
        require_absent(&self.output)?;
        require_single_link(&self.staged_file)?;
        ensure!(
            !fs::symlink_metadata(&self.temporary)?
                .file_type()
                .is_symlink()
                && same_file::Handle::from_file(self.staged_file.try_clone()?)?
                    == same_file::Handle::from_path(&self.temporary)?,
            "暂存样本的路径身份发生变化"
        );
        self.guard.verify_replaceable_file(&self.temporary)?;
        ensure!(
            self.staged_file.metadata()?.len() == self.bytes,
            "暂存样本发生变化"
        );
        let mut digest = Sha256::new();
        let mut buffer = Zeroizing::new([0u8; 64 * 1024]);
        let mut remaining = self.bytes;
        while remaining > 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            self.staged_file.read_exact(&mut buffer[..count])?;
            digest.update(&buffer[..count]);
            remaining -= count as u64;
        }
        ensure!(
            <[u8; 32]>::from(digest.finalize()) == self.digest,
            "暂存样本的内容发生变化"
        );
        self.guard.verify()?;
        self.guard.verify_replaceable_file(&self.temporary)?;
        ensure!(
            same_file::Handle::from_file(self.staged_file.try_clone()?)?
                == same_file::Handle::from_path(&self.temporary)?,
            "暂存样本的路径身份在发布前发生变化"
        );
        let report = json!({ "path": self.output, "format": self.format, "bytes": self.bytes });
        self.temporary
            .persist_noclobber(&self.output)
            .map_err(|error| error.error)
            .context("无法发布样本；不会覆盖已有输出")?;
        Ok(report)
    }
}

fn require_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("无法检查样本输出路径"),
        Ok(_) => bail!("样本输出已存在，拒绝覆盖"),
    }
}

fn require_single_link(file: &File) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::{
            Foundation::HANDLE,
            Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // 借用的文件在整个 Win32 调用期间持有有效句柄。
        unsafe {
            GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?;
        }
        ensure!(info.nNumberOfLinks == 1, "拒绝具有多个硬链接的样本");
        ensure!(info.dwFileAttributes & 0x400 == 0, "拒绝样本重解析点");
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = file;
        bail!("样本导出需要 Windows 路径固定句柄支持")
    }
}
