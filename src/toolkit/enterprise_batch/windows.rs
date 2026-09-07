//! Windows 只读适配器；main 也可通过 cmd_with_scanner 注入 scanner 的通用实现。
//! 所需 Win32 features 已存在：Foundation、Threading、Memory、Diagnostics_Debug/ToolHelp。
use super::scan::{MemoryRegion, ProcessMemory, ProcessScanner};
use anyhow::{ensure, Context, Result};
use std::{ffi::{c_void, CStr}, mem::size_of};
use windows::Win32::{
    Foundation::{CloseHandle, BOOL, HANDLE},
    System::{
        Diagnostics::{
            Debug::ReadProcessMemory,
            ToolHelp::{CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32, TH32CS_SNAPPROCESS},
        },
        Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS},
        Threading::{IsWow64Process, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
    },
};

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) { unsafe { let _ = CloseHandle(self.0); } }
}

pub struct WindowsScanner;
struct WindowsMemory { handle: OwnedHandle }

impl ProcessScanner for WindowsScanner {
    fn wxwork_pids(&self) -> Result<Vec<u32>> {
        let snapshot = OwnedHandle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.context("无法枚举企业微信进程")?);
        let mut entry = PROCESSENTRY32 { dwSize: size_of::<PROCESSENTRY32>() as u32, ..Default::default() };
        let mut pids = Vec::new();
        unsafe {
            if Process32First(snapshot.0, &mut entry).is_ok() {
                loop {
                    let name = CStr::from_ptr(entry.szExeFile.as_ptr()).to_bytes();
                    if name.eq_ignore_ascii_case(b"WXWork.exe") { pids.push(entry.th32ProcessID); }
                    if Process32Next(snapshot.0, &mut entry).is_err() { break; }
                }
            }
        }
        pids.sort_unstable();
        pids.dedup();
        Ok(pids)
    }

    fn open(&self, pid: u32) -> Result<Box<dyn ProcessMemory>> {
        let handle = OwnedHandle(unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) }.context("无法只读打开企业微信进程")?);
        // PID 可能在枚举后被复用，打开句柄后再核对映像名称。
        let mut path = vec![0u16; 32768];
        let mut length = path.len() as u32;
        unsafe { QueryFullProcessImageNameW(handle.0, PROCESS_NAME_WIN32, windows::core::PWSTR(path.as_mut_ptr()), &mut length) }
            .context("无法确认进程映像")?;
        let path = String::from_utf16(&path[..length as usize]).context("进程映像路径不可识别")?;
        ensure!(path.rsplit(['\\', '/']).next().is_some_and(|name| name.eq_ignore_ascii_case("WXWork.exe")), "进程不再是 WXWork.exe");
        Ok(Box::new(WindowsMemory { handle }))
    }
}

impl ProcessMemory for WindowsMemory {
    fn pointer_width(&self) -> Result<u8> {
        // 本仓库宿主固定为 Windows x64 MSVC；WOW64 表示目标使用 32 位指针。
        ensure!(size_of::<usize>() == 8, "企业微信扫描宿主必须为 64 位");
        let mut wow64 = BOOL::default();
        unsafe { IsWow64Process(self.handle.0, &mut wow64) }.context("无法判断企业微信进程位数")?;
        Ok(if wow64.as_bool() { 4 } else { 8 })
    }

    fn regions(&self) -> Result<Vec<MemoryRegion>> {
        let mut regions = Vec::new();
        let mut address = 0u64;
        while address < 0x0000_8000_0000_0000 {
            let mut information = MEMORY_BASIC_INFORMATION::default();
            let size = unsafe { VirtualQueryEx(self.handle.0, Some(address as usize as *const c_void), &mut information, size_of::<MEMORY_BASIC_INFORMATION>()) };
            if size == 0 { break; }
            let base = information.BaseAddress as usize as u64;
            let length = information.RegionSize as u64;
            let Some(next) = base.checked_add(length).filter(|next| *next > address) else { break; };
            let protection = information.Protect.0;
            if information.State == MEM_COMMIT && length != 0
                && protection & (PAGE_GUARD.0 | PAGE_NOACCESS.0) == 0
                && matches!(protection & 0xff, 0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80)
            { regions.push(MemoryRegion { base, size: length }); }
            address = next;
        }
        Ok(regions)
    }

    fn read(&self, address: u64, buffer: &mut [u8]) -> Result<usize> {
        ensure!(address.checked_add(buffer.len() as u64).is_some(), "内存读取地址溢出");
        let mut read = 0usize;
        unsafe { ReadProcessMemory(self.handle.0, address as usize as *const c_void, buffer.as_mut_ptr() as *mut c_void, buffer.len(), Some(&mut read)) }
            .context("读取企业微信内存失败")?;
        ensure!(read <= buffer.len(), "内存读取长度无效");
        Ok(read)
    }
}
