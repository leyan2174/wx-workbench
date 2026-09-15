//! 表情导出编排；预览不下载、不创建输出目录，下载不暴露 URL 或密钥。
use anyhow::{ensure, Context, Result};
use std::collections::HashMap;

use crate::{
    adapters::wechat::emoticons::CatalogSource,
    attachment::local_files::HostOutputGuard,
    business::emoticons::{self as domain, Source},
    daemon::cache::DbCache,
    runtime::RuntimeContext,
    toolkit::emoticons::download::{export_from, DownloadOptions},
};

pub use crate::service::operation_requests::export_emoticons::Args;

pub(super) fn export(
    runtime: RuntimeContext,
    keys: HashMap<String, String>,
    args: Args,
) -> Result<()> {
    ensure!(!keys.is_empty(), "密钥为空，请先提取密钥");
    let output = std::path::absolute(args.output_dir.unwrap_or_else(|| {
        runtime
            .config_path
            .parent()
            .unwrap()
            .join("exported_emoticons")
    }))?;
    super::export_chat::validate_output_for(&runtime, &output)?;
    // 单独缓存目录及账号锁，避免与常驻后台同时发布同一个解密缓存文件。
    let source = {
        let _lock = runtime.lock("emoticons.lock")?;
        let cache_dir = runtime.cache_dir().join("emoticons");
        tokio::runtime::Runtime::new()
            .context("创建表情读取运行时失败")?
            .block_on(async {
                let cache = DbCache::with_dirs(
                    runtime.config.db_dir.clone(),
                    cache_dir.clone(),
                    cache_dir.join("_mtimes.json"),
                    keys,
                )
                .await?;
                CatalogSource::load(&cache).await
            })?
    };
    let catalog = source.catalog()?;
    ensure!(!catalog.items.is_empty(), "未找到任何表情");
    eprintln!(
        "表情映射: {} 个 NonStore，新增 {} 个 Store",
        catalog.recorded_count, catalog.derived_count
    );
    let items = domain::select(catalog, args.filter.as_deref());
    if args.dry_run {
        print!("{}", preview(&items));
        return Ok(());
    }
    super::export_chat::validate_output_for(&runtime, &output)?;
    std::fs::create_dir_all(&output).context("创建表情输出目录失败")?;
    let mut guard = HostOutputGuard::new(&output)?;
    for path in [
        &runtime.config.db_dir,
        &runtime.config.decrypted_dir,
        &runtime.directory,
    ] {
        guard.protect_future(path)?;
    }
    guard.pin_input(&runtime.config_path)?;
    if let Some(path) = &runtime.config.key_store {
        guard.pin_input(path)?;
    }
    guard.protect(&runtime.config.keys_file)?;
    let options = DownloadOptions::default();
    let protected = crate::toolkit::export_protected(&runtime);
    let mut executor = Executor {
        source: &source,
        guard: &guard,
        options: &options,
        protected: &protected,
        current: 0,
        total: items.len(),
    };
    let report = domain::export_batch(&items, &mut executor);
    let (success, failed) = (report.succeeded, report.failed);
    println!(
        "完成: {success} 成功, {failed} 失败\n输出目录: {}",
        output.display()
    );
    finish_report(&report)
}

fn finish_report(report: &domain::BatchReport) -> Result<()> {
    crate::ipc::outcome::BusinessOutcome::from_counts(
        report.succeeded as u64,
        report.failed as u64,
    )
    .require_success()?;
    Ok(())
}

struct Executor<'a> {
    source: &'a CatalogSource,
    guard: &'a HostOutputGuard,
    options: &'a DownloadOptions,
    protected: &'a [std::path::PathBuf],
    current: usize,
    total: usize,
}
impl domain::Exporter for Executor<'_> {
    fn export(
        &mut self,
        reference: &domain::CatalogMediaRef,
    ) -> Result<domain::Exported, domain::Error> {
        self.current += 1;
        match export_from(
            self.source,
            reference,
            self.guard,
            self.options,
            self.protected,
        ) {
            Ok((outcome, file)) => {
                eprintln!(
                    "[{}/{}] {}{}",
                    self.current,
                    self.total,
                    file.filename,
                    if file.cached {
                        "（缓存）"
                    } else if file.conversion_fallback {
                        "（保留原始二进制）"
                    } else {
                        ""
                    }
                );
                Ok(outcome)
            }
            Err(error) => {
                eprintln!("[{}/{}] 表情下载或写出失败", self.current, self.total);
                Err(error)
            }
        }
    }
}

