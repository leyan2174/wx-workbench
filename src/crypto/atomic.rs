//! crypto 的同卷发布边界：失败只删除临时产物，不截断旧缓存或输入硬链接。
use anyhow::{ensure, Context, Result};
use std::fs::{self, File, OpenOptions};
use std::os::windows::{
    fs::{MetadataExt, OpenOptionsExt},
    io::AsRawHandle,
};
use std::path::{Path, PathBuf};
use windows::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
};

pub(super) fn open_source(path: &Path) -> Result<File> {
    // 允许微信继续写源文件，但禁止删除/替换所读文件。认证与时间戳复核负责
    // 拒绝读到的损坏页面；这不是 SQLite 事务锁，也不宣称冻结整个在线账号。
    let file = OpenOptions::new()
        .read(true)
        .share_mode(3)
        .custom_flags(0x00200000)
        .open(path)?;
    regular(&file)?;
    Ok(file)
}

fn regular(file: &File) -> Result<()> {
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.file_attributes() & 0x400 == 0,
        "拒绝非普通文件或重解析点"
    );
    Ok(())
}

fn single_link(file: &File) -> Result<()> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // 句柄在调用期间存活；结构体按 Windows API 的固定布局分配。
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?;
    }
    ensure!(info.nNumberOfLinks == 1, "拒绝多重硬链接输出");
    Ok(())
}

pub(super) struct Output {
    temporary: tempfile::NamedTempFile,
    path: PathBuf,
    parent: File,
    old: Option<File>,
}

impl Output {
    pub(super) fn new(path: &Path, sources: &[&File]) -> Result<Self> {
        let path = std::path::absolute(path)?;
        let directory = path.parent().context("输出缺少父目录")?;
        fs::create_dir_all(directory)?;
        // 固定目录句柄，发布前再验证原路径仍指向它，避免目录改名后误投递。
        let parent = OpenOptions::new()
            .access_mode(0x80)
            .share_mode(3)
            .custom_flags(0x02200000)
            .open(directory)?;
        ensure!(
            parent.metadata()?.is_dir() && parent.metadata()?.file_attributes() & 0x400 == 0,
            "输出父目录无效"
        );
        let old = match OpenOptions::new()
            .read(true)
            .share_mode(5)
            .custom_flags(0x00200000)
            .open(&path)
        {
            Ok(file) => {
                regular(&file)?;
                single_link(&file)?;
                let id = same_file::Handle::from_file(file.try_clone()?)?;
                for source in sources {
                    ensure!(
                        id != same_file::Handle::from_file(source.try_clone()?)?,
                        "输出不能覆盖输入文件"
                    );
                }
                Some(file)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let temporary = tempfile::NamedTempFile::new_in(directory)?;
        Ok(Self {
            temporary,
            path,
            parent,
            old,
        })
    }

    pub(super) fn file(&mut self) -> &mut File {
        self.temporary.as_file_mut()
    }

    pub(super) fn copy_old(&mut self) -> Result<()> {
        let old = self.old.as_mut().context("WAL 输出数据库不存在")?;
        std::io::copy(old, self.temporary.as_file_mut())?;
        Ok(())
    }

    pub(super) fn transform<T>(self, write: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
        let Self {
            temporary,
            path,
            parent,
            old,
        } = self;
        let (file, temporary_path) = temporary.into_parts();
        // 内层解密函数会原子替换暂存路径，因此先关闭临时文件句柄；TempPath
        // 始终负责清理失败产物，正式目标及目录的保护句柄则保持到最终发布。
        drop(file);
        let result = write(&temporary_path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(0x00200000)
            .open(&temporary_path)?;
        regular(&file)?;
        single_link(&file)?;
        let temporary = tempfile::NamedTempFile::from_parts(file, temporary_path);
        Self {
            temporary,
            path,
            parent,
            old,
        }
        .publish()?;
        Ok(result)
    }

    pub(super) fn publish(mut self) -> Result<()> {
        self.temporary.as_file().sync_all()?;
        ensure!(
            same_file::Handle::from_file(self.parent.try_clone()?)?
                == same_file::Handle::from_path(self.path.parent().unwrap())?,
            "发布前输出目录身份发生变化"
        );
        if let Some(old) = &self.old {
            single_link(old)?;
            ensure!(
                same_file::Handle::from_file(old.try_clone()?)?
                    == same_file::Handle::from_path(&self.path)?,
                "发布前输出文件身份发生变化"
            );
            // Windows 替换已有目标前必须关闭本次读取句柄。复核后立即发布，
            // 源文件与目录句柄继续存活；失败仍只丢弃副本，不删除旧目标。
            self.old.take();
            self.temporary.persist(&self.path)?;
        } else {
            // 原来不存在的目标不能覆盖并发创建的文件；失败由临时文件析构清理。
            self.temporary.persist_noclobber(&self.path)?;
        }
        Ok(())
    }
}
