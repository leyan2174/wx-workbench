//! 图片查询适配：先核验唯一消息和账号资源清单，再执行本地无覆盖导出。
use super::{strict_message, DbCache, Names};
use crate::attachment::{
    decoder::V2KeyMaterial,
    native_image::{self, ImageRequest, MessageIdentity},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

const RESOURCE_KEY: &str = "message/message_resource.db";
const MAX_INVENTORY_ENTRIES: usize = 20_000;

/// MCP 宿主入口：输出目录必须由宿主显式提供，不从配置或消息推断路径与密钥。
pub async fn q_decode_image_with_key_file(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
    output_root: &Path,
    key_file: Option<&Path>,
) -> Result<Value> {
    ensure!(
        key_file.is_none(),
        "Legacy plaintext image key files are unsupported"
    );
    let guard = image_guard(db, output_root, local_id, create_time)?;
    q_decode_image_guarded(
        db,
        names,
        chat,
        local_id,
        create_time,
        V2KeyMaterial {
            aes_key: None,
            xor_key: 0x88,
        },
        guard,
    )
    .await
    .map_err(|_| anyhow::anyhow!("image decoding failed; V2 requires an explicit valid image key"))
}

/// Web host-only adapter: caller supplies authorized account material in memory.
pub(crate) async fn q_decode_image_with_material(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
    output_root: &Path,
    material: V2KeyMaterial<'_>,
) -> Result<Value> {
    let guard = image_guard(db, output_root, local_id, create_time)?;
    q_decode_image_guarded(db, names, chat, local_id, create_time, material, guard)
        .await
        .map_err(|_| anyhow::anyhow!("image decoding failed"))
}

fn image_guard(
    db: &DbCache,
    output_root: &Path,
    local_id: i64,
    create_time: i64,
) -> Result<native_image::HostOutputGuard> {
    // 必须先做此检查；缺少宿主输出时不得访问账号或密钥文件。
    let mut guard = native_image::HostOutputGuard::new(output_root).map_err(|_| {
        anyhow::anyhow!("explicit existing absolute host output directory required")
    })?;
    ensure!(
        local_id > 0 && create_time >= 0,
        "invalid image message identity"
    );
    let protected = db
        .output_protection_paths()
        .map_err(|_| anyhow::anyhow!("image host protection metadata unavailable"))?;
    ensure!(
        !protected.is_empty(),
        "image host protection metadata unavailable"
    );
    guard
        .protect(db.db_dir())
        .map_err(|_| anyhow::anyhow!("image output conflicts with protected input"))?;
    for path in protected {
        guard
            .protect(&path)
            .map_err(|_| anyhow::anyhow!("image output conflicts with protected input"))?;
    }
    Ok(guard)
}

/// 仅供模块测试直接注入密钥；生产统一通过宿主入口保留最初的路径守卫。
#[cfg(test)]
pub async fn q_decode_image(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
    output_root: &Path,
    key: V2KeyMaterial<'_>,
) -> Result<Value> {
    let guard = native_image::HostOutputGuard::new(output_root)?;
    q_decode_image_guarded(db, names, chat, local_id, create_time, key, guard).await
}

async fn q_decode_image_guarded(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
    key: V2KeyMaterial<'_>,
    guard: native_image::HostOutputGuard,
) -> Result<Value> {
    ensure!(local_id > 0, "local_id must be positive");
    ensure!(create_time >= 0, "create_time must not be negative");
    use strict_message::Resolution;
    let message = match strict_message::locate(db, names, chat, local_id, create_time).await? {
        Resolution::Found(message) => message,
        Resolution::ChatNotFound => return Ok(failure(1, "chat not found")),
        Resolution::MessageNotFound => return Ok(failure(1, "message not found")),
        Resolution::AmbiguousChat => return Ok(failure(2, "ambiguous chat")),
        Resolution::AmbiguousMessage => return Ok(failure(2, "ambiguous message identity")),
    };
    if message.kind <= 0 || message.kind & 0xffff_ffff != 3 {
        return Ok(failure(1, "expected image base_type=3"));
    }
    let identity = MessageIdentity {
        username: message.username,
        source: message.source,
        local_id: message.local_id,
        create_time: message.create_time,
        local_type: message.kind,
    };
    let db_dir = db.db_dir().to_path_buf();
    let attach_root = db_dir
        .parent()
        .context("missing account root")?
        .join("msg/attach");
    // 匹配时规范化，缓存查找仍使用原名；重复别名也拒绝，不能随意挑一个 key。
    let raw_keys: Vec<_> = db
        .raw_db_keys()
        .into_iter()
        .filter(|key| normalize(key) == RESOURCE_KEY)
        .collect();
    ensure!(
        raw_keys.len() == 1,
        "resource key must be present and unique"
    );
    let raw_key = &raw_keys[0];
    let before = Inventory::capture(&db_dir, &names.msg_db_keys, raw_key)?;
    let resource_db = db
        .get(raw_key)
        .await?
        .context("current account resource database unavailable")?;
    let output_root = guard.output_root().to_path_buf();
    let message_keys = names.msg_db_keys.clone();
    let raw_key = raw_key.clone();
    // 密钥只为阻塞任务临时复制，并在任务结束时清零；不序列化或记录密钥。
    let aes = key.aes_key.map(|value| zeroize::Zeroizing::new(*value));
    let xor_key = key.xor_key;
    tokio::task::spawn_blocking(move || -> Result<Value> {
        before.verify(&db_dir, &message_keys, &raw_key)?;
        let snapshot = crate::daemon::cache::ResourceSnapshot::new(&resource_db)?;
        let result = native_image::export_image_with_guard(
            ImageRequest {
                message: &identity,
                resource_db: &snapshot.path(),
                attach_root: &attach_root,
                output_root: &output_root,
                key: V2KeyMaterial {
                    aes_key: aes.as_deref(),
                    xor_key,
                },
            },
            &guard,
        )?;
        // 成功发布后不再做清单检查、路径打开或其他可预见的失败操作。
        // 路径编码已由导出核心校验，以下字段均可直接序列化。
        Ok(json!({"exit_code":0, "status":"published", "image":result}))
    })
    .await?
}

fn failure(code: i32, text: &str) -> Value {
    json!({"exit_code":code, "text":text})
}

fn normalize(key: &str) -> String {
    key.replace('\\', "/").to_ascii_lowercase()
}

struct SourceState {
    path: PathBuf,
    identity: same_file::Handle,
    size: u64,
    modified: SystemTime,
}

impl SourceState {
    fn read(path: PathBuf) -> Result<Self> {
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "inventory source must be a regular file"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "inventory reparse point rejected"
            );
        }
        Ok(Self {
            identity: same_file::Handle::from_path(&path)?,
            path,
            size: metadata.len(),
            modified: metadata.modified()?,
        })
    }

    fn unchanged(&self, other: &Self) -> bool {
        self.path == other.path
            && self.identity == other.identity
            && self.size == other.size
            && self.modified == other.modified
    }
}

