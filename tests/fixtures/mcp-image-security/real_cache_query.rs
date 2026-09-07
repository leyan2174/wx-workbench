pub use crate::{real_cache::DbCache, Names};
use std::{collections::HashMap, path::PathBuf, sync::OnceLock};
#[path = "../../../src/daemon/query/mcp_image.rs"]
pub mod image;
pub async fn diagnostic(db:&DbCache,names:&Names,output:&std::path::Path)->anyhow::Result<serde_json::Value> {
    let mut guard=crate::native_image::HostOutputGuard::new(output)?;
    guard.protect(db.db_dir())?;
    for path in db.output_protection_paths()? { guard.protect(&path)?; }
    image::q_decode_image_with_key_file(db,names,"synthetic_peer",42,100,output,None).await
}
#[path = "../../../src/daemon/query/strict_message.rs"]
mod strict_message;
pub async fn cache(
    source: PathBuf,
    decrypted: PathBuf,
    mtime: PathBuf,
    keys: HashMap<String, String>,
) -> anyhow::Result<DbCache> {
    DbCache::with_dirs(source, decrypted, mtime, keys).await
}
fn msg_table_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^Msg_[0-9a-f]{32}$").unwrap())
}
fn ensure_complete_message_inventory(db: &DbCache, names: &Names) -> anyhow::Result<()> {
    anyhow::ensure!(
        crate::daemon::meta::discover_unknown_shards_checked(db.db_dir(), &names.msg_db_keys)?
            .is_empty(),
        "unknown message shards; complete inventory required"
    );
    Ok(())
}
