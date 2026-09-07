//! 仅演示 Windows 文件共享语义，不声称是生产函数的调度级复现。
#[cfg(windows)]
fn replace(source: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(source: *const u16, target: *const u16, flags: u32) -> i32;
    }
    let source: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // 两个字符串都以 NUL 结尾，在调用返回前保持有效；1 为 REPLACE_EXISTING。
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 1) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn main() -> std::io::Result<()> {
    use std::{fs, os::windows::fs::OpenOptionsExt};
    let root = std::env::temp_dir().join(format!(
        "wx-publication-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root)?;
    let target = root.join("output.json");
    let staged = root.join("staged.json");
    let displaced = root.join("old.json");
    fs::write(&target, b"ALICE_SNAPSHOT")?;
    fs::write(&staged, b"ALICE_RESULT")?;
    // 与发布检查相同：允许读和删除共享，不允许写共享。
    let guard = fs::OpenOptions::new()
        .read(true)
        .share_mode(1 | 4)
        .open(&target)?;
    let direct = replace(&staged, &target);
    println!("replace_while_snapshot_open={direct:?}");
    if direct.is_ok() {
        drop(guard);
        fs::write(&target, b"ALICE_SNAPSHOT")?;
        fs::write(&staged, b"ALICE_RESULT")?;
    } else {
        drop(guard);
    }
    let guard = fs::OpenOptions::new().read(true).share_mode(1 | 4).open(&target)?;
    let rebound = fs::rename(&target, &displaced);
    println!("rename_snapshot_to_other_name={rebound:?}");
    if rebound.is_ok() {
        fs::write(&target, b"BOB_NEW_OUTPUT")?;
        // 旧句柄仍指向 ALICE；按路径替换针对的是后来出现的 BOB 文件。
        let publish = replace(&staged, &target);
        println!("replace_rebound_destination={publish:?}");
        println!("destination={}", String::from_utf8_lossy(&fs::read(&target)?));
        println!("snapshot={}", String::from_utf8_lossy(&fs::read(&displaced)?));
    }
    drop(guard);
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    panic!("Windows-only sharing probe");
}
