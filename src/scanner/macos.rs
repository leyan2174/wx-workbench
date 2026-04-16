/// macOS WeChat 进程内存密钥扫描器
///
/// 翻译自 find_all_keys_macos.c，使用 Mach VM API：
/// - task_for_pid: 获取目标进程的 task port（需要 root 权限）
/// - mach_vm_region: 枚举内存区域
/// - mach_vm_read: 读取内存块
///
/// 注意：
/// 1. 需要以 root (sudo) 运行
/// 2. WeChat 需要进行 ad-hoc 签名
/// 3. 在内存中搜索 x'<64hex><32hex>' 格式的 SQLCipher 密钥
use anyhow::{bail, Context, Result};
use std::path::Path;

use super::{collect_db_salts, KeyEntry};

// Mach 相关常量
const KERN_SUCCESS: i32 = 0;
const VM_PROT_READ: i32 = 1;
const VM_PROT_WRITE: i32 = 2;
const VM_REGION_BASIC_INFO_64: i32 = 9;
const CHUNK_SIZE: usize = 2 * 1024 * 1024; // 2MB
const HEX_PATTERN_LEN: usize = 96; // 64(key) + 32(salt)

// vm_region_basic_info_64 结构体
#[repr(C)]
struct VmRegionBasicInfo64 {
    protection: i32,
    max_protection: i32,
    inheritance: u32,
    shared: u32,
    reserved: u32,
    _offset: u64,
    behavior: i32,
    user_wired_count: u16,
}

// Mach FFI 声明
#[allow(non_camel_case_types)]
type kern_return_t = i32;
#[allow(non_camel_case_types)]
type mach_port_t = u32;
#[allow(non_camel_case_types)]
type mach_vm_address_t = u64;
#[allow(non_camel_case_types)]
type mach_vm_size_t = u64;
#[allow(non_camel_case_types)]
type mach_msg_type_number_t = u32;
#[allow(non_camel_case_types)]
type vm_offset_t = usize;
#[allow(non_camel_case_types, dead_code)]
type vm_prot_t = i32;

extern "C" {
    fn mach_task_self() -> mach_port_t;
    fn task_for_pid(host: mach_port_t, pid: libc::pid_t, task: *mut mach_port_t) -> kern_return_t;
    fn mach_vm_region(
        task: mach_port_t,
        address: *mut mach_vm_address_t,
        size: *mut mach_vm_size_t,
        flavor: i32,
        info: *mut VmRegionBasicInfo64,
        info_count: *mut mach_msg_type_number_t,
        obj_name: *mut mach_port_t,
    ) -> kern_return_t;
    fn mach_vm_read(
        task: mach_port_t,
        addr: mach_vm_address_t,
        size: mach_vm_size_t,
        data: *mut vm_offset_t,
        data_cnt: *mut mach_msg_type_number_t,
    ) -> kern_return_t;
    fn mach_vm_deallocate(
        task: mach_port_t,
        addr: mach_vm_address_t,
        size: mach_vm_size_t,
    ) -> kern_return_t;
}

