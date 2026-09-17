//! 当前账号的数据库主文件快照导出，不合并实时 WAL。

use super::publication_report::{Failure, Report};
use crate::infrastructure::publication::*;
use crate::{attachment::local_files::HostOutputGuard, runtime::RuntimeContext};
use anyhow::{ensure, Result};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

fn database_key(value: &str) -> Result<[u8; 32]> {
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

pub(crate) fn decrypt(
    runtime: &RuntimeContext,
    keys: &HashMap<String, String>,
    incremental: bool,
    dry_run: bool,
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
    let sources = collect(&cfg.db_dir, "db", true)?;
    let mut report = Report::default();
    let source_count = sources.len();
    for source in sources {
        report.total += 1;
        let rel = source.strip_prefix(&cfg.db_dir)?;
        let name = rel.to_string_lossy().replace('\\', "/").to_lowercase();
        let output = cfg.decrypted_dir.join(rel);
        let result = (|| -> Result<&str> {
            validate_target(runtime, &output)?;
            let Some(raw_key) = normalized.get(&name) else {
                anyhow::bail!("数据库没有对应密钥");
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
            let config_pin = crate::service::config_pin::ConfigPin::new(runtime)?;
            let source_pin = crate::attachment::local_files::Pin::open(&source, false)?;
            no_sidecars(&output)?;
            let target = ExportTarget::capture_paths(&output, &decryption_protected(runtime))?;
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
            // Protect the configured key pathname even when its file is absent.
            guard.protect(&cfg.keys_file)?;
            guard.verify_replaceable_file(&output)?;
            target.write_with_checked(
                |tmp| {
                    crate::crypto::full_decrypt(&source, tmp, &key)?;
                    crate::infrastructure::sqlite_validation::validate_readonly_database(tmp)?;
                    Ok(())
                },
                || {
                    config_pin.verify(runtime)?;
                    source_pin.verify()?;
                    no_sidecars(&output)?;
                    guard.verify_replaceable_file(&output)
                },
            )?;
            Ok("written")
        })();
        match result {
            Ok("skipped") => report.skipped += 1,
            Ok("planned") => report.planned += 1,
            Ok(_) => report.written += 1,
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
    report.finish()
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
    validate_export_paths(output, &decryption_protected(runtime))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_validated_without_echoing_values() {
        assert!(database_key("invalid").is_err());
        assert_eq!(database_key(&"42".repeat(32)).unwrap(), [0x42; 32]);
        for whitespace in [" ", "\t", "\n", "\r", "\u{b}", "\u{c}"] {
            assert_eq!(
                database_key(&vec!["42"; 32].join(whitespace)).unwrap(),
                [0x42; 32]
            );
        }
        for invalid in [
            "4 2".repeat(32),
            "42".repeat(31),
            "42".repeat(33),
            "４２".repeat(32),
        ] {
            assert!(database_key(&invalid).is_err());
        }
    }
}
