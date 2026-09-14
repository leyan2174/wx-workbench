//! 个人微信自动图片适配；由宿主注册同源鉴权 POST，不使用会隐式取钥的 Extract。
use super::{query, WebService as Shared};
use crate::attachment::{local_files::HostOutputGuard, AttachmentId, AttachmentKind};
use crate::service::web::{exact_identity as identity, valid_source, Failure};
use anyhow::{ensure, Context, Result};
#[cfg(test)]
use serde::Deserialize;
use serde_json::{json, Value};
#[cfg(test)]
use std::io::Write;
use std::{fs, io::Read, path::Path, sync::Arc, time::Duration};
#[cfg(test)]
use zeroize::{Zeroize, Zeroizing};

const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;

pub struct Image {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

/// chat 必须由固定账号查询结果给出，不能由显示名、时间戳或附件列表顺序猜测。
/// 这是待核验的定位信息，不是成功解码证明；重复分片身份由严格 IPC 拒绝。
pub fn descriptor(chat: &str, message: &Value) -> Option<Value> {
    if message
        .get("username")
        .is_some_and(|value| value.as_str() != Some(chat))
    {
        return None;
    }
    let source = message["source"].as_str()?;
    if !valid_source(source) {
        return None;
    }
    let image = message
        .get("local_type")
        .and_then(Value::as_i64)
        .or_else(|| message.get("type").and_then(Value::as_i64))
        .map(|kind| kind & 0xffff_ffff == 3)
        .unwrap_or_else(|| matches!(message["type"].as_str(), Some("image" | "图片")));
    if !image {
        return None;
    }
    let timestamp = message
        .get("create_time")
        .or_else(|| message.get("timestamp"))?
        .as_i64()?;
    let id = AttachmentId {
        v: 1,
        chat: chat.into(),
        local_id: message["local_id"].as_i64()?,
        create_time: timestamp,
        kind: AttachmentKind::Image,
        db: None,
    };
    let encoded = id.encode().ok()?;
    identity(&encoded).ok()?;
    Some(json!({"attachment_id":encoded, "source":source,
        "decode_url":format!("/api/images/{encoded}/decode?source={}", source.replace('/', "%2F")),
        "status":"pending", "binding":"pending_strict_validation"}))
}

#[cfg(test)]
#[derive(Deserialize)]
struct ConfigKeys {
    image_aes_key: Option<String>,
    image_xor_key: Option<Value>,
}
#[cfg(test)]
impl Drop for ConfigKeys {
    fn drop(&mut self) {
        if let Some(key) = self.image_aes_key.as_mut() {
            key.zeroize();
        }
        if let Some(Value::String(key)) = self.image_xor_key.as_mut() {
            key.zeroize();
        }
        self.image_xor_key = None;
    }
}

#[cfg(test)]
fn key_bytes(bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let config: ConfigKeys = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("invalid image configuration"))?;
    let aes = config
        .image_aes_key
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(crate::toolkit::parse_image_aes)
        .transpose()?
        .map(Zeroizing::new);
    let xor = match config
        .image_xor_key
        .as_ref()
        .filter(|value| !value.is_null())
    {
        Some(value) => crate::toolkit::parse_image_xor(
            &value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string()),
        )?,
        None => 0x88,
    };
    let text = match aes {
        Some(aes) => {
            // 宿主密钥 JSON 与配置共用 ASCII 解析器，不能把密钥字节转成十六进制。
            let text = Zeroizing::new(serde_json::to_string(std::str::from_utf8(&*aes)?)?);
            Zeroizing::new(format!(
                "{{\"aes_key\":{},\"xor_key\":{xor}}}",
                text.as_str()
            ))
        }
        None => Zeroizing::new(format!("{{\"xor_key\":{xor}}}")),
    };
    Ok(Zeroizing::new(text.as_bytes().to_vec()))
}

/// 仅由已鉴权同源 POST 调用；不接收任意输出或密钥路径，不扫描/联网/调用转换器。
pub async fn decode(
    state: Arc<Shared>,
    encoded: String,
    source: String,
) -> std::result::Result<Image, Failure> {
    let id = identity(&encoded).map_err(|_| Failure::InvalidIdentity)?;
    if !valid_source(&source) {
        return Err(Failure::InvalidIdentity);
    }
    let permit = state
        .decodes
        .clone()
        .try_acquire_owned()
        .map_err(|_| Failure::Busy)?;
    if *state.shutdown.borrow() {
        return Err(Failure::Unavailable);
    }
    // 请求取消只丢弃等待句柄，操作继续持有临时文件直到 IPC 返回并清理。
    tokio::spawn(async move {
        let _permit = permit;
        let temporary = tempfile::tempdir().map_err(|_| Failure::DecodeFailed)?;
        let result = decode_at(&state, &id, &source, temporary.path()).await;
        let cleanup = cleanup(temporary).await;
        if cleanup.is_err() {
            state.event("image_status", json!({"status":"cleanup_failed"}));
        }
        cleanup?;
        result
    })
    .await
    .map_err(|_| Failure::DecodeFailed)?
}

