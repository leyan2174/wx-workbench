//! Short-lived Configure/per-job pin. Never retain in global service state.
use crate::{attachment::local_files::HostOutputGuard, runtime::RuntimeContext};
use anyhow::{ensure, Context, Result};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::Path,
};
use zeroize::Zeroizing;

pub struct ConfigPin {
    file: File,
    baseline: Zeroizing<Vec<u8>>,
    parent: HostOutputGuard,
}

fn open(path: &Path) -> Result<File> {
    use std::os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
    };
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(path)?;
    ensure!(
        file.metadata()?.file_attributes() & (0x400 | 0x10) == 0,
        "配置不是普通文件"
    );
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?;
    }
    ensure!(info.nNumberOfLinks == 1, "配置不能具有多个硬链接");
    Ok(file)
}

fn read(mut file: &File) -> Result<Zeroizing<Vec<u8>>> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 4 * 1024 * 1024, "配置超过读取限额");
    Ok(bytes)
}

fn normalized(path: &Path) -> Result<String> {
    let path = path.canonicalize().or_else(|_| std::path::absolute(path))?;
    Ok(path
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_lowercase())
}

fn identity(runtime: &RuntimeContext) -> Result<()> {
    let current = crate::config::load_config_at(&runtime.config_path)?;
    for (a, b) in [
        (&current.db_dir, &runtime.config.db_dir),
        (&current.keys_file, &runtime.config.keys_file),
        (&current.decrypted_dir, &runtime.config.decrypted_dir),
    ] {
        ensure!(normalized(a)? == normalized(b)?, "账号配置身份发生变化");
    }
    ensure!(
        current.wechat_process == runtime.config.wechat_process,
        "账号进程配置发生变化"
    );
    let current_store = current.key_store.as_deref().map(normalized).transpose()?;
    let pinned_store = runtime
        .config
        .key_store
        .as_deref()
        .map(normalized)
        .transpose()?;
    ensure!(current_store == pinned_store, "账号密钥存储引用发生变化");
    Ok(())
}

fn fingerprint_bytes(bytes: &[u8]) -> Result<String> {
    use sha2::{Digest, Sha256};
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

impl ConfigPin {
    pub fn file_identity(&self) -> Result<(u32, u64)> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::{
            Foundation::HANDLE,
            Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
        };
        let file = &self.file;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe {
            GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?;
        }
        Ok((
            info.dwVolumeSerialNumber,
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        ))
    }

    pub fn verify(&self, runtime: &RuntimeContext) -> Result<()> {
        self.parent.verify()?;
        identity(runtime)?;
        let file = &self.file;
        ensure!(
            same_file::Handle::from_file(file.try_clone()?)?
                == same_file::Handle::from_path(&runtime.config_path)?,
            "Configuration file identity changed"
        );
        ensure!(
            read(file)?.as_slice() == self.baseline.as_slice(),
            "Pinned configuration content changed"
        );
        Ok(())
    }

    /// Key rotation changes the encrypted store, never these configuration bytes.
    pub fn fingerprint(&self) -> Result<String> {
        fingerprint_bytes(&self.baseline)
    }

    pub fn new(runtime: &RuntimeContext) -> Result<Self> {
        let parent = HostOutputGuard::new(runtime.config_path.parent().context("配置缺少父目录")?)?;
        let file = open(&runtime.config_path)?;
        identity(runtime)?;
        let baseline = read(&file)?;
        parent.verify()?;
        Ok(Self {
            file,
            baseline,
            parent,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn fingerprint_includes_legacy_image_fields() -> Result<()> {
        let a = br#"{"db_dir":"a","image_aes_key":"synthetic","image_xor_key":1,"nested":{"x":1}}"#;
        let b = br#"{"nested":{"x":1},"image_xor_key":2,"image_aes_key":"rotated","db_dir":"a"}"#;
        assert_ne!(fingerprint_bytes(a)?, fingerprint_bytes(b)?);
        assert_eq!(fingerprint_bytes(a)?.len(), 64);
        for changed in [
            br#"{"db_dir":"b","nested":{"x":1}}"#.as_slice(),
            br#"{"db_dir":"a","nested":{"x":2}}"#.as_slice(),
            br#"{"db_dir":"a","nested":{"x":1},"extra":true}"#.as_slice(),
        ] {
            assert_ne!(fingerprint_bytes(a)?, fingerprint_bytes(changed)?);
        }
        Ok(())
    }
    #[test]
    fn fixed_runtime_identity_and_operation_lock_are_enforced() -> Result<()> {
        let root = tempfile::tempdir()?;
        let config_path = root.path().join("config.json");
        let config = crate::config::Config {
            key_store: None,
            db_dir: root.path().join("account/db_storage"),
            keys_file: root.path().join("keys.json"),
            decrypted_dir: root.path().join("decrypted"),
            wechat_process: "SyntheticNeverLaunched.exe".into(),
        };
        std::fs::create_dir_all(&config.db_dir)?;
        std::fs::write(&config_path, serde_json::to_vec(&config)?)?;
        let runtime = RuntimeContext::from_config(
            config_path.clone(),
            config.clone(),
            root.path().join("runtime"),
        )?;
        let pin = ConfigPin::new(&runtime)?;
        pin.verify(&runtime)?;
        assert!(OpenOptions::new().write(true).open(&config_path).is_err());
        drop(pin);
        let mut value = serde_json::to_value(&config)?;
        value["wechat_process"] = Value::String("Other.exe".into());
        std::fs::write(&config_path, serde_json::to_vec(&value)?)?;
        assert!(ConfigPin::new(&runtime).is_err());
        Ok(())
    }
}
