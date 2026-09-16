//! Same-origin HTTP automatic-image adapter. All file/key work belongs to daemon.
use super::{query, server_types::Shared};
pub use crate::service::web::Failure;
use crate::service::web::{exact_identity, valid_source, Call};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceQuery {
    pub source: String,
}

pub struct Image {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

pub async fn decode(state: Arc<Shared>, encoded: String, source: String) -> Result<Image, Failure> {
    exact_identity(&encoded).map_err(|_| Failure::InvalidIdentity)?;
    if !valid_source(&source) {
        return Err(Failure::InvalidIdentity);
    }
    let value = query::web(&state, Call::DecodeImage { encoded, source })
        .await
        .map_err(|error| {
            if query::is_busy(&error) {
                Failure::Busy
            } else {
                Failure::DecodeFailed
            }
        })?;
    if let Some(failure) = value.get("failure") {
        return Err(serde_json::from_value(failure.clone()).map_err(|_| Failure::DecodeFailed)?);
    }
    let image: crate::service::web::Image =
        serde_json::from_value(value["image"].clone()).map_err(|_| Failure::DecodeFailed)?;
    let (bytes, content_type) = image.into_bytes().map_err(|_| Failure::DecodeFailed)?;
    Ok(Image {
        bytes,
        content_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn source_query_accepts_no_business_paths() {
        assert!(
            serde_json::from_value::<SourceQuery>(json!({"source":"message/message_0.db"})).is_ok()
        );
        for value in [
            json!({}),
            json!({"source":7}),
            json!({"source":"message/message_0.db","image_key_file":"secret"}),
            json!({"source":"message/message_0.db","output_root":"arbitrary"}),
        ] {
            assert!(serde_json::from_value::<SourceQuery>(value).is_err());
        }
    }
}