/// 宿主已发出 shutdown 后、销毁 Tokio runtime 前调用；等待取消后仍持有临时文件的操作。
pub async fn drain(state: &Shared) -> bool {
    tokio::time::timeout(Duration::from_secs(30), state.decodes.acquire_many(2))
        .await
        .is_ok_and(|result| result.is_ok())
}

async fn cleanup(temporary: tempfile::TempDir) -> std::result::Result<(), Failure> {
    // IPC 超时不等于 daemon 已释放文件；为其只读密钥句柄留出有限清理窗口。
    let key = temporary.path().join("image-key.json");
    for _ in 0..50 {
        match fs::remove_file(&key) {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    temporary.close().map_err(|_| Failure::DecodeFailed)
}

#[cfg(test)]
fn create_key_file(path: &Path) -> Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    // 写入前禁止其他数据句柄打开空文件，权限设置失败时不写入任何密钥。
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(path)?;
    crate::toolkit::private_file::restrict(&file)?;
    Ok(file)
}

fn query_failure(error: anyhow::Error) -> Failure {
    if query::is_busy(&error) {
        Failure::Busy
    } else if let Some(failure) = error.downcast_ref::<crate::ipc::outcome::BusinessFailure>() {
        match failure.legacy_exit_code() {
            Some(1) => Failure::Unavailable,
            Some(2) => Failure::Ambiguous,
            _ => Failure::DecodeFailed,
        }
    } else {
        Failure::DecodeFailed
    }
}

async fn decode_at(
    state: &Shared,
    id: &AttachmentId,
    source: &str,
    root: &Path,
) -> std::result::Result<Image, Failure> {
    let prepare = || -> Result<(HostOutputGuard, std::path::PathBuf)> {
        let mut guard = HostOutputGuard::new(root)?;
        let account = state
            .runtime
            .config
            .db_dir
            .parent()
            .context("account root missing")?;
        for path in [account, &state.runtime.config.decrypted_dir] {
            guard.protect_future(path)?;
        }
        guard.pin_input(&state.runtime.config_path)?;
        match fs::symlink_metadata(&state.runtime.config.keys_file) {
            Ok(_) => guard.protect(&state.runtime.config.keys_file)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                guard.protect_future(&state.runtime.config.keys_file)?;
            }
            Err(error) => return Err(error.into()),
        }
        if let Some(path) = &state.runtime.config.key_store {
            match fs::symlink_metadata(path) {
                Ok(_) => guard.protect(path)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    guard.protect_future(path)?
                }
                Err(error) => return Err(error.into()),
            }
        }
        let mut current = state.runtime.clone();
        current.config = crate::config::load_config_at(&state.runtime.config_path)?;
        ensure!(
            state.runtime.same_account(&current)?,
            "fixed configuration identity changed"
        );
        let output = root.join("output");
        fs::create_dir(&output)?;
        guard.verify()?;
        Ok((guard, output))
    };
    let (_guard, output) = prepare().map_err(|_| Failure::DecodeFailed)?;
    let report = query::request(
        state,
        crate::ipc::Request::DecodeImage {
            chat: id.chat.clone(),
            local_id: id.local_id,
            create_time: id.create_time,
            output_root: output.to_str().ok_or(Failure::DecodeFailed)?.into(),
            image_key_file: None,
        },
    )
    .await
    .map_err(query_failure)?;
    match report["exit_code"].as_i64() {
        Some(2) => return Err(Failure::Ambiguous),
        Some(1) => return Err(Failure::Unavailable),
        Some(0) if report["status"] == "published" => (),
        _ => return Err(Failure::DecodeFailed),
    }
    let format = report["image"]["format"]
        .as_str()
        .ok_or(Failure::DecodeFailed)?;
    if !matches!(format, "jpg" | "png" | "gif" | "webp" | "bmp") {
        return Err(Failure::UnsupportedFormat);
    }
    read_result(&output, id, source, &report).map_err(|_| Failure::DecodeFailed)
}

