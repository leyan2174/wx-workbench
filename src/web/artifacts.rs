//! Identity-only downloads. The daemon owns all filesystem access and validation.
use super::{server_types::Shared, *};
use crate::service::task_artifacts::{
    Artifact, ArtifactBytes, ArtifactsPage, CHUNK_BYTES, MAX_ARTIFACTS,
};
use axum::{
    body::{Body, Bytes},
    http::HeaderMap,
};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio_stream::wrappers::ReceiverStream;

fn hex_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DownloadTicket {
    expires: u64,
    ticket: String,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn ticket_mac(
    key: &str,
    runtime: &str,
    authority: &str,
    id: &str,
    artifact: &str,
    expires: u64,
) -> Hmac<Sha256> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    let payload = serde_json::to_vec(&(
        "wx-artifact-download-v1",
        runtime,
        authority,
        id,
        artifact,
        expires,
    ))
    .expect("string tuple");
    mac.update(&payload);
    mac
}

fn ticket_authorizes(
    key: &str,
    runtime: &str,
    authority: &str,
    method: &Method,
    uri: &axum::http::Uri,
    time: u64,
) -> bool {
    let parts: Vec<_> = uri.path().split('/').collect();
    if method != Method::GET
        || parts.len() != 7
        || parts[1] != "api"
        || parts[2] != "tasks"
        || parts[3].is_empty()
        || parts[4] != "artifacts"
        || !hex_id(parts[5])
        || parts[6] != "download"
    {
        return false;
    }
    let Ok(Query(query)) = Query::<DownloadTicket>::try_from_uri(uri) else {
        return false;
    };
    if !hex_id(&query.ticket) || query.expires <= time || query.expires.saturating_sub(time) > 60 {
        return false;
    }
    let signature: Vec<u8> = query
        .ticket
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |b: u8| {
                if b.is_ascii_digit() {
                    b - b'0'
                } else {
                    b - b'a' + 10
                }
            };
            digit(pair[0]) * 16 + digit(pair[1])
        })
        .collect();
    ticket_mac(key, runtime, authority, parts[3], parts[5], query.expires)
        .verify_slice(&signature)
        .is_ok()
}

pub(super) fn authorized(state: &Shared, method: &Method, uri: &axum::http::Uri) -> bool {
    ticket_authorizes(
        &state.token,
        &state.runtime.id,
        &state.authority,
        method,
        uri,
        now(),
    )
}

pub(super) async fn ticket(
    State(state): State<Arc<Shared>>,
    Path((id, artifact_id)): Path<(String, String)>,
    uri: axum::http::Uri,
) -> Response {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || !hex_id(&artifact_id)
        || uri.query().is_some()
    {
        return ArtifactError(StatusCode::BAD_REQUEST, "invalid_artifact_request").into_response();
    }
    if let Err(error) = metadata(&state, &id, &artifact_id).await {
        return failure(error).into_response();
    }
    let expires = now().saturating_add(60);
    let signature: String = ticket_mac(
        &state.token,
        &state.runtime.id,
        &state.authority,
        &id,
        &artifact_id,
        expires,
    )
    .finalize()
    .into_bytes()
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect();
    Json(json!({"url":format!("/api/tasks/{id}/artifacts/{artifact_id}/download?expires={expires}&ticket={signature}"),
        "expires_at":expires})).into_response()
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Pagination {
    offset: u64,
    limit: u32,
}
impl Default for Pagination {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: 50,
        }
    }
}

