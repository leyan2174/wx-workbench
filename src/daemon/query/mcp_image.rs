//! 图片查询适配：先核验唯一消息和账号资源清单，再执行本地无覆盖导出。
use super::{strict_message, DbCache, Names};
use crate::{
    adapters::wechat::media::{
        strict_image::{AccountSources, Proof},
        strict_message::Message,
    },
    attachment::{decoder::V2KeyMaterial, native_image},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

fn resource_discovery_error(error: crate::business::media::Error) -> anyhow::Error {
    use crate::business::media::{Failure, Stage};
    let context = match (error.stage, error.failure) {
        (Stage::Association, Failure::NotFound) => Some("exact image resource not found"),
        (Stage::Association, Failure::Ambiguous) => {
            Some("ambiguous exact image resource; chat mapping must be unique")
        }
        (Stage::Association, Failure::ConflictingEvidence) => Some("resource MD5 missing"),
        _ => None,
    };
    let error = anyhow::Error::new(error);
    match context {
        Some(context) => error.context(context),
        None => error,
    }
}

#[cfg(test)]
mod resource_error_tests {
    use super::resource_discovery_error;
    use crate::business::media::{Error, Failure, Stage};

    #[test]
    fn legacy_context_preserves_typed_resource_failure() {
        for (failure, text) in [
            (Failure::NotFound, "exact image resource not found"),
            (Failure::Ambiguous, "ambiguous exact image resource"),
        ] {
            let error = resource_discovery_error(Error::new(Stage::Association, failure));
            assert!(error.to_string().contains(text));
            let typed = error.downcast_ref::<Error>().unwrap();
            assert_eq!(typed.stage, Stage::Association);
            assert_eq!(typed.failure, failure);
        }
        let error =
            resource_discovery_error(Error::new(Stage::Revalidation, Failure::StaleEvidence));
        assert!(!error.to_string().contains("not found"));
        assert_eq!(
            error.downcast_ref::<Error>().unwrap().failure,
            Failure::StaleEvidence
        );
    }
}

/// MCP 宿主入口：输出目录必须由宿主显式提供，不从配置或消息推断路径与密钥。
pub async fn q_decode_image_for_host(
    db: &DbCache,
    names: &Names,
    chat: &str,
    local_id: i64,
    create_time: i64,
    output_root: &Path,
) -> Result<Value> {
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
    let message = match strict_message::with_resolved(
        db,
        names,
        chat,
        local_id,
        create_time,
        Message::capture,
    )
    .await?
    {
        Resolution::Found(message) => message,
        Resolution::ChatNotFound => return Ok(failure(1, "chat not found")),
        Resolution::MessageNotFound => return Ok(failure(1, "message not found")),
        Resolution::AmbiguousChat => return Ok(failure(2, "ambiguous chat")),
        Resolution::AmbiguousMessage => return Ok(failure(2, "ambiguous message identity")),
    };
    if !message.is_image() {
        return Ok(failure(1, "expected image base_type=3"));
    }
    let sources = AccountSources::capture(db.db_dir(), &names.msg_db_keys, &db.raw_db_keys())?;
    let resource_db = db
        .get(sources.resource_key())
        .await?
        .context("current account resource database unavailable")?;
    let discovery_db = resource_db.clone();
    // The host owns snapshot lifetime; only opaque adapter evidence leaves this callback.
    let proof = strict_message::with_resolved(
        db,
        names,
        chat,
        local_id,
        create_time,
        move |messages, raw| {
            let resource = crate::daemon::cache::ResourceSnapshot::new(&discovery_db)?;
            Proof::prepare(message, messages, raw, &resource.path()).map_err(|error| {
                match error.downcast::<crate::business::media::Error>() {
                    Ok(error) => resource_discovery_error(error),
                    Err(error) => error,
                }
            })
        },
    )
    .await?;
    let proof = match proof {
        Resolution::Found(proof) => proof,
        Resolution::AmbiguousChat | Resolution::AmbiguousMessage => {
            anyhow::bail!(crate::business::media::Error::new(
                crate::business::media::Stage::Revalidation,
                crate::business::media::Failure::Ambiguous
            ))
        }
        Resolution::ChatNotFound | Resolution::MessageNotFound => {
            anyhow::bail!(crate::business::media::Error::new(
                crate::business::media::Stage::Revalidation,
                crate::business::media::Failure::StaleEvidence
            ))
        }
    };
    // Copy key material only for the blocking task; never serialize it.
    let aes = key.aes_key.map(|value| zeroize::Zeroizing::new(*value));
    let xor_key = key.xor_key;
    tokio::task::spawn_blocking(move || -> Result<Value> {
        sources.verify()?;
        let snapshot = crate::daemon::cache::ResourceSnapshot::new(&resource_db)?;
        let result = proof.export(
            &sources,
            &snapshot.path(),
            &guard,
            V2KeyMaterial {
                aes_key: aes.as_deref(),
                xor_key,
            },
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

#[cfg(test)]
#[path = "../../../tests/fixtures/mcp-image/tests.rs"]
mod tests;
