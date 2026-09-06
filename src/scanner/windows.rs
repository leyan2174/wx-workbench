/// Windows WeChat 进程内存密钥扫描器
///
/// 使用 Windows API：
/// - CreateToolhelp32Snapshot + Process32Next: 枚举进程找 Weixin.exe
/// - OpenProcess: 获取进程句柄（需要 PROCESS_VM_READ | PROCESS_QUERY_INFORMATION）
/// - VirtualQueryEx: 枚举内存区域
/// - ReadProcessMemory: 读取内存内容
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::Path;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};

use super::{collect_db_salts, KeyEntry};

mod config_cipher;
mod legacy;
mod version;

pub fn scan_keys(db_dir: &Path, process_name: &str) -> Result<Vec<KeyEntry>> {
    let db_salts = collect_db_salts(db_dir);
    let targets: BTreeSet<String> = db_salts.iter().map(|(_, name)| name.clone()).collect();
    eprintln!("找到 {} 个加密数据库", db_salts.len());

    let pids = find_wechat_pids(process_name);
    if pids.is_empty() {
        anyhow::bail!("找不到 {} 进程，请确认微信正在运行", process_name);
    }

    let (wechat_version, image_path) = detect_version(&pids)?;
    eprintln!(
        "检测到微信版本: {} ({})",
        wechat_version,
        image_path.display()
    );
    let config_cipher_version = version::WechatVersion {
        major: 4,
        minor: 1,
        build: 10,
        revision: 0,
    };
    if wechat_version < config_cipher_version {
        eprintln!(
            "按微信版本 {} 选择 legacy raw-key provider...",
            wechat_version
        );
        let process = open_process(pids[0])?;
        let found = legacy::scan(process, db_dir);
        unsafe {
            let _ = CloseHandle(process);
        }
        return found;
    }

    let mut entries = Vec::new();
    eprintln!(
        "按微信版本 {} 选择 Config.Cipher 只读扫描（{} 个进程）...",
        wechat_version,
        pids.len()
    );
    for pid in &pids {
        let Ok(process) = open_process(*pid) else {
            eprintln!("跳过 PID {}：无法打开进程", pid);
            continue;
        };
        let result = config_cipher::scan(process, db_dir);
        unsafe {
            let _ = CloseHandle(process);
        }
        match result {
            Ok(found) => merge_entries(
                &mut entries,
                found.into_iter().filter(|entry| targets.contains(&entry.db_name)).collect(),
            ),
            Err(error) => eprintln!("Config.Cipher 扫描 PID {} 失败: {error:#}", pid),
        }
        if missing_databases(&targets, &entries).is_empty() {
            break;
        }
    }
    let missing = missing_databases(&targets, &entries);
    if missing.is_empty() {
        eprintln!(
            "Config.Cipher 已完整验证 {}/{} 个数据库",
            entries.len(),
            targets.len()
        );
        return Ok(entries);
    }
    anyhow::bail!(
        "Config.Cipher 未完整验证数据库密钥：{}/{}；缺失：{}；未执行旧版回退，现有密钥文件未修改",
        targets.len() - missing.len(),
        targets.len(),
        missing.join(", ")
    )
}

fn missing_databases(targets: &BTreeSet<String>, entries: &[KeyEntry]) -> Vec<String> {
    let verified: BTreeSet<&str> = entries.iter().map(|entry| entry.db_name.as_str()).collect();
    targets.iter().filter(|name| !verified.contains(name.as_str())).cloned().collect()
}

fn detect_version(pids: &[u32]) -> Result<(version::WechatVersion, std::path::PathBuf)> {
    for pid in pids {
        let Ok(process) = open_process(*pid) else {
            continue;
        };
        let result = version::detect(process);
        unsafe {
            let _ = CloseHandle(process);
        }
        if let Ok(version) = result {
            return Ok(version);
        }
    }
    anyhow::bail!("无法读取 Weixin.exe 文件版本，已停止扫描")
}

fn find_wechat_pids(process_name: &str) -> Vec<u32> {
    let Some(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok() }) else {
        return Vec::new();
    };
    let mut pids = Vec::new();
    let mut entry = PROCESSENTRY32 {
        dwSize: std::mem::size_of::<PROCESSENTRY32>() as u32,
        ..Default::default()
    };
    unsafe {
        if Process32First(snap, &mut entry).is_ok() {
            loop {
                let name = std::ffi::CStr::from_ptr(entry.szExeFile.as_ptr()).to_string_lossy();
                if name.eq_ignore_ascii_case(process_name) {
                    pids.push(entry.th32ProcessID);
                }
                if Process32Next(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    pids
}

fn open_process(pid: u32) -> Result<HANDLE> {
    unsafe {
        OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, false, pid)
            .context("OpenProcess 失败，请以管理员权限运行")
    }
}

fn merge_entries(target: &mut Vec<KeyEntry>, found: Vec<KeyEntry>) {
    for entry in found {
        if !target
            .iter()
            .any(|existing| existing.db_name == entry.db_name)
        {
            target.push(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> KeyEntry {
        KeyEntry { db_name: name.into(), enc_key: String::new(), salt: String::new() }
    }

    #[test]
    fn extra_and_duplicate_keys_cannot_hide_missing_database() {
        let targets = BTreeSet::from(["message/a.db".into(), "message/b.db".into()]);
        let entries = vec![entry("message/a.db"), entry("message/a.db"), entry("migrate/old.db")];
        assert_eq!(missing_databases(&targets, &entries), vec!["message/b.db"]);
    }

    #[test]
    fn complete_keys_can_be_merged_across_processes() {
        let targets = BTreeSet::from(["message/a.db".into(), "message/b.db".into()]);
        let mut entries = vec![entry("message/b.db")];
        merge_entries(&mut entries, vec![entry("message/a.db"), entry("message/b.db")]);
        assert!(missing_databases(&targets, &entries).is_empty());
        assert_eq!(entries.len(), 2);
    }
}
