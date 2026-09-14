//! Short-lived Configure/per-job pin. Never retain in global service state.
use crate::{attachment::native_image::HostOutputGuard, runtime::RuntimeContext};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::Path,
};
use zeroize::Zeroizing;

pub struct ConfigPin {
    file: Option<File>,
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

fn immutable(bytes: &[u8]) -> Result<Value> {
    let mut value: Value = serde_json::from_slice(bytes)?;
    let fields = value.as_object_mut().context("配置必须为对象")?;
    // 删除前清零原材料，比较结果中不保留图片密钥。
    if let Some(Value::String(mut secret)) = fields.remove("image_aes_key") {
        use zeroize::Zeroize;
        secret.zeroize();
    }
    fields.remove("image_xor_key");
    Ok(value)
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
    Ok(())
}

fn fingerprint_bytes(bytes: &[u8]) -> Result<String> {
    use sha2::{Digest, Sha256};
    let canonical = Zeroizing::new(serde_json::to_vec(&immutable(bytes)?)?);
    Ok(format!("{:x}", Sha256::digest(canonical.as_slice())))
}

impl ConfigPin {
    /// Stable identity across image-key rotation; includes all other config fields.
    pub fn fingerprint(&self) -> Result<String> {
        fingerprint_bytes(&self.baseline)
    }

    pub fn new(runtime: &RuntimeContext) -> Result<Self> {
        let parent = HostOutputGuard::new(runtime.config_path.parent().context("配置缺少父目录")?)?;
        let file = open(&runtime.config_path)?;
        identity(runtime)?;
        let baseline = read(&file)?;
        immutable(&baseline)?;
        parent.verify()?;
        Ok(Self {
            file: Some(file),
            baseline,
            parent,
        })
    }

    pub fn release_for_image_key(&mut self, runtime: &RuntimeContext) -> Result<()> {
        ensure!(self.file.is_some(), "配置锁已被释放");
        self.parent.verify()?;
        identity(runtime)?;
        ensure!(
            immutable(&read(self.file.as_ref().unwrap())?)? == immutable(&self.baseline)?,
            "非图片配置发生变化"
        );
        self.file.take();
        Ok(())
    }

    pub fn repin(&mut self, runtime: &RuntimeContext) -> Result<()> {
        ensure!(self.file.is_none(), "配置锁未被释放");
        let file = open(&runtime.config_path)?;
        // 先恢复锁，再做任何检查。检查失败也持有新文件，调用者必须停止服务。
        self.file = Some(file);
        self.parent.verify()?;
        identity(runtime)?;
        let current = read(self.file.as_ref().unwrap())?;
        ensure!(
            immutable(&current)? == immutable(&self.baseline)?,
            "配置变化超出图片字段授权范围"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fingerprint_ignores_only_image_keys() -> Result<()> {
        let a = br#"{"db_dir":"a","image_aes_key":"synthetic","image_xor_key":1,"nested":{"x":1}}"#;
        let b = br#"{"nested":{"x":1},"image_xor_key":2,"image_aes_key":"rotated","db_dir":"a"}"#;
        assert_eq!(fingerprint_bytes(a)?, fingerprint_bytes(b)?);
        assert_eq!(fingerprint_bytes(a)?.len(), 64);
        for changed in [
            br#"{"db_dir":"b","nested":{"x":1}}"#.as_slice(),
            br#"{"db_dir":"a","nested":{"x":2}}"#.as_slice(),
            br#"{"db_dir":"a","nested":{"x":1},"extra":true}"#.as_slice(),
        ] {
            assert_ne!(fingerprint_bytes(a)?, fingerprint_bytes(changed)?);
        }
        assert!(fingerprint_bytes(b"[]").is_err());
        Ok(())
    }
    #[test]
    fn fixed_runtime_identity_and_repin_are_enforced() -> Result<()> {
        let root = tempfile::tempdir()?;
        let config_path = root.path().join("config.json");
        let config = crate::config::Config {
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
        let mut pin = ConfigPin::new(&runtime)?;
        let baseline = pin.fingerprint()?;
        pin.release_for_image_key(&runtime)?;
        let mut value = serde_json::to_value(&config)?;
        value["image_aes_key"] = Value::String("synthetic".into());
        std::fs::write(&config_path, serde_json::to_vec(&value)?)?;
        pin.repin(&runtime)?;
        assert_eq!(baseline, pin.fingerprint()?);
        pin.release_for_image_key(&runtime)?;
        value["wechat_process"] = Value::String("Other.exe".into());
        std::fs::write(&config_path, serde_json::to_vec(&value)?)?;
        assert!(pin.repin(&runtime).is_err());
        drop(pin);
        assert!(ConfigPin::new(&runtime).is_err());
        Ok(())
    }
}