struct Inventory {
    files: Vec<SourceState>,
}

impl Inventory {
    fn capture(db_dir: &Path, message_keys: &[String], resource_key: &str) -> Result<Self> {
        let unknown = crate::daemon::meta::discover_unknown_shards_checked(db_dir, message_keys)?;
        ensure!(
            unknown.is_empty(),
            "unknown message shards; complete inventory required"
        );
        let mut resource_paths = Vec::new();
        for (index, entry) in fs::read_dir(db_dir.join("message"))?.enumerate() {
            ensure!(
                index < MAX_INVENTORY_ENTRIES,
                "resource inventory entry limit exceeded"
            );
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .context("invalid resource inventory filename")?
                .to_ascii_lowercase();
            if name.starts_with("message_") && name.contains("resource") && name.ends_with(".db") {
                ensure!(
                    name == "message_resource.db",
                    "unknown resource database; complete inventory required"
                );
                resource_paths.push(entry.path());
            }
        }
        ensure!(
            resource_paths.len() == 1,
            "resource source must be present and unique"
        );
        let resource = db_dir.join(resource_key.replace('\\', "/"));
        ensure!(
            same_file::is_same_file(&resource, &resource_paths[0])?,
            "resource key does not identify account source"
        );
        let mut paths: Vec<PathBuf> = message_keys
            .iter()
            .map(|key| db_dir.join(key.replace('\\', "/")))
            .collect();
        paths.push(resource);
        paths.sort();
        paths.dedup();
        let mut files = Vec::new();
        for path in paths {
            files.push(SourceState::read(path.clone())?);
            let mut wal = path.into_os_string();
            wal.push("-wal");
            let wal = PathBuf::from(wal);
            match fs::symlink_metadata(&wal) {
                Ok(_) => files.push(SourceState::read(wal)?),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Self { files })
    }

    fn verify(&self, db_dir: &Path, message_keys: &[String], resource_key: &str) -> Result<()> {
        let current = Self::capture(db_dir, message_keys, resource_key)?;
        ensure!(
            self.files.len() == current.files.len()
                && self
                    .files
                    .iter()
                    .zip(&current.files)
                    .all(|(a, b)| a.unchanged(b)),
            "account source inventory changed before image publication"
        );
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/fixtures/mcp-image/tests.rs"]
mod tests;