/// 查找 WeChat 进程的 PID
fn find_wechat_pid() -> Option<libc::pid_t> {
    // 使用 pgrep -x WeChat 查找（与 C 版本一致）
    let output = std::process::Command::new("pgrep")
        .args(["-x", "WeChat"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout);
    s.trim().parse().ok()
}

/// 判断字节是否是 ASCII 十六进制字符
#[inline]
fn is_hex_char(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

pub fn scan_keys(db_dir: &Path) -> Result<Vec<KeyEntry>> {
    // 1. 查找 WeChat PID
    let pid = find_wechat_pid()
        .context("找不到 WeChat 进程，请确认 WeChat 正在运行")?;
    eprintln!("WeChat PID: {}", pid);

    // 2. 获取 task port
    // SAFETY: task_for_pid 是标准 Mach API，参数合法
    let task = unsafe {
        let mut task: mach_port_t = 0;
        let kr = task_for_pid(mach_task_self(), pid, &mut task);
        if kr != KERN_SUCCESS {
            bail!(
                "task_for_pid 失败 (kr={})\n请确认：(1) 以 root 运行 (2) WeChat 已 ad-hoc 签名",
                kr
            );
        }
        task
    };
    eprintln!("Got task port: {}", task);

    // 3. 收集数据库 salt 映射
    eprintln!("扫描数据库文件...");
    let db_salts = collect_db_salts(db_dir);
    eprintln!("找到 {} 个加密数据库", db_salts.len());

    // 4. 扫描进程内存
    eprintln!("扫描进程内存寻找密钥...");
    let raw_keys = scan_memory(task)?;
    eprintln!("找到 {} 个候选密钥", raw_keys.len());

    // 5. 将密钥与数据库 salt 匹配
    let mut entries = Vec::new();
    for (key_hex, salt_hex) in &raw_keys {
        for (db_salt, db_name) in &db_salts {
            if salt_hex == db_salt {
                entries.push(KeyEntry {
                    db_name: db_name.clone(),
                    enc_key: key_hex.clone(),
                    salt: salt_hex.clone(),
                });
                break;
            }
        }
    }

    eprintln!("匹配到 {}/{} 个密钥", entries.len(), raw_keys.len());
    Ok(entries)
}

/// 扫描进程内存，返回 (key_hex, salt_hex) 列表
fn scan_memory(task: mach_port_t) -> Result<Vec<(String, String)>> {
    let mut results: Vec<(String, String)> = Vec::new();
    let mut addr: mach_vm_address_t = 0;

    // VM_REGION_BASIC_INFO_COUNT_64 = 9（来自 <mach/vm_region.h>，固定值，不能用 sizeof 计算）
    let info_count_expected: mach_msg_type_number_t = 9;

    loop {
        let mut size: mach_vm_size_t = 0;
        let mut info = VmRegionBasicInfo64 {
            protection: 0, max_protection: 0, inheritance: 0,
            shared: 0, reserved: 0, _offset: 0, behavior: 0, user_wired_count: 0,
        };
        let mut info_count: mach_msg_type_number_t = info_count_expected;
        let mut obj_name: mach_port_t = 0;

        // SAFETY: mach_vm_region 枚举虚拟内存区域，所有参数合法
        let kr = unsafe {
            mach_vm_region(
                task,
                &mut addr,
                &mut size,
                VM_REGION_BASIC_INFO_64,
                &mut info,
                &mut info_count,
                &mut obj_name,
            )
        };

        if kr != KERN_SUCCESS {
            break;
        }
        if size == 0 {
            addr = addr.saturating_add(1);
            continue;
        }

        // 只扫描可读可写区域（密钥通常存在于堆内存）
        if (info.protection & (VM_PROT_READ | VM_PROT_WRITE)) == (VM_PROT_READ | VM_PROT_WRITE) {
            scan_region(task, addr, size, &mut results);
        }

        addr = addr.saturating_add(size);
    }

    Ok(results)
}

/// 扫描单个内存区域，按 CHUNK_SIZE 分块读取
fn scan_region(
    task: mach_port_t,
    addr: mach_vm_address_t,
    size: mach_vm_size_t,
    results: &mut Vec<(String, String)>,
) {
    let end = addr + size;
    let mut ca = addr;

    while ca < end {
        let cs = std::cmp::min(end - ca, CHUNK_SIZE as u64);

        let mut data: vm_offset_t = 0;
        let mut dc: mach_msg_type_number_t = 0;

        // SAFETY: mach_vm_read 读取目标进程内存到内核缓冲区，
        // 返回的 data 指针指向通过 vm_allocate 分配的内存，
        // 必须用 mach_vm_deallocate 释放
        let kr = unsafe {
            mach_vm_read(task, ca, cs, &mut data, &mut dc)
        };

        if kr == KERN_SUCCESS {
            // SAFETY: data 是 mach_vm_read 返回的有效指针，dc 是字节数
            let buf: &[u8] = unsafe {
                std::slice::from_raw_parts(data as *const u8, dc as usize)
            };

            search_pattern(buf, results);

            // SAFETY: 释放 mach_vm_read 分配的内核内存
            unsafe {
                mach_vm_deallocate(mach_task_self(), data as u64, dc as u64);
            }
        }

        // 保留 (HEX_PATTERN_LEN + 3) 字节重叠以处理跨块边界的模式
        let overlap = HEX_PATTERN_LEN + 3;
        if cs as usize > overlap {
            ca += cs - overlap as u64;
        } else {
            ca += cs;
        }
    }
}

/// 在缓冲区中搜索 x'<96个十六进制字符>' 模式
///
/// 格式：x'<64hex(key)><32hex(salt)>'（总计 99 字节）
fn search_pattern(buf: &[u8], results: &mut Vec<(String, String)>) {
    let total = HEX_PATTERN_LEN + 3; // x' + 96 hex + '
    if buf.len() < total {
        return;
    }

    let mut i = 0;
    while i + total <= buf.len() {
        if buf[i] != b'x' || buf[i + 1] != b'\'' {
            i += 1;
            continue;
        }

        // 验证后续 96 字节都是十六进制字符
        let hex_start = i + 2;
        let all_hex = buf[hex_start..hex_start + HEX_PATTERN_LEN]
            .iter()
            .all(|&c| is_hex_char(c));

        if !all_hex {
            i += 1;
            continue;
        }

        // 验证结尾的单引号
        if buf[hex_start + HEX_PATTERN_LEN] != b'\'' {
            i += 1;
            continue;
        }

        // 提取 key_hex 和 salt_hex，统一转小写
        let key_hex = String::from_utf8_lossy(&buf[hex_start..hex_start + 64])
            .to_lowercase();
        let salt_hex = String::from_utf8_lossy(&buf[hex_start + 64..hex_start + 96])
            .to_lowercase();

        // 去重检查
        let is_dup = results.iter().any(|(k, s)| k == &key_hex && s == &salt_hex);
        if !is_dup {
            results.push((key_hex, salt_hex));
        }

        i += total;
    }
}
