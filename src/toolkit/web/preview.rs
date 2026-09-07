//! 只读图片预览：严格附件元数据先行，只读取当前账号已解码缓存，不解密或下载。
use super::{query, server_types::Shared};
use crate::service::protocol::Kind;
use crate::{
    attachment::{AttachmentId, AttachmentKind},
    ipc::Request,
};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_MONTHS: usize = 128;
pub struct Image {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

pub fn identity(encoded: &str) -> Result<AttachmentId> {
    ensure!(encoded.len() <= 2048, "附件标识过长");
    let id = AttachmentId::decode(encoded)?;
    ensure!(
        id.kind == AttachmentKind::Image
            && id.db.is_none()
            && id.local_id > 0
            && id.create_time >= 0
            && id.create_time < i64::MAX
            && !id.chat.is_empty()
            && id.chat.len() <= 256
            && !id.chat.chars().any(char::is_control),
        "图片标识无效"
    );
    ensure!(id.encode()? == encoded, "图片标识不是规范编码");
    Ok(id)
}

pub async fn list(
    state: &Shared,
    chat: String,
    limit: usize,
    offset: usize,
    since: Option<i64>,
) -> Result<Value> {
    ensure!(limit <= 1000, "图片列表最多 1000 条");
    let mut data = query::request(
        state,
        Request::Attachments {
            chat,
            image_metadata: true,
            kinds: Some(vec!["image".into()]),
            limit,
            offset,
            since,
            until: None,
            with_meta: false,
            debug_source: false,
        },
    )
    .await?;
    if let Some(rows) = data["attachments"].as_array_mut() {
        for row in rows {
            if row["resource_status"] == "found" {
                if let Some(encoded) = row["attachment_id"].as_str() {
                    if identity(encoded).is_ok() {
                        row["preview_url"] = format!("/api/images/{encoded}").into();
                    }
                }
            }
        }
    }
    Ok(data)
}

pub async fn read(state: Arc<Shared>, encoded: String, id: AttachmentId) -> Result<Option<Image>> {
    let data = query::request(
        &state,
        Request::Attachments {
            chat: id.chat.clone(),
            image_metadata: true,
            kinds: Some(vec!["image".into()]),
            limit: 1000,
            offset: 0,
            since: Some(id.create_time),
            until: Some(id.create_time + 1),
            with_meta: false,
            debug_source: false,
        },
    )
    .await?;
    ensure!(
        data["username"].as_str() == Some(id.chat.as_str()),
        "图片账号会话身份不一致"
    );
    let rows = data["attachments"].as_array().context("图片元数据缺失")?;
    let matches: Vec<_> = rows
        .iter()
        .filter(|r| r["attachment_id"].as_str() == Some(encoded.as_str()))
        .collect();
    if matches.is_empty() {
        return Ok(None);
    }
    ensure!(
        matches.len() == 1 && matches[0]["resource_status"] == "found",
        "图片身份或资源有歧义"
    );
    let hash = matches[0]["md5"]
        .as_str()
        .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .context("图片资源摘要无效")?
        .to_ascii_lowercase();
    let mut roots = Vec::new();
    {
        let records = state.records.lock().unwrap();
        if let Some(task) = records.tasks.iter().rev().find(|t| {
            t.kind == Kind::DecodeImages && matches!(t.status.as_str(), "succeeded" | "failed")
        }) {
            // This path comes only from the authenticated daemon's task projection.
            roots.push(task.output_dir.join("images"));
        }
    }
    if let Some(root) = &state.effective_settings().image_cache_dir {
        roots.push(root.clone());
    }
    let permit = state
        .queries
        .clone()
        .try_acquire_owned()
        .map_err(|_| anyhow::anyhow!("图片读取繁忙"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        for root in roots {
            if let Some(image) = from_cache(&root, &id.chat, &hash)? {
                return Ok(Some(image));
            }
        }
        Ok(None)
    })
    .await?
}

fn month(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 7
        && bytes[4] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..].iter().all(u8::is_ascii_digit)
        && matches!(name[5..].parse::<u8>(), Ok(1..=12))
}

fn from_cache(root: &Path, chat: &str, hash: &str) -> Result<Option<Image>> {
    use crate::attachment::native_image::HostOutputGuard;
    if !root.exists() {
        return Ok(None);
    }
    let root_guard = HostOutputGuard::new(root)?;
    let chat = root.join(format!("{:x}", md5::compute(chat.as_bytes())));
    if !chat.exists() {
        return Ok(None);
    }
    let chat_guard = HostOutputGuard::new(&chat)?;
    let mut candidate: Option<PathBuf> = None;
    let mut candidate_guard = None;
    for (index, entry) in fs::read_dir(&chat)?.enumerate() {
        ensure!(index < MAX_MONTHS, "图片月份目录超过限额");
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("图片目录编码无效"))?;
        if !month(&name) {
            continue;
        }
        let directory = entry.path();
        let month_guard = HostOutputGuard::new(&directory)?;
        for extension in ["jpg", "jpeg", "png", "gif", "webp", "bmp"] {
            let path = directory.join(format!("{hash}.{extension}"));
            match fs::symlink_metadata(&path) {
                Ok(_) => {
                    ensure!(candidate.is_none(), "缓存存在多个图片候选");
                    candidate = Some(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        month_guard.verify()?;
        // 命中目录保持锁定直到文件读取结束，避免枚举后目录被替换。
        if candidate
            .as_ref()
            .is_some_and(|path| path.parent() == Some(directory.as_path()))
        {
            candidate_guard = Some(month_guard);
        }
    }
    let image = candidate.as_deref().map(read_file).transpose()?;
    if let Some(guard) = candidate_guard {
        guard.verify()?;
    }
    root_guard.verify()?;
    chat_guard.verify()?;
    Ok(image)
}

fn read_file(path: &Path) -> Result<Image> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
    };
    let parent = crate::attachment::native_image::HostOutputGuard::new(
        path.parent().context("图片缺少父目录")?,
    )?;
    // 不跟随最终重解析点，且在读取期间禁止其他句柄写入或替换文件。
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?;
    }
    ensure!(
        info.nNumberOfLinks == 1 && info.dwFileAttributes & (0x400 | 0x10) == 0,
        "图片缓存不是独立普通文件"
    );
    let size = file.metadata()?.len();
    ensure!(size > 0 && size <= MAX_IMAGE_BYTES, "图片预览大小超限");
    let mut bytes = Vec::with_capacity(size as usize);
    (&file).take(MAX_IMAGE_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 == size, "读取期间图片发生变化");
    let content_type = match crate::attachment::decoder::detect_image_format(&bytes) {
        "jpg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => anyhow::bail!("图片格式不支持浏览器预览"),
    };
    parent.verify()?;
    Ok(Image {
        bytes,
        content_type,
    })
}
