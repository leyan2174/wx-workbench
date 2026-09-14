//! 当前账号的数据库主文件快照导出，不合并实时 WAL。

use super::{files::*, Failure, Report};
use crate::{attachment::local_files::HostOutputGuard, runtime::RuntimeContext};
use anyhow::{ensure, Result};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 独立原生命令：缺密钥和逐项失败均返回非零退出码。
    Strict,
    /// 旧一键入口：缺密钥算跳过，逐项失败仍完成批次并报告计数。
    Legacy,
}

pub(super) fn database_key(value: &str) -> Result<[u8; 32]> {
    let mut key = [0; 32];
    let mut count = 0;
    // 兼容 Python bytes.fromhex：空白只能位于完整字节之间，不能拆开半字节。
    for part in value.split(|c| matches!(c, ' ' | '\t'..='\r')) {
        ensure!(
            part.len() % 2 == 0 && part.bytes().all(|b| b.is_ascii_hexdigit()),
            "数据库密钥格式错误"
        );
        for pair in part.as_bytes().chunks_exact(2) {
            ensure!(count < key.len(), "数据库密钥长度错误");
            key[count] = ((pair[0] as char).to_digit(16).unwrap() * 16
                + (pair[1] as char).to_digit(16).unwrap()) as u8;
            count += 1;
        }
    }
    ensure!(count == key.len(), "数据库密钥长度错误");
    Ok(key)
}

pub fn decrypt(
    runtime: &RuntimeContext,
    keys: &HashMap<String, String>,
    incremental: bool,
    dry_run: bool,
    mode: Mode,
) -> Result<()> {
    let cfg = &runtime.config;
    separate(&cfg.db_dir, &cfg.decrypted_dir)?;
    separate(&runtime.directory, &cfg.decrypted_dir)?;
    let mut normalized = HashMap::new();
    for (name, value) in keys {
        ensure!(
            normalized
                .insert(name.replace('\\', "/").to_lowercase(), value.as_str())
                .is_none(),
            "数据库密钥存在规范化重名路径"
        );
    }
    let mut sources = collect(&cfg.db_dir, "db", mode == Mode::Strict)?
        .into_iter()
        .map(|path| Ok((fs::metadata(&path)?.len(), path)))
        .collect::<Result<Vec<_>>>()?;
    if mode == Mode::Legacy {
        // 与旧批次一致先处理小库；同大小保留目录遍历的确定性顺序。
        sources.sort_by_key(|(bytes, _)| *bytes);
    }
    let mut report = Report::default();
    let mut missing_keys = 0;
    let mut unchanged = 0;
    let mut written_bytes = 0u64;
    let source_count = sources.len();
    for (bytes, source) in sources {
        report.total += 1;
        let rel = source.strip_prefix(&cfg.db_dir)?;
        let name = rel.to_string_lossy().replace('\\', "/").to_lowercase();
        let output = cfg.decrypted_dir.join(rel);
        let result = (|| -> Result<&str> {
            validate_target(runtime, &output)?;
            let Some(raw_key) = normalized.get(&name) else {
                ensure!(mode == Mode::Legacy, "数据库没有对应密钥");
                return Ok("missing");
            };
            // 未变更及预览路径不解析或校验密钥，也不打开数据库正文。
            if incremental
                && output.exists()
                && fs::metadata(&source)?.modified()? <= fs::metadata(&output)?.modified()?
            {
                return Ok("skipped");
            }
            if dry_run {
                return Ok("planned");
            }
            let key = zeroize::Zeroizing::new(database_key(raw_key)?);
            let mut page = [0; crate::crypto::PAGE_SZ];
            fs::File::open(&source)?.read_exact(&mut page)?;
            ensure!(
                crate::crypto::verify_page1(&key, &page),
                "数据库首页认证失败"
            );
            no_sidecars(&output)?;
            let parent = output
                .parent()
                .ok_or_else(|| anyhow::anyhow!("解密目标没有父目录"))?;
            fs::create_dir_all(parent)?;
            let mut guard = HostOutputGuard::new(parent)?;
            guard.protect_future(&cfg.db_dir)?;
            guard.protect_future(&runtime.directory)?;
            guard.pin_input(&runtime.config_path)?;
            if let Some(path) = &cfg.key_store {
                guard.pin_input(path)?;
            }
            // Keep the legacy pathname protected after explicit migration removed its file.
            guard.protect(&cfg.keys_file)?;
            guard.verify_replaceable_file(&output)?;
            atomic_output(&output, |tmp| {
                crate::crypto::full_decrypt(&source, tmp, &key)?;
                let db = rusqlite::Connection::open_with_flags(
                    tmp,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )?;
                let check: String = db.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
                ensure!(check == "ok", "解密数据库未通过 SQLite 完整性检查");
                no_sidecars(&output)?;
                guard.verify_replaceable_file(&output)?;
                Ok(())
            })?;
            Ok("written")
        })();
        match result {
            Ok("missing") => {
                missing_keys += 1;
                report.skipped += 1;
            }
            Ok("skipped") => {
                unchanged += 1;
                report.skipped += 1;
            }
            Ok("planned") => report.planned += 1,
            Ok(_) => {
                report.written += 1;
                written_bytes += bytes;
            }
            Err(e) => report.failures.push(Failure {
                path: rel.into(),
                error: e.to_string(),
            }),
        }
        eprintln!(
            "数据库: {} / {}，{}",
            report.total,
            source_count,
            rel.display()
        );
    }
    if mode == Mode::Strict {
        return report.finish();
    }
    println!(
        "结果: {} 成功, {} 失败, {} 跳过(无密钥), {} 未变更, {} 待解密, 共 {} 个",
        report.written,
        report.failures.len(),
        missing_keys,
        unchanged,
        report.planned,
        report.total
    );
    println!(
        "解密数据量: {} 字节\n输出目录: {}",
        written_bytes,
        cfg.decrypted_dir.display()
    );
    for failure in &report.failures {
        eprintln!("失败: {}: {}", failure.path.display(), failure.error);
    }
    // 延续旧一键脚本的批次退出码；具体失败必须读取上面的逐项与汇总结果。
    Ok(())
}

fn no_sidecars(output: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        ensure!(
            matches!(fs::symlink_metadata(PathBuf::from(format!("{}{suffix}", output.display()))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound),
            "输出数据库存在 WAL/SHM/journal，请先关闭读取它的程序"
        );
    }
    Ok(())
}

fn validate_target(runtime: &RuntimeContext, output: &Path) -> Result<()> {
    separate(&runtime.config.db_dir, output)?;
    separate(&runtime.directory, output)?;
    let target = resolved(output)?;
    for source in [&runtime.config_path, &runtime.config.keys_file]
        .into_iter()
        .chain(runtime.config.key_store.iter())
    {
        ensure!(
            !target
                .as_os_str()
                .eq_ignore_ascii_case(resolved(source)?.as_os_str()),
            "解密目标不得覆盖配置或密钥文件"
        );
        if output.exists() && source.exists() {
            ensure!(
                !same_file::is_same_file(output, source)?,
                "解密目标不得覆盖配置或密钥文件别名"
            );
        }
    }
    Ok(())
}
