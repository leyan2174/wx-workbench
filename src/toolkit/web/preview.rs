//! HTTP preview RPC adapter. No cache enumeration or file reads in Web.
use super::{query, server_types::Shared};
pub use crate::service::web::identity;
use crate::{attachment::AttachmentId, service::web::Call};
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

pub struct Image {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}
pub async fn list(
    state: &Shared,
    chat: String,
    limit: usize,
    offset: usize,
    since: Option<i64>,
) -> Result<Value> {
    query::web(
        state,
        Call::Images {
            chat,
            limit,
            offset,
            since,
        },
    )
    .await
}
pub async fn read(state: Arc<Shared>, encoded: String, _id: AttachmentId) -> Result<Option<Image>> {
    let value = query::web(&state, Call::PreviewImage { encoded }).await?;
    if value["image"].is_null() {
        return Ok(None);
    }
    let image: crate::service::web::Image = serde_json::from_value(value["image"].clone())?;
    let (bytes, content_type) = image.into_bytes()?;
    Ok(Some(Image {
        bytes,
        content_type,
    }))
}