fn read_result(output: &Path, id: &AttachmentId, source: &str, report: &Value) -> Result<Image> {
    let image = &report["image"];
    let message = &image["message"];
    ensure!(
        message["username"] == id.chat
            && message["local_id"] == id.local_id
            && message["create_time"] == id.create_time
            && message["local_type"]
                .as_i64()
                .is_some_and(|kind| kind > 0 && kind & 0xffff_ffff == 3),
        "decoded image identity mismatch"
    );
    ensure!(
        valid_source(source) && message["source"].as_str() == Some(source),
        "decoded logical source mismatch"
    );
    let hash = image["decoded_md5"]
        .as_str()
        .context("decoded digest missing")?;
    ensure!(
        hash.len() == 32
            && hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid decoded digest"
    );
    let (format, content_type) = match image["format"].as_str() {
        Some("jpg") => ("jpg", "image/jpeg"),
        Some("png") => ("png", "image/png"),
        Some("gif") => ("gif", "image/gif"),
        Some("webp") => ("webp", "image/webp"),
        Some("bmp") => ("bmp", "image/bmp"),
        _ => anyhow::bail!("unsupported decoded image"),
    };
    let path = output.join(format!("{hash}.{format}"));
    ensure!(
        image["path"].as_str().map(Path::new) == Some(path.as_path()),
        "decoded path mismatch"
    );
    let guard = HostOutputGuard::new(output)?;
    guard.verify_replaceable_file(&path)?;
    use std::os::windows::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(&path)?;
    let size = file.metadata()?.len();
    ensure!(
        size > 0 && size <= MAX_IMAGE_BYTES && image["size"].as_u64() == Some(size),
        "decoded size mismatch"
    );
    let mut bytes = Vec::new();
    (&file).take(MAX_IMAGE_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == size && format!("{:x}", md5::compute(&bytes)) == hash,
        "decoded bytes mismatch"
    );
    ensure!(
        crate::attachment::decoder::detect_image_format(&bytes) == format,
        "decoded format mismatch"
    );
    ensure!(
        same_file::Handle::from_file(file.try_clone()?)? == same_file::Handle::from_path(&path)?,
        "decoded file identity changed"
    );
    guard.verify_replaceable_file(&path)?;
    Ok(Image {
        bytes,
        content_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_query_saturation_returns_busy_and_preserves_other_failures() -> Result<()> {
        let root = tempfile::tempdir()?;
        let config_path = root.path().join("config.json");
        let config = crate::config::Config {
            key_store: Some(root.path().join("keys.dpapi")),
            db_dir: root.path().join("account/db_storage"),
            keys_file: root.path().join("keys.json"),
            decrypted_dir: root.path().join("decrypted"),
            wechat_process: "SyntheticNeverLaunched.exe".into(),
        };
        fs::create_dir_all(&config.db_dir)?;
        fs::write(&config_path, serde_json::to_vec(&config)?)?;
        fs::write(&config.keys_file, b"{}")?;
        let runtime = crate::runtime::RuntimeContext::from_config(
            config_path,
            config,
            root.path().join("runtime"),
        )?;
        crate::key_store::Store::for_runtime(&runtime)?.update(
            Some(0),
            &[crate::key_store::Update::ImageXor(
                0xa2,
                crate::key_store::Verification::Verified,
            )],
        )?;
        let state = Shared::new(
            runtime.clone(),
            Arc::new(crate::daemon::query_state::QueryState::new(runtime)),
        );
        let _held = state.queries.acquire_many(4).await?;
        let work = tempfile::tempdir()?;
        let work_path = work.path().to_owned();
        let id = AttachmentId {
            v: 1,
            chat: "alice".into(),
            local_id: 7,
            create_time: 123,
            kind: AttachmentKind::Image,
            db: None,
        };
        let result = decode_at(&state, &id, "message/message_0.db", work.path()).await;
        assert!(matches!(result, Err(Failure::Busy)));
        assert_eq!(Failure::Busy.http_status(), 429);
        assert_eq!(state.queries.available_permits(), 0);
        assert!(!work_path.join("image-key.json").exists());
        assert!(work_path.join("output").is_dir());
        assert!(cleanup(work).await.is_ok());
        assert!(!work_path.exists());
        for error in [
            anyhow::anyhow!("查询繁忙"),
            anyhow::anyhow!("synthetic disconnected pipe"),
        ] {
            assert_eq!(query_failure(error), Failure::DecodeFailed);
        }
        assert_eq!(Failure::DecodeFailed.http_status(), 503);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn key_file_is_private_before_first_write_and_exclusive_until_closed() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("image-key.json");
        let mut file = create_key_file(&path).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 0);
        crate::toolkit::private_file::assert_private_acl(&path);
        assert_eq!(fs::File::open(&path).unwrap_err().raw_os_error(), Some(32));
        file.write_all(b"synthetic image key").unwrap();
        file.sync_all().unwrap();
        drop(file);
        crate::toolkit::private_file::assert_private_acl(&path);
        assert_eq!(fs::read(&path).unwrap(), b"synthetic image key");
        assert!(create_key_file(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"synthetic image key");
    }

    #[test]
    fn logical_source_rejects_path_variants_and_enforces_length_limit() {
        let boundary = format!("message/message_{}.db", "1".repeat(45));
        assert_eq!(boundary.len(), 64);
        assert!(valid_source(&boundary));
        assert!(!valid_source(&format!(
            "message/message_{}.db",
            "1".repeat(46)
        )));
        for invalid in [
            "",
            "message\\message_0.db",
            "message/message_-1.db",
            "message/message_0.db:stream",
            "message/message_0.db\0",
            "message%2Fmessage_0.db",
            "message/../message/message_0.db",
        ] {
            assert!(!valid_source(invalid), "accepted source: {invalid:?}");
        }
    }

    #[test]
    fn failure_http_contract_is_stable() {
        for (failure, code, status) in [
            (Failure::InvalidIdentity, "invalid_identity", 400),
            (Failure::Busy, "busy", 429),
            (Failure::Ambiguous, "ambiguous", 409),
            (Failure::Unavailable, "unavailable", 404),
            (Failure::UnsupportedFormat, "unsupported_format", 415),
            (Failure::DecodeFailed, "decode_failed", 503),
        ] {
            assert_eq!(failure.code(), code);
            assert_eq!(failure.http_status(), status);
            assert!(!failure.message().is_empty());
        }
    }

    #[test]
    fn key_bridge_includes_only_validated_image_fields() {
        let source = br#"{"image_aes_key":"0123456789abcdef","image_xor_key":162,"openai_api_key":"UNRELATED_SYNTHETIC_SECRET","db_dir":"UNRELATED_PATH"}"#;
        let bytes = key_bytes(source).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value, json!({"aes_key":"0123456789abcdef","xor_key":162}));
        assert!(!String::from_utf8_lossy(&bytes).contains("UNRELATED"));
        assert_eq!(
            serde_json::from_slice::<Value>(&key_bytes(b"{}").unwrap()).unwrap(),
            json!({"xor_key":136})
        );
        for config in [
            json!({"image_xor_key":162}),
            json!({"image_aes_key":null,"image_xor_key":"0xa2"}),
            json!({"image_aes_key":"","image_xor_key":162}),
        ] {
            let bytes = key_bytes(&serde_json::to_vec(&config).unwrap()).unwrap();
            let bridged: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(bridged, json!({"xor_key":162}));
            let consumed = bridged
                .get("aes_key")
                .map(|key| crate::toolkit::parse_image_aes(key.as_str().unwrap()))
                .transpose()
                .unwrap();
            assert!(consumed.is_none());
        }
        assert!(key_bytes(br#"{"image_aes_key":"invalid"}"#).is_err());
        assert!(key_bytes(br#"{"image_xor_key":256}"#).is_err());
        for configured in [
            "0123456789abcdef",
            "0123456789abcdef0123456789abcdef",
            "key\"\\abcdefghijk",
        ] {
            let config = serde_json::to_vec(&json!({
                "image_aes_key": configured,
                "image_xor_key": "0xa2"
            }))
            .unwrap();
            let bridged: Value = serde_json::from_slice(&key_bytes(&config).unwrap()).unwrap();
            // 与 daemon::query::mcp_image::parse_key_json 使用同一个消费解析器。
            let consumed =
                crate::toolkit::parse_image_aes(bridged["aes_key"].as_str().unwrap()).unwrap();
            assert_eq!(consumed.as_slice(), &configured.as_bytes()[..16]);
            assert_eq!(
                consumed,
                crate::toolkit::parse_image_aes(configured).unwrap()
            );
            assert_eq!(bridged["aes_key"].as_str().unwrap().len(), 16);
            assert_eq!(bridged["xor_key"], 162);
        }
    }

    #[tokio::test]
    async fn cleanup_removes_synthetic_key_and_output() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_path_buf();
        fs::write(path.join("image-key.json"), b"synthetic key").unwrap();
        fs::create_dir(path.join("output")).unwrap();
        fs::write(path.join("output/image.png"), b"synthetic output").unwrap();
        assert!(cleanup(root).await.is_ok());
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn cleanup_succeeds_when_preparation_never_created_key() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_path_buf();
        assert!(cleanup(root).await.is_ok());
        assert!(!path.exists());
    }

    #[test]
    fn descriptor_requires_explicit_nonzero_identity_and_image_type() {
        let row = json!({"username":"alice","local_id":7,"timestamp":123,"type":"图片","source":"message/message_2.db"});
        let descriptor = descriptor("alice", &row).unwrap();
        let id = identity(descriptor["attachment_id"].as_str().unwrap()).unwrap();
        assert_eq!(
            (id.chat.as_str(), id.local_id, id.create_time),
            ("alice", 7, 123)
        );
        assert_eq!(descriptor["source"], "message/message_2.db");
        assert_eq!(descriptor["status"], "pending");
        assert_eq!(descriptor["binding"], "pending_strict_validation");
        assert!(descriptor.get("preview_url").is_none());
        assert!(descriptor["decode_url"]
            .as_str()
            .unwrap()
            .ends_with("?source=message%2Fmessage_2.db"));
        assert!(super::descriptor("bob", &row).is_none());
        for (field, value) in [
            ("timestamp", json!(0)),
            ("local_id", Value::Null),
            ("type", json!("text")),
        ] {
            let mut invalid = row.clone();
            invalid[field] = value;
            assert!(super::descriptor("alice", &invalid).is_none());
        }
        for source in [
            "../message_2.db",
            "message/message_2.db/extra",
            "C:/message_2.db",
            "message/message_resource.db",
            "message/message_.db",
        ] {
            let mut invalid = row.clone();
            invalid["source"] = json!(source);
            assert!(super::descriptor("alice", &invalid).is_none());
        }
        let mut no_source = row.clone();
        no_source.as_object_mut().unwrap().remove("source");
        assert!(super::descriptor("alice", &no_source).is_none());
    }

    #[test]
    fn response_validation_reads_only_exact_identity_and_digest_named_file() {
        use base64::Engine;
        let root = tempfile::tempdir().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aD1sAAAAASUVORK5CYII=").unwrap();
        let id = AttachmentId {
            v: 1,
            chat: "alice".into(),
            local_id: 7,
            create_time: 123,
            kind: AttachmentKind::Image,
            db: None,
        };
        let hash = format!("{:x}", md5::compute(&bytes));
        let path = root.path().join(format!("{hash}.png"));
        fs::write(&path, &bytes).unwrap();
        let report = json!({"image":{"message":{"username":"alice","local_id":7,"create_time":123,"local_type":3,"source":"message/message_0.db"},"decoded_md5":hash,"path":path,"size":bytes.len(),"format":"png"}});
        let result = read_result(root.path(), &id, "message/message_0.db", &report).unwrap();
        assert_eq!(result.bytes, bytes);
        assert_eq!(result.content_type, "image/png");
        assert!(read_result(root.path(), &id, "message/message_1.db", &report).is_err());
        for (pointer, value) in [
            ("/image/message/username", json!("bob")),
            ("/image/message/local_id", json!(8)),
            ("/image/message/local_type", json!(1)),
            ("/image/message/source", json!("message/message_1.db")),
            ("/image/message/create_time", json!(124)),
            ("/image/path", json!("../outside.png")),
            ("/image/size", json!(1)),
            ("/image/format", json!("gif")),
            ("/image/decoded_md5", json!("../outside")),
        ] {
            let mut forged = report.clone();
            *forged.pointer_mut(pointer).unwrap() = value;
            assert!(read_result(root.path(), &id, "message/message_0.db", &forged).is_err());
        }
        let mut changed = bytes.clone();
        let last = changed.last_mut().unwrap();
        *last ^= 1;
        fs::write(&path, &changed).unwrap();
        assert!(read_result(root.path(), &id, "message/message_0.db", &report).is_err());

        let invalid_bytes = vec![0u8; bytes.len()];
        let invalid_hash = format!("{:x}", md5::compute(&invalid_bytes));
        let invalid_path = root.path().join(format!("{invalid_hash}.png"));
        fs::write(&invalid_path, &invalid_bytes).unwrap();
        let mut forged = report.clone();
        forged["image"]["decoded_md5"] = json!(invalid_hash);
        forged["image"]["path"] = json!(invalid_path);
        assert!(read_result(root.path(), &id, "message/message_0.db", &forged).is_err());
    }
}