struct ArtifactError(StatusCode, &'static str);
impl IntoResponse for ArtifactError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({"error":{"code":self.1,"message":self.1}})),
        )
            .into_response()
    }
}
fn failure(error: anyhow::Error) -> ArtifactError {
    let code = error
        .downcast_ref::<crate::service::protocol::ServiceError>()
        .map(|e| e.code.as_str());
    let (status, code) = match code {
        Some("invalid_artifact_request") => (StatusCode::BAD_REQUEST, "invalid_artifact_request"),
        Some("not_found") => (StatusCode::NOT_FOUND, "not_found"),
        Some("task_not_terminal") => (StatusCode::CONFLICT, "task_not_terminal"),
        Some("result_unavailable") => (StatusCode::CONFLICT, "result_unavailable"),
        Some("artifact_unavailable") => (StatusCode::CONFLICT, "artifact_unavailable"),
        Some("artifact_changed") => (StatusCode::CONFLICT, "artifact_changed"),
        Some("artifact_unsafe") => (StatusCode::CONFLICT, "artifact_unsafe"),
        Some("artifact_busy") => (StatusCode::TOO_MANY_REQUESTS, "artifact_busy"),
        Some("unauthorized") => (StatusCode::UNAUTHORIZED, "unauthorized"),
        Some("configuration_changed") => (StatusCode::CONFLICT, "configuration_changed"),
        _ if error.is::<tokio::sync::TryAcquireError>() => {
            (StatusCode::TOO_MANY_REQUESTS, "artifact_busy")
        }
        _ => (StatusCode::SERVICE_UNAVAILABLE, "artifact_unavailable"),
    };
    ArtifactError(status, code)
}

pub(super) async fn list(
    State(state): State<Arc<Shared>>,
    Path(id): Path<String>,
    query: std::result::Result<Query<Pagination>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return ArtifactError(StatusCode::BAD_REQUEST, "invalid_artifact_request").into_response();
    };
    if !(1..=100).contains(&query.limit) {
        return ArtifactError(StatusCode::BAD_REQUEST, "invalid_artifact_request").into_response();
    }
    match state
        .backend(Call::TaskArtifacts {
            id,
            offset: query.offset,
            limit: query.limit,
        })
        .await
    {
        Ok(data) => Json(data).into_response(),
        Err(error) => failure(error).into_response(),
    }
}

async fn metadata(state: &Shared, id: &str, artifact_id: &str) -> Result<Artifact> {
    let mut offset = 0;
    loop {
        let page: ArtifactsPage = serde_json::from_value(
            state
                .backend(Call::TaskArtifacts {
                    id: id.into(),
                    offset,
                    limit: 100,
                })
                .await?,
        )?;
        ensure!(
            page.version == 1
                && page.task_id == id
                && page.scope == "chat_directory"
                && page.offset == offset
                && page.total <= MAX_ARTIFACTS as u64
                && page.items.len() <= 100,
            "invalid artifact page"
        );
        if let Some(item) = page
            .items
            .into_iter()
            .find(|item| item.artifact_id == artifact_id)
        {
            ensure!(hex_id(&item.sha256), "invalid artifact digest");
            return Ok(item);
        }
        match page.next_offset {
            Some(next) if next > offset && next < page.total => offset = next,
            None => {
                return Err(crate::service::protocol::ServiceError::new(
                    "not_found",
                    "Artifact not found",
                )
                .into())
            }
            _ => anyhow::bail!("invalid artifact pagination"),
        }
    }
}

fn decode(chunk: ArtifactBytes, id: &str, artifact: &Artifact, offset: u64) -> Result<Bytes> {
    let expected = (artifact
        .size
        .checked_sub(offset)
        .context("invalid offset")?)
    .min(CHUNK_BYTES as u64);
    ensure!(
        chunk.version == 1
            && chunk.task_id == id
            && chunk.artifact_id == artifact.artifact_id
            && chunk.offset == offset
            && chunk.size == artifact.size
            && chunk.sha256 == artifact.sha256
            && chunk.encoding == "base64"
            && chunk.bytes_read == expected
            && chunk.next_offset == offset + expected
            && chunk.eof == (offset + expected == artifact.size)
            && chunk.data_base64.len() <= (CHUNK_BYTES as usize).div_ceil(3) * 4,
        "invalid artifact chunk"
    );
    let bytes = base64::engine::general_purpose::STANDARD.decode(chunk.data_base64)?;
    ensure!(bytes.len() as u64 == expected, "invalid artifact bytes");
    Ok(Bytes::from(bytes))
}
async fn read(state: &Shared, id: &str, artifact: &Artifact, offset: u64) -> Result<Bytes> {
    let data = state
        .backend(Call::ReadTaskArtifact {
            id: id.into(),
            artifact_id: artifact.artifact_id.clone(),
            offset,
            max_bytes: CHUNK_BYTES,
        })
        .await?;
    decode(serde_json::from_value(data)?, id, artifact, offset)
}

