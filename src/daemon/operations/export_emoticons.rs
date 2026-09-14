//! 表情导出编排；预览不下载、不创建输出目录，下载不暴露 URL 或密钥。
use anyhow::{ensure, Context, Result};
use std::{collections::HashMap, path::PathBuf};

use crate::{
    attachment::local_files::HostOutputGuard,
    daemon::cache::DbCache,
    runtime::RuntimeContext,
    toolkit::emoticons::{
        catalog,
        download::{download, DownloadOptions},
        types::Emoji,
    },
};

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, Clone)]
pub struct Args {
    /// 输出目录；默认配置文件旁的 exported_emoticons
    pub output_dir: Option<PathBuf>,
    /// 只列出表情，不下载，不创建输出目录
    #[arg(long)]
    pub dry_run: bool,
    /// 按描述或表情包 product_id 过滤，忽略大小写
    #[arg(long)]
    pub filter: Option<String>,
}

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
    let catalog = {
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
                catalog::load(&cache).await
            })?
    };
    ensure!(!catalog.items.is_empty(), "未找到任何表情");
    eprintln!(
        "表情映射: {} 个 NonStore，新增 {} 个 Store",
        catalog.non_store_count, catalog.store_added
    );
    let items = select(catalog.items, args.filter.as_deref());
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
    let mut success = 0;
    let mut failed = 0;
    for (index, item) in items.iter().enumerate() {
        match download(&item.md5, &item.info, &guard, &options) {
            Ok(result) => {
                success += 1;
                eprintln!(
                    "[{}/{}] {}{}",
                    index + 1,
                    items.len(),
                    result.filename,
                    if result.cached {
                        "（缓存）"
                    } else if result.conversion_fallback {
                        "（保留原始二进制）"
                    } else {
                        ""
                    }
                );
            }
            Err(_) => {
                // CDN 地址和签名来自数据库，错误链可能含凭据，不透传到终端。
                failed += 1;
                eprintln!("[{}/{}] 表情下载或写出失败", index + 1, items.len());
            }
        }
    }
    println!(
        "完成: {success} 成功, {failed} 失败\n输出目录: {}",
        output.display()
    );
    // 兼容旧脚本：单个下载失败不终止批次，也不改变批次退出码。
    Ok(())
}

fn select(items: Vec<Emoji>, filter: Option<&str>) -> Vec<Emoji> {
    let Some(keyword) = filter
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase)
    else {
        return items;
    };
    items
        .into_iter()
        .filter(|item| {
            item.info
                .caption
                .as_deref()
                .unwrap_or_default()
                .to_lowercase()
                .contains(&keyword)
                || item.info.product_id.to_lowercase().contains(&keyword)
        })
        .collect()
}

fn preview(items: &[Emoji]) -> String {
    use std::fmt::Write;
    let mut text = "MD5                               格式      来源  描述\n".to_owned();
    for item in items {
        let source = if item.info.product_id.is_empty() {
            "NonStore"
        } else {
            "Store"
        };
        writeln!(
            text,
            "{}  cdn={}  {:>8}  {}",
            item.md5,
            if item.info.cdn_url.is_empty() {
                "N"
            } else {
                "Y"
            },
            source,
            item.info.caption.as_deref().unwrap_or_default()
        )
        .unwrap();
    }
    writeln!(text, "\n共 {} 个表情", items.len()).unwrap();
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolkit::emoticons::types::EmojiInfo;

    #[test]
    fn filtering_and_preview_preserve_order_without_disclosing_secrets() {
        let items: Vec<_> = [("One", "", "A"), ("two", "PACK", "B"), ("其他", "", "C")]
            .into_iter()
            .map(|(caption, product, md5)| Emoji {
                md5: md5.into(),
                info: EmojiInfo {
                    caption: Some(caption.into()),
                    product_id: product.into(),
                    cdn_url: "https://private.invalid/token".into(),
                    aes_key: "secret-key".into(),
                    encrypt_url: "https://private.invalid/encrypted".into(),
                },
            })
            .collect();
        assert_eq!(
            select(items.clone(), Some("o"))
                .iter()
                .map(|x| x.md5.as_str())
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        assert_eq!(select(items.clone(), Some("pack")).len(), 1);
        assert_eq!(select(items.clone(), Some("")).len(), 3);
        assert!(select(items.clone(), Some("missing")).is_empty());
        let text = preview(&items);
        assert!(
            text.contains("NonStore") && text.contains("Store") && text.contains("共 3 个表情")
        );
        assert!(!text.contains("private.invalid") && !text.contains("secret-key"));
    }
}
