//! Optional account-key capture and DPAPI storage. See docs/account-key-provider.md.
use super::super::{collect_db_salts, KeyEntry};
use super::config_cipher::{collect_db_pages, verify_page1, DbPage};
use anyhow::{bail, ensure, Context, Result};
use frida::{DeviceManager, Frida, Message, ScriptHandler, ScriptOption, SpawnOptions};
use pbkdf2::pbkdf2_hmac;
use serde::{Deserialize, Serialize};
use sha2::Sha512;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};
use zeroize::{Zeroize, Zeroizing};

enum Event {
    Ready,
    Candidate(Zeroizing<Vec<u8>>),
    Error,
}
static EVENTS: Mutex<Option<mpsc::SyncSender<Event>>> = Mutex::new(None);

// A zero-sized handler also works with older frida-rust callback layouts.
struct Handler;
impl ScriptHandler for Handler {
    fn on_message(&mut self, message: Message, data: Option<Vec<u8>>) {
        let data = data.map(Zeroizing::new);
        let event = match message {
            Message::Send(message) if message.payload.r#type == "wx-account" => {
                match message.payload.result.as_str() {
                    "ready" => Some(Event::Ready),
                    "candidate" => data.filter(|key| key.len() == 32).map(Event::Candidate),
                    "error" => Some(Event::Error),
                    _ => None,
                }
            }
            Message::Error(_) => Some(Event::Error),
            _ => None,
        };
        if let Some(event) = event {
            if let Ok(sender) = EVENTS.lock() {
                if let Some(sender) = sender.as_ref() {
                    let _ = sender.try_send(event);
                }
            }
        }
    }
}