fn disposition(name: &str) -> HeaderValue {
    let safe: String = name
        .chars()
        .filter(|c| {
            !c.is_control()
                && !"/\\\":;<>|?*".contains(*c)
                && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(180)
        .collect();
    let safe = safe.trim_matches([' ', '.']);
    let safe = if safe.is_empty() { "download" } else { safe };
    let encoded: String = safe
        .as_bytes()
        .iter()
        .map(|b| format!("%{b:02X}"))
        .collect();
    HeaderValue::from_str(&format!(
        "attachment; filename=\"download\"; filename*=UTF-8''{encoded}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("attachment; filename=\"download\""))
}

pub(super) async fn download(
    State(state): State<Arc<Shared>>,
    Path((id, artifact_id)): Path<(String, String)>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Response {
    if let Err(error) = reject_range(&headers) {
        return error.into_response();
    }
    if !hex_id(&artifact_id) || (uri.query().is_some() && !authorized(&state, &Method::GET, &uri)) {
        return ArtifactError(StatusCode::BAD_REQUEST, "invalid_artifact_request").into_response();
    }
    let artifact = match metadata(&state, &id, &artifact_id).await {
        Ok(item) => item,
        Err(error) => return failure(error).into_response(),
    };
    let first = match read(&state, &id, &artifact, 0).await {
        Ok(bytes) => bytes,
        Err(error) => return failure(error).into_response(),
    };
    stream_response(
        artifact.size,
        disposition(&artifact.name),
        first,
        move |offset| {
            let state = state.clone();
            let id = id.clone();
            let artifact = artifact.clone();
            async move { read(&state, &id, &artifact, offset).await }
        },
    )
}

fn reject_range(headers: &HeaderMap) -> std::result::Result<(), ArtifactError> {
    if headers.contains_key(header::RANGE) {
        Err(ArtifactError(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "range_not_supported",
        ))
    } else {
        Ok(())
    }
}

fn stream_response<F, Fut>(size: u64, name: HeaderValue, first: Bytes, mut next: F) -> Response
where
    F: FnMut(u64) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<Bytes>> + Send + 'static,
{
    // One queued chunk plus the current read; never accumulate the complete file.
    let (sender, receiver) = tokio::sync::mpsc::channel::<std::io::Result<Bytes>>(1);
    tokio::spawn(async move {
        let mut offset = first.len() as u64;
        if sender.send(Ok(first)).await.is_err() {
            return;
        }
        while offset < size {
            let Ok(permit) = sender.reserve().await else {
                return;
            };
            let chunk = tokio::select! {
                _ = sender.closed() => return,
                chunk = next(offset) => chunk,
            };
            match chunk {
                Ok(bytes) if !bytes.is_empty() && bytes.len() as u64 <= size - offset => {
                    offset += bytes.len() as u64;
                    permit.send(Ok(bytes));
                }
                _ => {
                    permit.send(Err(std::io::Error::other("artifact stream interrupted")));
                    return;
                }
            }
        }
    });
    let mut response = Body::from_stream(ReceiverStream::new(receiver)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&size.to_string()).expect("u64 header"),
    );
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CONTENT_DISPOSITION, name);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn unexpected_empty_chunk_is_not_a_successful_short_file() {
        let response = stream_response(
            6,
            disposition("report.json"),
            Bytes::from_static(b"abc"),
            |_| async { Ok(Bytes::new()) },
        );
        assert!(axum::body::to_bytes(response.into_body(), 100)
            .await
            .is_err());
    }
    #[tokio::test]
    async fn later_chunk_failure_is_a_body_error_not_a_short_success() {
        let response = stream_response(
            6,
            disposition("report.html"),
            Bytes::from_static(b"abc"),
            |offset| async move {
                assert_eq!(offset, 3);
                Err(anyhow::anyhow!("synthetic private error"))
            },
        );
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "6");
        assert!(axum::body::to_bytes(response.into_body(), 100)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn zero_byte_download_has_safe_headers_and_never_reads_another_chunk() {
        let response = stream_response(0, disposition("report.html"), Bytes::new(), |_| async {
            panic!("zero-byte file must not request a subsequent chunk");
            #[allow(unreachable_code)]
            Ok(Bytes::new())
        });
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "0");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/octet-stream"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert!(response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment;"));
        assert!(axum::body::to_bytes(response.into_body(), 100)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn chunked_download_preserves_bytes_and_offsets() {
        let response = stream_response(
            7,
            disposition("report.json"),
            Bytes::from_static(b"abc"),
            |offset| async move {
                match offset {
                    3 => Ok(Bytes::from_static(b"def")),
                    6 => Ok(Bytes::from_static(b"g")),
                    _ => panic!("invalid byte offset"),
                }
            },
        );
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 100)
                .await
                .unwrap()
                .as_ref(),
            b"abcdefg"
        );
    }
    #[test]
    fn ticket_is_short_lived_identity_bound_and_download_only() {
        let artifact = "a".repeat(64);
        let sign: String = ticket_mac("secret", "runtime", "localhost:1", "task", &artifact, 160)
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let path = format!("/api/tasks/task/artifacts/{artifact}/download");
        let query = format!("expires=160&ticket={sign}");
        let uri: axum::http::Uri = format!("{path}?{query}").parse().unwrap();
        let valid = |uri: &axum::http::Uri, method: &Method, time| {
            ticket_authorizes("secret", "runtime", "localhost:1", method, uri, time)
        };
        assert!(valid(&uri, &Method::GET, 100));
        assert!(!valid(&uri, &Method::GET, 160));
        assert!(!valid(&uri, &Method::GET, 99));
        assert!(!valid(&uri, &Method::POST, 100));
        for changed in [
            format!("/api/tasks/task?{query}"),
            format!("{}?{query}", path.replace("/task/", "/other/")),
            format!("{}?{query}", path.replace(&artifact, &"b".repeat(64))),
            format!("{path}?expires=159&ticket={sign}"),
            format!("{path}?expires=160&ticket={}", "0".repeat(64)),
            format!("{path}?{query}&expires=160"),
            format!("{path}?{query}&ticket={sign}"),
            format!("{path}?{query}&path=secret"),
        ] {
            assert!(!valid(&changed.parse().unwrap(), &Method::GET, 100));
        }
        assert!(!ticket_authorizes(
            "secret",
            "other",
            "localhost:1",
            &Method::GET,
            &uri,
            100
        ));
        assert!(!ticket_authorizes(
            "secret",
            "runtime",
            "localhost:2",
            &Method::GET,
            &uri,
            100
        ));
        assert!(!ticket_authorizes(
            "other",
            "runtime",
            "localhost:1",
            &Method::GET,
            &uri,
            100
        ));
    }

    #[test]
    fn range_is_rejected_instead_of_silently_ignored() {
        let mut headers = HeaderMap::new();
        assert!(reject_range(&headers).is_ok());
        for range in ["bytes=0-1", "bytes=10-", "invalid"] {
            headers.insert(header::RANGE, HeaderValue::from_str(range).unwrap());
            let error = reject_range(&headers).err().unwrap();
            assert_eq!(error.0, StatusCode::RANGE_NOT_SATISFIABLE);
        }
    }
    #[test]
    fn filename_is_attachment_without_path_or_control_characters() {
        let value = disposition("../\\\r\n\"<script>报告.html");
        let value = value.to_str().unwrap();
        assert!(value.starts_with("attachment;"));
        assert!(
            !value.contains('\r')
                && !value.contains('\n')
                && !value.contains("%2F")
                && !value.contains("%5C")
        );
    }
    #[test]
    fn empty_chunk_still_checks_identity() {
        let artifact = Artifact {
            artifact_id: "a".repeat(64),
            name: "empty".into(),
            role: "media".into(),
            media_type: "application/octet-stream".into(),
            size: 0,
            sha256: "b".repeat(64),
        };
        let make = || ArtifactBytes {
            version: 1,
            task_id: "task".into(),
            artifact_id: artifact.artifact_id.clone(),
            offset: 0,
            bytes_read: 0,
            next_offset: 0,
            size: 0,
            sha256: artifact.sha256.clone(),
            encoding: "base64".into(),
            data_base64: String::new(),
            eof: true,
        };
        assert!(decode(make(), "task", &artifact, 0).unwrap().is_empty());
        let mut bad = make();
        bad.sha256 = "c".repeat(64);
        assert!(decode(bad, "task", &artifact, 0).is_err());
    }
}