fn preview(items: &[domain::Emoticon]) -> String {
    use std::fmt::Write;
    let mut text = "MD5                               格式      来源  描述\n".to_owned();
    for item in items {
        let source = if item.package.is_empty() {
            "NonStore"
        } else {
            "Store"
        };
        writeln!(
            text,
            "{}  cdn={}  {:>8}  {}",
            item.id,
            if !item.has_direct_resource { "N" } else { "Y" },
            source,
            item.caption.as_deref().unwrap_or_default()
        )
        .unwrap();
    }
    writeln!(text, "\n共 {} 个表情", items.len()).unwrap();
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_batch_preserves_artifacts_continues_and_returns_shared_failure() {
        use crate::ipc::outcome::{BusinessFailure, BusinessOutcome};
        use std::{path::PathBuf, sync::Arc};
        struct SyntheticExporter {
            root: PathBuf,
            calls: usize,
            fail_all: bool,
        }
        impl domain::Exporter for SyntheticExporter {
            fn export(
                &mut self,
                _: &domain::CatalogMediaRef,
            ) -> Result<domain::Exported, domain::Error> {
                self.calls += 1;
                if self.fail_all || self.calls == 2 {
                    return Err(domain::Error::new(
                        domain::Stage::Publication,
                        domain::Failure::Refused,
                    ));
                }
                std::fs::write(self.root.join(format!("{}.gif", self.calls)), b"synthetic")
                    .unwrap();
                Ok(domain::Exported {
                    bytes: 9,
                    materialization: domain::Materialization::Downloaded,
                })
            }
        }
        let root = tempfile::tempdir().unwrap();
        let owner = Arc::new(());
        let items: Vec<_> = (0..3)
            .map(|slot| domain::Emoticon {
                reference: domain::CatalogMediaRef::new(&owner, slot),
                id: format!("item{slot}"),
                caption: None,
                package: String::new(),
                origin: domain::Origin::CatalogRecorded,
                has_direct_resource: true,
            })
            .collect();
        let mut exporter = SyntheticExporter {
            root: root.path().into(),
            calls: 0,
            fail_all: false,
        };
        let report = domain::export_batch(&items, &mut exporter);
        assert_eq!((report.succeeded, report.failed, exporter.calls), (2, 1, 3));
        assert_eq!(report.items.len(), 3);
        let error = finish_report(&report).unwrap_err();
        let failure = error.downcast_ref::<BusinessFailure>().unwrap();
        assert_eq!(failure.0, BusinessOutcome::Partial);
        assert_eq!(failure.0.worker_exit_code(), 20);
        assert_eq!(
            std::fs::read(root.path().join("1.gif")).unwrap(),
            b"synthetic"
        );
        assert_eq!(
            std::fs::read(root.path().join("3.gif")).unwrap(),
            b"synthetic"
        );
        assert!(!root.path().join("2.gif").exists());
        exporter.fail_all = true;
        let report = domain::export_batch(&items, &mut exporter);
        assert_eq!(
            finish_report(&report)
                .unwrap_err()
                .downcast_ref::<BusinessFailure>()
                .unwrap()
                .0,
            BusinessOutcome::Failure
        );
        let report = domain::export_batch(
            &items[..1],
            &mut SyntheticExporter {
                root: root.path().into(),
                calls: 0,
                fail_all: false,
            },
        );
        assert!(finish_report(&report).is_ok());
    }

    #[test]
    fn filtering_and_preview_preserve_order_without_disclosing_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("catalog.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(include_str!(
                "../../../tests/fixtures/emoticons-catalog/schema.sql"
            ))
            .unwrap();
        for (caption, product, md5) in [("One", "", "A"), ("two", "PACK", "B"), ("其他", "", "C")]
        {
            connection.execute("INSERT INTO kNonStoreEmoticonTable VALUES(?1,'secret-key','https://private.invalid/token','https://private.invalid/encrypted',?2)", rusqlite::params![md5, product]).unwrap();
            connection.execute("INSERT INTO kStoreEmoticonCaptionsTable(md5_,language_,caption_) VALUES(?1,'default',?2)", rusqlite::params![md5, caption]).unwrap();
        }
        let source = CatalogSource::from_path(&path).unwrap();
        assert_eq!(
            domain::select(source.catalog().unwrap(), Some("o"))
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        assert_eq!(
            domain::select(source.catalog().unwrap(), Some("pack")).len(),
            1
        );
        assert_eq!(domain::select(source.catalog().unwrap(), Some("")).len(), 3);
        assert!(domain::select(source.catalog().unwrap(), Some("missing")).is_empty());
        let text = preview(&source.catalog().unwrap().items);
        assert!(
            text.contains("NonStore") && text.contains("Store") && text.contains("共 3 个表情")
        );
        assert!(!text.contains("private.invalid") && !text.contains("secret-key"));
    }
}