struct ChannelGuard;
impl Drop for ChannelGuard {
    fn drop(&mut self) {
        if let Ok(mut slot) = EVENTS.lock() {
            *slot = None;
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SavedKey {
    version: u32,
    db_dir: String,
    key: Vec<u8>,
}
impl Drop for SavedKey {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

fn binding(db_dir: &Path) -> Result<String> {
    Ok(db_dir.canonicalize()?.to_string_lossy().to_lowercase())
}

fn protect(data: &[u8], decrypt: bool) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(data.len() <= 65536, "账号密钥文件长度无效");
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        if decrypt {
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
        }
        .context("Windows DPAPI 处理失败，请使用保存密钥时的 Windows 用户")?;
        let slice = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
        let result = Zeroizing::new(slice.to_vec());
        slice.zeroize();
        let _ = LocalFree(HLOCAL(output.pbData as *mut _));
        Ok(result)
    }
}

fn save(db_dir: &Path, file: &Path, key: &[u8]) -> Result<()> {
    use std::io::Write;
    let record = SavedKey {
        version: 1,
        db_dir: binding(db_dir)?,
        key: key.to_vec(),
    };
    let plain = Zeroizing::new(serde_json::to_vec(&record)?);
    let encrypted = protect(&plain, false)?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let suffix = format!(
        "{}.{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let temporary = file.with_extension(format!("dpapi.{suffix}.tmp"));
    let result = (|| -> Result<()> {
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        output.write_all(&encrypted)?;
        output.sync_all()?;
        drop(output);
        if file.exists() {
            std::fs::copy(file, file.with_extension(format!("dpapi.{suffix}.bak")))?;
        }
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let src: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let dst: Vec<u16> = file.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            MoveFileExW(
                PCWSTR(src.as_ptr()),
                PCWSTR(dst.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn derive(raw: &[u8], page: &[u8]) -> Option<Zeroizing<[u8; 32]>> {
    if raw.len() != 32 || page.len() != 4096 {
        return None;
    }
    let mut key = Zeroizing::new([0u8; 32]);
    pbkdf2_hmac::<Sha512>(raw, &page[..16], 256_000, &mut *key);
    verify_page1(&key, page).then_some(key)
}

fn verified_entries(
    raw: &[u8],
    pages: &[DbPage],
    targets: &BTreeSet<String>,
) -> Result<Vec<KeyEntry>> {
    ensure!(!targets.is_empty(), "没有可验证的加密数据库");
    let mut entries = Vec::new();
    for page in pages.iter().filter(|page| targets.contains(&page.db_name)) {
        let verified = page
            .page1_variants
            .iter()
            .find_map(|data| derive(raw, data).map(|key| (key, data)));
        if let Some((key, data)) = verified {
            entries.push(KeyEntry {
                db_name: page.db_name.clone(),
                enc_key: hex(&*key),
                salt: hex(&data[..16]),
            });
        }
    }
    let missing = super::missing_databases(targets, &entries);
    ensure!(
        missing.is_empty(),
        "账号密钥未覆盖全部目标数据库；缺失：{}；现有逐库密钥文件未修改",
        missing.join(", ")
    );
    Ok(entries)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|value| format!("{value:02x}")).collect()
}

pub(crate) fn derive_saved(db_dir: &Path, file: &Path) -> Result<Vec<KeyEntry>> {
    ensure!(std::fs::metadata(file)?.len() <= 65536, "账号密钥文件过大");
    let encrypted = std::fs::read(file)?;
    let plain = protect(&encrypted, true)?;
    let record: SavedKey = serde_json::from_slice(&plain).context("账号密钥记录无效")?;
    ensure!(
        record.version == 1 && record.key.len() == 32,
        "账号密钥记录版本或长度无效"
    );
    ensure!(
        record.db_dir == binding(db_dir)?,
        "账号密钥属于不同的数据库目录"
    );
    let targets = collect_db_salts(db_dir)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    verified_entries(&record.key, &collect_db_pages(db_dir)?, &targets)
}

fn executable(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        ensure!(
            path.file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("Weixin.exe")),
            "请指定 Weixin.exe"
        );
        ensure!(path.is_file(), "Weixin.exe 不存在");
        return Ok(path.canonicalize()?);
    }
    let pids = super::find_wechat_pids("Weixin.exe");
    if !pids.is_empty() {
        return Ok(super::detect_version(&pids)?.1);
    }
    for base in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(base) {
            let path = PathBuf::from(base).join("Tencent/Weixin/Weixin.exe");
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    bail!("无法找到 Weixin.exe，请使用 --wechat-exe 指定")
}

pub(crate) fn capture_and_save(
    db_dir: &Path,
    override_path: Option<&Path>,
    timeout: u64,
    file: &Path,
) -> Result<Vec<KeyEntry>> {
    let pages = collect_db_pages(db_dir)?;
    let target = pages
        .iter()
        .find(|page| page.db_name == "message/message_0.db")
        .context("账号目录缺少可验证的 message/message_0.db")?;
    let exe = executable(override_path)?;
    let frida = unsafe { Frida::obtain() };
    let manager = DeviceManager::obtain(&frida);
    let mut device = manager.get_local_device().context("无法初始化本机 Frida")?;
    let (sender, receiver) = mpsc::sync_channel(128);
    {
        let mut slot = EVENTS
            .lock()
            .map_err(|_| anyhow::anyhow!("捕获通道不可用"))?;
        ensure!(slot.is_none(), "已有账号密钥捕获正在进行");
        *slot = Some(sender);
    }
    let _channel_guard = ChannelGuard;
    let pids = super::find_wechat_pids("Weixin.exe");
    for &pid in &pids {
        let process = super::open_process(pid)?;
        let path = super::version::detect(process);
        unsafe {
            let _ = CloseHandle(process);
        }
        let (_, path) = path?;
        ensure!(
            binding(&path)? == binding(&exe)?,
            "发现其他安装目录的微信进程，请先关闭后重试"
        );
    }
    eprintln!("将重启微信以捕获账号密钥，请在启动后扫码或用手机确认登录。");
    for pid in pids {
        if let Err(error) = device.kill(pid) {
            if super::find_wechat_pids("Weixin.exe").contains(&pid) {
                bail!("关闭微信失败: {error}");
            }
        }
    }
    let pid = device
        .spawn(exe.to_string_lossy(), &SpawnOptions::new())
        .context("启动微信失败")?;
    let session = match device.attach(pid) {
        Ok(session) => session,
        Err(error) => {
            let _ = device.resume(pid);
            return Err(anyhow::anyhow!("附加 Frida 失败: {error}"));
        }
    };
    let result = (|| -> Result<Zeroizing<Vec<u8>>> {
        let mut options = ScriptOption::new();
        let mut script = session.create_script(include_str!("account_hook.js"), &mut options)?;
        script.handle_message(Handler)?;
        script.load()?;
        let result = (|| -> Result<Zeroizing<Vec<u8>>> {
            device.resume(pid)?;
            let deadline = Instant::now() + Duration::from_secs(timeout);
            let mut report = Instant::now();
            let mut candidates = 0;
            while Instant::now() < deadline {
                if session.is_detached() {
                    bail!("微信进程已退出或 Frida 已断开，捕获未完成");
                }
                match receiver.recv_timeout(Duration::from_millis(200)) {
                    Ok(Event::Ready) => eprintln!("SHA-512 捕获已就绪，等待登录及数据库密钥验证"),
                    Ok(Event::Candidate(key)) => {
                        candidates += 1;
                        ensure!(
                            candidates <= 128,
                            "候选过多，已停止捕获，请检查微信版本兼容性"
                        );
                        if target
                            .page1_variants
                            .iter()
                            .any(|page| derive(&key, page).is_some())
                        {
                            return Ok(key);
                        }
                    }
                    Ok(Event::Error) => {
                        bail!("SHA-512 Hook 初始化或执行失败；该微信构建可能不兼容")
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => bail!("捕获通道已关闭"),
                    Err(mpsc::RecvTimeoutError::Timeout) => (),
                }
                if report.elapsed() >= Duration::from_secs(30) {
                    eprintln!("仍在等待有效账号密钥，请确认微信已登录目标账号");
                    report = Instant::now();
                }
            }
            bail!("等待登录和账号密钥超时；现有密钥文件未修改")
        })();
        let _ = script.unload();
        result
    })();
    let _ = session.detach();
    // Also release a spawn left suspended if script creation/loading failed.
    let _ = device.resume(pid);
    let raw = result?;
    eprintln!("账号密钥候选已验证，正在逐库派生并检查完整覆盖");
    let targets = collect_db_salts(db_dir)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    let entries = verified_entries(&raw, &collect_db_pages(db_dir)?, &targets)?;
    save(db_dir, file, &raw)?;
    eprintln!("账号密钥已使用 Windows DPAPI 加密保存: {}", file.display());
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::{Hmac, Mac};

    fn page(raw: &[u8], salt: u8) -> Vec<u8> {
        let mut data = vec![salt; 4096];
        let mut enc = [0; 32];
        pbkdf2_hmac::<Sha512>(raw, &data[..16], 256_000, &mut enc);
        let mut mac_key = [0; 32];
        pbkdf2_hmac::<Sha512>(&enc, &[salt ^ 0x3a; 16], 2, &mut mac_key);
        let mut mac = Hmac::<Sha512>::new_from_slice(&mac_key).unwrap();
        mac.update(&data[16..4032]);
        mac.update(&1u32.to_le_bytes());
        data[4032..].copy_from_slice(&mac.finalize().into_bytes());
        data
    }

    #[test]
    fn dpapi_roundtrip_and_tamper_rejection() {
        let raw = [0x27; 32];
        let encrypted = protect(&raw, false).unwrap();
        assert_ne!(&encrypted[..], &raw);
        assert_eq!(&protect(&encrypted, true).unwrap()[..], &raw);
        let mut corrupt = encrypted.to_vec();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert!(protect(&corrupt, true).is_err());
    }

    #[test]
    fn account_key_verifies_different_salts_and_rejects_wrong_key() {
        let raw = [0x42; 32];
        let pages = vec![
            DbPage {
                db_name: "a.db".into(),
                page1_variants: vec![page(&raw, 1)],
            },
            DbPage {
                db_name: "b.db".into(),
                page1_variants: vec![page(&raw, 2)],
            },
        ];
        let targets = BTreeSet::from(["a.db".into(), "b.db".into()]);
        let entries = verified_entries(&raw, &pages, &targets).unwrap();
        assert_ne!(entries[0].enc_key, entries[1].enc_key);
        assert!(verified_entries(&[0x43; 32], &pages, &targets).is_err());
        assert!(verified_entries(&raw, &pages[..1], &targets).is_err());
    }

    #[test]
    fn saved_key_handles_new_shards_and_rejects_other_account_directory() {
        let root = std::env::temp_dir().join(format!(
            "wx-account-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let db = root.join("account/db_storage");
        let other = root.join("other/db_storage");
        std::fs::create_dir_all(db.join("message")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let file = root.join("account_key.dpapi");
        let raw = [0x42; 32];
        std::fs::write(db.join("message/message_0.db"), page(&raw, 1)).unwrap();
        save(&db, &file, &raw).unwrap();
        assert_eq!(derive_saved(&db, &file).unwrap().len(), 1);
        std::fs::write(db.join("message/message_1.db"), page(&raw, 2)).unwrap();
        assert_eq!(derive_saved(&db, &file).unwrap().len(), 2);
        assert!(derive_saved(&other, &file).is_err());
        save(&db, &file, &raw).unwrap();
        assert_eq!(
            std::fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "bak"))
                .count(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "Explicit native Frida integration test; starts a hidden temporary PowerShell process"]
    fn frida_binary_message_smoke() {
        use std::os::windows::process::CommandExt;
        let mut child = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        let result = (|| -> Result<()> {
            let frida = unsafe { Frida::obtain() };
            let manager = DeviceManager::obtain(&frida);
            let device = manager.get_local_device()?;
            let session = device.attach(child.id())?;
            let (tx, rx) = mpsc::sync_channel(8);
            *EVENTS.lock().unwrap() = Some(tx);
            let _guard = ChannelGuard;
            let mut options = ScriptOption::new();
            let script_source = format!("{}\nsend({{type:'wx-account',id:0,result:'candidate',returns:null}},new Uint8Array(32).fill(42).buffer);", include_str!("account_hook.js"));
            let mut script = session.create_script(&script_source, &mut options)?;
            script.handle_message(Handler)?;
            script.load()?;
            let event = rx.recv_timeout(Duration::from_secs(10))?;
            let valid = matches!(event, Event::Candidate(key) if key.iter().all(|b| *b == 42));
            script.unload()?;
            session.detach()?;
            ensure!(valid, "Frida binary transport returned an unexpected event");
            Ok(())
        })();
        let _ = child.kill();
        let _ = child.wait();
        result.unwrap();
    }
}
