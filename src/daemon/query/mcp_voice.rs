//! Account inventory and async cache boundary for the media catalog adapter.
use crate::adapters::wechat::media::voice_catalog::{
    discover_media, source_key, validate_query, Catalog, MediaShard,
};
pub use crate::business::voice::catalog::resolve_exact_chat;
use crate::business::voice::catalog::{self as domain, Page, Query};
use crate::daemon::cache::DbCache;
use anyhow::{ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet};

/// 从账号级 DbCache 只读枚举媒体数据库路径；不读取密钥文件。
/// 磁盘上额外媒体片或 get 返回 None 均失败，不返回已查询的部分结果。
pub async fn q_voice_messages(db: &DbCache, query: &Query) -> Result<Page> {
    validate_query(query)?;
    let media_paths = db.media_db_keys();
    // DbCache 精确匹配原始键；只规范化证据，不能改变传给 get 的键。
    let mut original_keys = BTreeMap::new();
    for original in &media_paths {
        ensure!(
            original_keys
                .insert(source_key(original)?, original)
                .is_none(),
            "duplicate canonical media key"
        );
    }
    let keys: BTreeSet<_> = original_keys.keys().cloned().collect();
    ensure!(
        !keys.is_empty() && keys.len() == media_paths.len(),
        "empty or duplicate media inventory"
    );
    let discovered = discover_media(db.db_dir())?;
    ensure!(
        discovered == keys,
        "unknown or missing media shard; complete account inventory required"
    );
    let mut shards = Vec::new();
    for (source, original) in original_keys {
        let path = db
            .get(original)
            .await
            .with_context(|| format!("resolve media shard {source}"))?
            .with_context(|| format!("media shard unavailable or undecrypted: {source}"))?;
        shards.push(MediaShard { source, path });
    }
    let page = domain::list(&Catalog::new(&shards), query)?;
    ensure!(
        discover_media(db.db_dir())? == discovered,
        "media inventory changed during query"
    );
    Ok(page)
}
