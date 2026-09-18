//! 使用 axum 提供 Web HTTP 接口，并内嵌 HTML、JavaScript 和 CSS 资源。
//! 任务提交与取消通过服务客户端转交 daemon，Web 维护用于展示的任务状态。
mod artifacts;
mod automatic_image;
#[cfg(test)]
mod automatic_image_runtime_tests;
mod plans;
mod preview;
mod query;
mod read_queries;
mod server_types;
mod voices;

use crate::ipc;
use crate::service::{
    plan as tasks,
    protocol::{Call, SettingsInput, Submission},
};
use anyhow::{ensure, Context, Result};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use server_types::{Records, Shared};
use std::{
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{broadcast, watch, Semaphore};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

type ApiResult = std::result::Result<Json<Value>, ApiError>;
struct ApiError(StatusCode, &'static str);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.0 == StatusCode::CONFLICT && self.1 == crate::service::web::QUERY_AMBIGUOUS_MESSAGE
        {
            return (
                self.0,
                Json(json!({
                    "error": self.1,
                    "code": crate::service::web::QUERY_AMBIGUOUS_CODE,
                    "status": "ambiguous",
                    "exit_code": 2,
                })),
            )
                .into_response();
        }
        if matches!(
            self.1,
            "plan_ref_unavailable"
                | "plan_ref_changed"
                | "plan_selection_invalid"
                | "plan_scan_not_authorized"
                | "media_write_not_authorized"
                | "invalid_page"
        ) {
            return (self.0, Json(json!({"error":self.1,"code":self.1}))).into_response();
        }
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
fn unavailable(_: impl std::fmt::Display) -> ApiError {
    ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "当前账号查询不可用，请确认配置、密钥和数据库已就绪",
    )
}
fn bad() -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, "请求参数无效或任务不支持所选选项")
}

fn query_error(error: anyhow::Error) -> ApiError {
    if error.is::<crate::service::web::QueryAmbiguity>()
        || error
            .downcast_ref::<crate::service::protocol::ServiceError>()
            .is_some_and(|error| error.code == crate::service::web::QUERY_AMBIGUOUS_CODE)
    {
        return ApiError(
            StatusCode::CONFLICT,
            crate::service::web::QUERY_AMBIGUOUS_MESSAGE,
        );
    }
    if let Some(failure) = error.downcast_ref::<crate::ipc::outcome::BusinessFailure>() {
        let mut public = business_error(failure.0);
        public.1 = failure.public_message();
        public
    } else if let Some(failure) = error
        .downcast_ref::<crate::service::protocol::ServiceError>()
        .and_then(|error| crate::ipc::outcome::BusinessFailure::from_service_code(&error.code))
    {
        let mut public = business_error(failure.0);
        public.1 = failure.public_message();
        public
    } else if query::is_busy(&error) {
        ApiError(StatusCode::TOO_MANY_REQUESTS, "查询繁忙，请稍后重试")
    } else {
        unavailable(error)
    }
}

fn business_error(outcome: crate::ipc::outcome::BusinessOutcome) -> ApiError {
    use crate::ipc::outcome::BusinessOutcome;
    let status = match outcome {
        BusinessOutcome::Partial => StatusCode::CONFLICT,
        BusinessOutcome::Refused | BusinessOutcome::Failure => StatusCode::UNPROCESSABLE_ENTITY,
        BusinessOutcome::Success => StatusCode::INTERNAL_SERVER_ERROR,
    };
    ApiError(status, outcome.public_message())
}

#[test]
fn business_http_status_is_distinct_from_transport_and_sanitized() {
    use crate::ipc::outcome::BusinessOutcome;
    for (outcome, status) in [
        (BusinessOutcome::Partial, StatusCode::CONFLICT),
        (BusinessOutcome::Refused, StatusCode::UNPROCESSABLE_ENTITY),
        (BusinessOutcome::Failure, StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let typed = query_error(anyhow::Error::new(outcome.require_success().unwrap_err()));
        assert_eq!(typed.0, status);
        let wire = query_error(
            crate::service::protocol::ServiceError::new(
                outcome.service_code(),
                "SYNTHETIC_PRIVATE_KEY",
            )
            .into(),
        );
        assert_eq!(wire.0, status);
        assert_eq!(wire.1, typed.1);
        assert!(!wire.1.contains("SYNTHETIC_PRIVATE_KEY"));
    }
    assert_eq!(
        query_error(anyhow::anyhow!("SYNTHETIC_PRIVATE_KEY")).0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[test]
fn query_busy_is_not_reported_as_backend_failure() {
    let error = query_error(
        crate::service::protocol::ServiceError::new("busy", "synthetic-private-detail").into(),
    );
    assert_eq!(error.0, StatusCode::TOO_MANY_REQUESTS);
    assert!(!error.1.contains("synthetic-private-detail"));
    let error = query_error(anyhow::anyhow!("synthetic-database-detail"));
    assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!error.1.contains("synthetic-database-detail"));
}

#[tokio::test]
async fn query_ambiguity_http_response_preserves_409_and_legacy_code() {
    for error in [
        anyhow::Error::new(crate::service::web::QueryAmbiguity),
        crate::service::protocol::ServiceError::new(
            crate::service::web::QUERY_AMBIGUOUS_CODE,
            "SYNTHETIC_PRIVATE_XML_OR_PATH",
        )
        .into(),
    ] {
        let response = query_error(error).into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["code"], "query_ambiguous");
        assert_eq!(value["status"], "ambiguous");
        assert_eq!(value["exit_code"], 2);
        assert!(!String::from_utf8_lossy(&bytes).contains("SYNTHETIC_PRIVATE"));
    }
}

fn backend_error(error: anyhow::Error) -> ApiError {
    if error.is::<tokio::sync::TryAcquireError>() {
        return ApiError(StatusCode::TOO_MANY_REQUESTS, "任务请求繁忙");
    }
    if let Some(error) = error.downcast_ref::<crate::service::protocol::ServiceError>() {
        return match error.code.as_str() {
            "plan_ref_unavailable" => ApiError(StatusCode::CONFLICT, "plan_ref_unavailable"),
            "plan_ref_changed" => ApiError(StatusCode::CONFLICT, "plan_ref_changed"),
            "plan_selection_invalid" => ApiError(StatusCode::BAD_REQUEST, "plan_selection_invalid"),
            "plan_scan_not_authorized" => {
                ApiError(StatusCode::FORBIDDEN, "plan_scan_not_authorized")
            }
            "invalid_page" => ApiError(StatusCode::BAD_REQUEST, "invalid_page"),
            "invalid_request" | "invalid_task" | "invalid_id" | "invalid_events"
            | "invalid_settings" => bad(),
            "not_found" => ApiError(StatusCode::NOT_FOUND, "任务不存在"),
            "conflict" | "settings_conflict" | "submission_conflict" | "configuration_changed" => {
                ApiError(StatusCode::CONFLICT, "任务或后台配置冲突")
            }
            "busy" | "queue_full" | "history_full" | "capacity" | "rate_limited" => {
                ApiError(StatusCode::TOO_MANY_REQUESTS, "任务容量已满或后台繁忙")
            }
            _ => unavailable(error),
        };
    }
    unavailable(error)
}

fn token_equal(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

async fn security(State(state): State<Arc<Shared>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if headers.get_all(header::HOST).iter().count() != 1
        || host != Some(&state.authority)
        || headers.get_all(header::ORIGIN).iter().count() > 1
        || (headers.contains_key(header::ORIGIN) && origin.is_none())
        || origin.is_some_and(|value| value != state.origin)
        || headers
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v != "same-origin" && v != "none")
    {
        return ApiError(StatusCode::FORBIDDEN, "请求来源不受信任").into_response();
    }
    if *state.shutdown.borrow() {
        return ApiError(StatusCode::SERVICE_UNAVAILABLE, "服务正在关闭").into_response();
    }
    let path = request.uri().path();
    let protected = path.starts_with("/api/") || path == "/stream";
    if protected {
        let auth = headers
            .get("x-wx-token")
            .and_then(|v| v.to_str().ok())
            .or_else(|| {
                headers
                    .get(header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("Bearer "))
            })
            .unwrap_or_default();
        if !token_equal(auth, &state.token)
            && !artifacts::authorized(&state, request.method(), request.uri())
        {
            return ApiError(StatusCode::UNAUTHORIZED, "需要本次启动令牌").into_response();
        }
    }
    if request.method() != Method::GET && request.method() != Method::HEAD {
        let csrf = headers
            .get("x-wx-csrf")
            .and_then(|v| v.to_str().ok())
            .or_else(|| headers.get("x-wx-token").and_then(|v| v.to_str().ok()))
            .unwrap_or_default();
        if request.method() != Method::POST
            || origin != Some(&state.origin)
            || !token_equal(csrf, &state.token)
        {
            return ApiError(StatusCode::FORBIDDEN, "写操作需要同源 POST 和 CSRF 令牌")
                .into_response();
        }
    }
    let mut response = match tokio::time::timeout(Duration::from_secs(30), next.run(request)).await
    {
        Ok(response) => response,
        Err(_) => ApiError(StatusCode::REQUEST_TIMEOUT, "请求超时").into_response(),
    };
    let headers = response.headers_mut();
    for (name, value) in [
        ("cache-control", "no-store"), ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"), ("x-frame-options", "DENY"),
        ("content-security-policy", "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"),
    ] { headers.insert(name, HeaderValue::from_static(value)); }
    response
}

async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_bytes!("assets/index.html").as_slice(),
    )
}
async fn js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_bytes!("assets/app.js").as_slice(),
    )
}
async fn css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_bytes!("assets/app.css").as_slice(),
    )
}

async fn state(State(state): State<Arc<Shared>>) -> ApiResult {
    let info = state.backend(Call::Info {}).await.map_err(backend_error)?;
    let kinds = info["task_kinds"]
        .as_array()
        .cloned()
        .unwrap_or_else(tasks::capabilities);
    let kinds = plans::capabilities(
        kinds,
        info["capabilities"]["chat_plan_v1"] == true,
        state.allow_plan_scan,
    );
    let kinds = voices::capabilities(
        kinds,
        info["capabilities"]["raw_voices_v1"] == true,
        state.allow_media_write,
    );
    let mut limits = info["limits"].clone();
    if let Some(limits) = limits.as_object_mut() {
        limits.insert("sse_clients".into(), json!(16));
    }
    Ok(Json(
        json!({"api_version":1,"engine":"rust","runtime_id":state.runtime.id,
        "gui_mode":"browser","task_kinds":kinds,
        "limits":limits,"capabilities":info["capabilities"],
        "history_persisted":info["history_persisted"],"running":info["running"],
        "sources":["wechat"],
        "image_preview":{"enabled":true,"readonly":true,"max_bytes":16777216,"requires_decoded_cache":true},
        "boundaries":["GUI 为本地浏览器页面，不是原 tkinter/EXE 窗口","图片预览只读取缓存；没有缓存时须先批量解密图片"]}),
    ))
}

async fn list_tasks(State(state): State<Arc<Shared>>) -> ApiResult {
    state
        .backend(Call::List {})
        .await
        .map(Json)
        .map_err(backend_error)
}
async fn task(State(state): State<Arc<Shared>>, Path(id): Path<String>) -> ApiResult {
    state
        .backend(Call::Get { id })
        .await
        .map(Json)
        .map_err(backend_error)
}
async fn submit(
    State(state): State<Arc<Shared>>,
    headers: axum::http::HeaderMap,
    body: std::result::Result<Json<Submission>, axum::extract::rejection::JsonRejection>,
) -> std::result::Result<(StatusCode, Json<Value>), ApiError> {
    let Json(task) = body.map_err(|_| bad())?;
    let idempotency_key = match headers.get("idempotency-key") {
        Some(value) => {
            let key = value.to_str().map_err(|_| bad())?;
            if key.len() != 64
                || !key
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(bad());
            }
            key.to_owned()
        }
        None => server_types::random_id().map_err(unavailable)?,
    };
    if !state.allow_plan_scan && plans::requires_scan(&task) {
        return plans::existing_scan(&state, &idempotency_key, &task)
            .await
            .map(|value| (StatusCode::ACCEPTED, Json(value)));
    }
    if !state.allow_media_write && task.kind == crate::service::protocol::Kind::ExportVoices {
        return voices::existing(&state, &idempotency_key, &task)
            .await
            .map(|value| (StatusCode::ACCEPTED, Json(value)));
    }
    let value = state
        .backend(Call::Submit {
            idempotency_key,
            task: task.into(),
        })
        .await
        .map_err(backend_error)?;
    Ok((StatusCode::ACCEPTED, Json(value)))
}
async fn cancel(State(state): State<Arc<Shared>>, Path(id): Path<String>) -> ApiResult {
    state
        .backend(Call::Cancel { id })
        .await
        .map(Json)
        .map_err(backend_error)
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Filter {
    source: String,
    query: Option<String>,
    name: Option<String>,
    chat: Option<String>,
    limit: usize,
    offset: usize,
    since: Option<i64>,
}
impl Default for Filter {
    fn default() -> Self {
        Self {
            source: "wechat".into(),
            query: None,
            name: None,
            chat: None,
            limit: 200,
            offset: 0,
            since: None,
        }
    }
}
impl Filter {
    fn validate(&self) -> std::result::Result<(), ApiError> {
        if self.source != "wechat"
            || self.limit == 0
            || self.limit > 2000
            || self.offset > 1_000_000
            || [&self.query, &self.name, &self.chat]
                .into_iter()
                .flatten()
                .any(|s| s.len() > 256 || s.chars().any(char::is_control))
        {
            return Err(bad());
        }
        Ok(())
    }
}
type FilterInput = std::result::Result<Query<Filter>, axum::extract::rejection::QueryRejection>;
async fn contacts(State(state): State<Arc<Shared>>, filter: FilterInput) -> ApiResult {
    let Query(filter) = filter.map_err(|_| bad())?;
    filter.validate()?;
    query::request(
        &state,
        ipc::Request::Contacts(ipc::ContactsRequest {
            query: filter.query,
            limit: filter.limit,
        }),
    )
    .await
    .map(query::contacts)
    .map(Json)
    .map_err(query_error)
}
async fn sessions(State(state): State<Arc<Shared>>, filter: FilterInput) -> ApiResult {
    let Query(filter) = filter.map_err(|_| bad())?;
    filter.validate()?;
    query::request(
        &state,
        ipc::Request::Sessions {
            limit: filter.limit,
            with_meta: false,
            debug_source: false,
        },
    )
    .await
    .map(query::sessions)
    .map(Json)
    .map_err(query_error)
}
async fn tags(State(state): State<Arc<Shared>>, filter: FilterInput) -> ApiResult {
    let Query(filter) = filter.map_err(|_| bad())?;
    filter.validate()?;
    if filter.source != "wechat" {
        return Err(bad());
    }
    query::web(
        &state,
        crate::service::web::Call::Tags { name: filter.name },
    )
    .await
    .map(Json)
    .map_err(query_error)
}

async fn tag_members(State(state): State<Arc<Shared>>, filter: FilterInput) -> ApiResult {
    let Query(filter) = filter.map_err(|_| bad())?;
    filter.validate()?;
    if filter.source != "wechat" {
        return Err(bad());
    }
    let name = filter
        .name
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(bad)?;
    query::request(&state, ipc::Request::TagMembers { tag_name: name })
        .await
        .map(Json)
        .map_err(query_error)
}

async fn images(State(state): State<Arc<Shared>>, filter: FilterInput) -> ApiResult {
    let Query(filter) = filter.map_err(|_| bad())?;
    filter.validate()?;
    if filter.source != "wechat" || filter.limit > 1000 {
        return Err(bad());
    }
    let chat = filter.chat.filter(|s| !s.is_empty()).ok_or_else(bad)?;
    preview::list(&state, chat, filter.limit, filter.offset, filter.since)
        .await
        .map(Json)
        .map_err(query_error)
}

async fn image(
    State(state): State<Arc<Shared>>,
    Path(encoded): Path<String>,
) -> std::result::Result<Response, ApiError> {
    let id = preview::identity(&encoded).map_err(|_| bad())?;
    let image = preview::read(state, encoded, id)
        .await
        .map_err(query_error)?
        .ok_or(ApiError(
            StatusCode::NOT_FOUND,
            "图片未在已授权的解码缓存中找到，请先运行批量解密图片",
        ))?;
    Ok((
        [
            (header::CONTENT_TYPE, image.content_type),
            (
                header::HeaderName::from_static("x-wx-image-binding"),
                "exact-resource-cache-filename-heuristic",
            ),
        ],
        image.bytes,
    )
        .into_response())
}

async fn poll_tasks(state: Arc<Shared>, initial_cursor: u64) {
    let mut stop = state.shutdown.subscribe();
    let mut cursor = Some(initial_cursor);
    loop {
        if *stop.borrow() {
            break;
        }
        let poll = async {
            if cursor.is_none() {
                cursor =
                    Some(tokio::time::timeout(Duration::from_secs(25), state.reconcile()).await??);
                state.event(
                    "reset",
                    json!({"reason":"reconnected","reload":["state","tasks"]}),
                );
            }
            let page: crate::service::protocol::EventsPage = serde_json::from_value(
                state
                    .backend(Call::Events {
                        after: cursor.unwrap(),
                        limit: 128,
                        wait_ms: 1000,
                    })
                    .await?,
            )?;
            ensure!(page.events.len() <= 128, "Event page exceeds limit");
            if page.reset {
                cursor = None;
                state.event(
                    "reset",
                    json!({"reason":"lagged","reload":["state","tasks"]}),
                );
                return Ok::<_, anyhow::Error>(());
            }
            let mut previous = cursor.unwrap();
            ensure!(
                page.cursor >= previous,
                "Event cursor regressed without reset"
            );
            for event in &page.events {
                ensure!(
                    event.seq > previous && event.seq <= page.cursor,
                    "Invalid event sequence"
                );
                previous = event.seq;
            }
            let empty = page.events.is_empty();
            for event in page.events {
                match event.name.as_str() {
                    "task" => {
                        state.project_task(event.data.clone())?;
                        state.daemon_event("task", event.data, event.seq);
                    }
                    "log" => {
                        state.project_log(&event.data)?;
                        state.daemon_event("log", event.data, event.seq);
                    }
                    _ => {}
                }
            }
            cursor = Some(page.cursor);
            if empty {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Ok(())
        };
        tokio::select! {
            _ = stop.changed() => break,
            result = poll => if result.is_err() {
                cursor = None;
                state.event("reset", json!({"reason":"disconnected","reload":["state","tasks"]}));
                tokio::select! {
                    _ = stop.changed() => break,
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
            }
        }
    }
}

async fn events(State(state): State<Arc<Shared>>, headers: axum::http::HeaderMap) -> Response {
    let reconnect = headers.contains_key("last-event-id");
    if reconnect
        && headers
            .get("last-event-id")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .is_none()
    {
        return bad().into_response();
    }
    let permit = match state.streams.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => return ApiError(StatusCode::TOO_MANY_REQUESTS, "事件连接数已满").into_response(),
    };
    let stream = BroadcastStream::new(state.events.subscribe())
        .take_while(|event| !matches!(event,Ok(e) if e.name=="shutdown"))
        .map(move |event| {
            let _held = &permit;
            let (name, data, seq) = match event {
                Ok(e) => (e.name, e.data, e.seq),
                Err(_) => (
                    "reset",
                    json!({"reason":"lagged","reload":["state","tasks","history"]}),
                    None,
                ),
            };
            let event = Event::default().event(name).data(data.to_string());
            Ok::<_, Infallible>(match seq {
                Some(seq) => event.id(seq.to_string()),
                None => event,
            })
        });
    let ready = tokio_stream::once(Ok::<_, Infallible>(Event::default().event(if reconnect { "reset" } else { "ready" }).data(
        json!({"api_version":1,"reason":"reconcile","replay":false,"reload":["state","tasks","history"]}).to_string(),
    )));
    Sse::new(ready.chain(stream))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}
async fn automatic_image_decode(
    State(state): State<Arc<Shared>>,
    Path(encoded): Path<String>,
    input: std::result::Result<
        Query<automatic_image::SourceQuery>,
        axum::extract::rejection::QueryRejection,
    >,
    body: axum::body::Bytes,
) -> Response {
    let Ok(Query(source)) = input else {
        return bad().into_response();
    };
    if !body.is_empty() {
        return bad().into_response();
    }
    match automatic_image::decode(state, encoded, source.source).await {
        Ok(image) => ([(header::CONTENT_TYPE, image.content_type)], image.bytes).into_response(),
        Err(error) => (
            StatusCode::from_u16(error.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({"error": {"code": error.code(), "message": error.message()}})),
        )
            .into_response(),
    }
}

async fn shutdown(State(state): State<Arc<Shared>>) -> Json<Value> {
    let _ = state.shutdown.send(true);
    state.event("shutdown", json!({}));
    Json(json!({"stopping":true}))
}

fn router(state: Arc<Shared>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/app.js", get(js))
        .route("/app.css", get(css))
        .route("/api/state", get(state_handler))
        .route("/api/contacts", get(contacts))
        .route("/api/sessions", get(sessions))
        .route("/api/history", get(read_queries::history))
        .route("/api/search", get(read_queries::search))
        .route("/api/unread", get(read_queries::unread))
        .route("/api/members", get(read_queries::members))
        .route("/api/stats", get(read_queries::stats))
        .route("/api/favorites", get(read_queries::favorites))
        .route("/api/articles", get(read_queries::articles))
        .route("/api/sns-feed", get(read_queries::sns_feed))
        .route("/api/sns-search", get(read_queries::sns_search))
        .route(
            "/api/sns-notifications",
            get(read_queries::sns_notifications),
        )
        .route("/api/voice-messages", get(read_queries::voice_messages))
        .route("/api/decode-transfer", get(read_queries::decode_transfer))
        .route("/api/decode-location", get(read_queries::decode_location))
        .route("/api/decode-refer", get(read_queries::decode_refer))
        .route(
            "/api/decode-file-message",
            get(read_queries::decode_file_message),
        )
        .route(
            "/api/decode-record-item",
            get(read_queries::decode_record_item),
        )
        .route("/api/tags", get(tags))
        .route("/api/tag-members", get(tag_members))
        .route("/api/images", get(images))
        .route("/api/images/{id}", get(image))
        .route("/api/images/{id}/decode", post(automatic_image_decode))
        .route("/api/tasks", get(list_tasks).post(submit))
        .route(
            "/api/tasks/{task_id}/artifacts/{artifact_id}/plan",
            get(plans::read),
        )
        .route(
            "/api/tasks/{id}/artifacts/{artifact_id}/ticket",
            post(artifacts::ticket),
        )
        .route("/api/tasks/{id}/artifacts", get(artifacts::list))
        .route(
            "/api/tasks/{id}/artifacts/{artifact_id}/download",
            get(artifacts::download),
        )
        .route("/api/tasks/{id}", get(task))
        .route("/api/tasks/{id}/cancel", post(cancel))
        .route("/api/events", get(events_handler))
        .route("/stream", get(events_handler))
        .route("/api/shutdown", post(shutdown_handler))
        .fallback(|| async { ApiError(StatusCode::NOT_FOUND, "路由不存在") })
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), security))
        .with_state(state)
}

pub async fn serve(
    runtime: crate::runtime::RuntimeContext,
    args: crate::service::web::HostSettings,
) -> Result<()> {
    let _lock = runtime
        .lock("web.lock")
        .map_err(|_| anyhow::anyhow!("当前账号已有 Web 服务或运行目录不可写"))?;
    query::ensure_detached(runtime.clone()).await?;
    crate::service::client::wait_ready(&runtime).await?;
    let input = SettingsInput {
        image_cache_dir: args.image_cache_dir.clone(),
    };
    // Configure must reject a conflicting existing binding, never replace it.
    let info = tokio::time::timeout(
        Duration::from_secs(25),
        crate::service::client::request(&runtime, Call::Configure { settings: input }),
    )
    .await??;
    ensure!(
        info["runtime_id"] == runtime.id && info["configured"] == true,
        "Daemon configuration identity mismatch"
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, args.port))
        .await
        .context("无法绑定本地端口")?;
    let authority = format!("127.0.0.1:{}", listener.local_addr()?.port());
    let origin = format!("http://{authority}");
    let (events, _) = broadcast::channel(256);
    let (shutdown, _) = watch::channel(false);
    let state = Arc::new(Shared {
        runtime,
        token: server_types::random_id()?,
        authority,
        origin,
        allow_plan_scan: args.allow_plan_scan,
        allow_media_write: args.allow_media_write,
        records: Mutex::new(Records::default()),
        events,
        shutdown,
        queries: Arc::new(Semaphore::new(4)),
        query_waiters: Arc::new(Semaphore::new(8)),
        streams: Arc::new(Semaphore::new(16)),
        task_requests: Arc::new(Semaphore::new(4)),
    });
    let cursor = tokio::time::timeout(Duration::from_secs(25), state.reconcile()).await??;
    let router = router(state.clone());
    let url = format!("{}/#token={}", state.origin, state.token);
    println!("本地 Web（仅当前账号；Ctrl+C 关闭）：{url}");
    if args.open && open_browser(&url).is_err() {
        eprintln!("无法自动打开浏览器，请使用上方本地地址");
    }
    let task_events = tokio::spawn(poll_tasks(state.clone(), cursor));
    let monitor = tokio::spawn(query::monitor(state.clone()));
    let signal_state = state.clone();
    let signal = tokio::spawn(async move {
        let close = tokio::signal::windows::ctrl_close();
        let system_shutdown = tokio::signal::windows::ctrl_shutdown();
        match (close, system_shutdown) {
            (Ok(mut close), Ok(mut system_shutdown)) => {
                tokio::select! {
                    _=tokio::signal::ctrl_c()=>(),_=close.recv()=>(),_=system_shutdown.recv()=>(),
                }
            }
            _ => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
        let _ = signal_state.shutdown.send(true);
        signal_state.event("shutdown", json!({}));
    });
    let mut stop = state.shutdown.subscribe();
    use std::future::IntoFuture;
    let server = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            if !*stop.borrow() {
                let _ = stop.changed().await;
            }
        })
        .into_future();
    tokio::pin!(server);
    let mut stopped = state.shutdown.subscribe();
    let result = tokio::select! {
        result=&mut server=>result,
        _=async {if !*stopped.borrow() {let _=stopped.changed().await;}}=> {
            state.event("shutdown",json!({}));
            // Slow HTTP clients cannot prevent Web shutdown.
            tokio::time::timeout(Duration::from_secs(5),&mut server).await.unwrap_or(Ok(()))
        },
    };
    let _ = state.shutdown.send(true);
    state.event("shutdown", json!({}));
    let _ = task_events.await;
    let _ = monitor.await;
    signal.abort();
    let _ = signal.await;
    result.context("本地 Web 服务已停止")
}

// 局部状态变量与处理函数同名时，保留显式别名避免注册错误。
use self::{events as events_handler, shutdown as shutdown_handler, state as state_handler};

fn open_browser(url: &str) -> Result<()> {
    use windows::{
        core::{w, PCWSTR},
        Win32::{
            Foundation::HWND,
            UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
        },
    };
    let wide: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            HWND::default(),
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    ensure!(result.0 as usize > 32, "无法打开默认浏览器");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    async fn mock_service(
        runtime: &crate::runtime::RuntimeContext,
        settings: crate::service::settings::Settings,
    ) -> Result<(watch::Sender<bool>, tokio::task::JoinHandle<Result<()>>)> {
        use windows::Win32::{
            Foundation::FILETIME,
            System::Threading::{GetCurrentProcess, GetProcessTimes},
        };
        let mut birth = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut birth,
                &mut exit,
                &mut kernel,
                &mut user,
            )?;
        }
        std::fs::create_dir_all(&runtime.directory)?;
        std::fs::write(
            runtime.pid_path(),
            serde_json::to_vec(&json!({
                "pid":std::process::id(), "exe":std::env::current_exe()?,
                "created":(u64::from(birth.dwHighDateTime) << 32) | u64::from(birth.dwLowDateTime),
                "runtime_id":runtime.id,
            }))?,
        )?;
        let info = json!({"version":1,"runtime_id":runtime.id,"configured":true,
            "settings":settings,"history_persisted":true,"running":0,"cursor":0,
            "limits":{"queue":8,"history":100,"logs_per_task":256}});
        // One authoritative mock task lives behind the actual authenticated transport.
        let task = Arc::new(Mutex::new(None::<Value>));
        let handler = Arc::new(move |call: Call, peer_pid| {
            assert_eq!(peer_pid, std::process::id());
            let mut info = info.clone();
            let task = task.clone();
            async move {
                use crate::service::protocol::ServiceError;
                let mut task = task.lock().unwrap();
                match call {
                    Call::Info {} => {
                        info["cursor"] = json!(u64::from(task.is_some()));
                        info["running"] = json!(usize::from(
                            task.as_ref().is_some_and(|task| task["status"] == "queued")
                        ));
                        Ok(info)
                    }
                    Call::List {} => {
                        let rows: Vec<_> = task
                            .iter()
                            .cloned()
                            .map(|mut row| {
                                row["logs"] = json!([]);
                                row
                            })
                            .collect();
                        Ok(
                            json!({"tasks":rows,"cursor":u64::from(task.is_some()),"history_persisted":true}),
                        )
                    }
                    Call::Events { after, .. } => {
                        let cursor = u64::from(task.is_some());
                        let events: Vec<_> = task
                            .iter()
                            .filter(|_| after == 0)
                            .map(|task| json!({"seq":1,"name":"task","data":task}))
                            .collect();
                        Ok(json!({"events":events,"cursor":cursor,"reset":after > cursor}))
                    }
                    Call::Get { id } => task
                        .as_ref()
                        .filter(|task| task["id"] == id)
                        .cloned()
                        .ok_or_else(|| ServiceError::new("not_found", "Task not found")),
                    Call::Cancel { id } => {
                        let task = task
                            .as_mut()
                            .filter(|task| task["id"] == id)
                            .ok_or_else(|| ServiceError::new("not_found", "Task not found"))?;
                        task["status"] = json!("cancelled");
                        task["finished_at"] = json!(2);
                        Ok(task.clone())
                    }
                    Call::Submit {
                        idempotency_key,
                        task: request,
                    } => {
                        if idempotency_key != "a".repeat(64)
                            || request.kind != crate::service::protocol::Kind::WechatDecrypt
                        {
                            return Err(ServiceError::new(
                                "invalid_request",
                                "Invalid service request",
                            ));
                        }
                        Ok(task.get_or_insert_with(|| json!({
                            "id":"a".repeat(64), "kind":request.kind, "options":request.options,
                            "status":"queued", "created_at":1, "started_at":null,
                            "finished_at":null, "exit_code":null, "logs":[],
                            "log_start_seq":0, "next_log_seq":0, "output_dir":"synthetic-output",
                            "error":null,
                        })).clone())
                    }
                    Call::Configure { .. } => Err(ServiceError::new(
                        "conflict",
                        "Service request conflicts with current state",
                    )),
                    Call::Shutdown {} => panic!("Web must never stop the daemon"),
                    Call::Web { request } => Ok(json!({"web_call": request})),
                    _ => Err(ServiceError::new(
                        "invalid_request",
                        "Unsupported fixture request",
                    )),
                }
            }
        });
        let (shutdown, stop) = watch::channel(false);
        let handle = tokio::spawn(crate::service::transport::serve(
            runtime.clone(),
            handler,
            stop,
        ));
        let ready = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if crate::service::client::request(runtime, Call::Info {})
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if let Err(error) = ready {
            let _ = shutdown.send(true);
            handle.abort();
            let _ = handle.await;
            return Err(error.into());
        }
        Ok((shutdown, handle))
    }

    async fn sse_frame(response: &mut reqwest::Response) -> Result<String> {
        let mut bytes = Vec::new();
        loop {
            let chunk = response
                .chunk()
                .await?
                .context("SSE ended before an event")?;
            bytes.extend_from_slice(&chunk);
            ensure!(bytes.len() <= 64 * 1024, "SSE test frame exceeds limit");
            if bytes.windows(2).any(|bytes| bytes == b"\n\n") {
                return Ok(String::from_utf8(bytes)?);
            }
        }
    }

    async fn task_checks(client: &reqwest::Client, state: &Arc<Shared>) -> Result<()> {
        let permits = state.task_requests.clone().acquire_many_owned(4).await?;
        let response = client
            .get(format!("{}/api/tasks", state.origin))
            .header("x-wx-token", &state.token)
            .send()
            .await?;
        ensure!(
            response.status() == StatusCode::TOO_MANY_REQUESTS,
            "task request limit"
        );
        response.bytes().await?;
        ensure!(
            state.queries.available_permits() == 4,
            "task calls consumed query permits"
        );
        drop(permits);
        let response = client
            .get(format!("{}/api/tasks/{}", state.origin, "b".repeat(64)))
            .header("x-wx-token", &state.token)
            .send()
            .await?;
        ensure!(
            response.status() == StatusCode::NOT_FOUND,
            "backend not_found mapping"
        );
        response.bytes().await?;
        let mut id = String::new();
        for _ in 0..2 {
            let response = client
                .post(format!("{}/api/tasks", state.origin))
                .header("x-wx-token", &state.token)
                .header("origin", &state.origin)
                .header("idempotency-key", "a".repeat(64))
                .header("content-type", "application/json")
                .body(r#"{"kind":"wechat_decrypt"}"#)
                .send()
                .await?;
            ensure!(response.status() == StatusCode::ACCEPTED, "backend submit");
            let task: Value = serde_json::from_slice(&response.bytes().await?)?;
            let next = task["id"].as_str().context("missing submitted id")?;
            ensure!(
                id.is_empty() || id == next,
                "idempotency key was not forwarded"
            );
            id = next.to_owned();
        }
        ensure!(
            state.records.lock().unwrap().tasks.is_empty(),
            "HTTP submit owns a task queue"
        );
        let response = client
            .get(format!("{}/api/tasks", state.origin))
            .header("x-wx-token", &state.token)
            .send()
            .await?;
        ensure!(response.status() == StatusCode::OK, "list after submit");
        let list: Value = serde_json::from_slice(&response.bytes().await?)?;
        ensure!(
            list["tasks"].as_array().unwrap().len() == 1 && list["tasks"][0]["logs"] == json!([]),
            "authoritative summary list"
        );
        let response = client
            .post(format!("{}/api/tasks/{id}/cancel", state.origin))
            .header("x-wx-token", &state.token)
            .header("origin", &state.origin)
            .send()
            .await?;
        ensure!(response.status() == StatusCode::OK, "backend cancel");
        let cancelled: Value = serde_json::from_slice(&response.bytes().await?)?;
        ensure!(
            cancelled["status"] == "cancelled",
            "cancel did not reach service"
        );
        state.reconcile().await?;
        state.records.lock().unwrap().tasks[0].status = "synthetic-stale-projection".into();
        let response = client
            .get(format!("{}/api/tasks/{id}", state.origin))
            .header("x-wx-token", &state.token)
            .send()
            .await?;
        ensure!(response.status() == StatusCode::OK, "backend get");
        let task: Value = serde_json::from_slice(&response.bytes().await?)?;
        ensure!(
            task["status"] == "cancelled" && task["logs"] == json!([]),
            "HTTP Get used local task state"
        );
        for (cursor, expected_name, expected_reason) in
            [(0, "task", None), (u64::MAX, "reset", Some("reconnected"))]
        {
            let mut events = state.events.subscribe();
            let observed = async {
                loop {
                    let event = events.recv().await?;
                    if event.name == expected_name
                        && expected_reason.is_none_or(|reason| event.data["reason"] == reason)
                    {
                        return Ok::<_, anyhow::Error>(());
                    }
                }
            };
            tokio::time::timeout(Duration::from_secs(3), async {
                tokio::select! {
                    result = observed => result,
                    _ = poll_tasks(state.clone(), cursor) => anyhow::bail!("event poll stopped unexpectedly"),
                }
            }).await??;
            ensure!(
                state.records.lock().unwrap().tasks[0].status == "cancelled",
                "daemon events did not reconcile task projection"
            );
        }
        for seq in 0..260 {
            state.project_log(
                &json!({"task_id":id,"seq":seq,"stream":"stdout","text":"synthetic"}),
            )?;
        }
        state.project_log(&json!({"task_id":id,"seq":259,"stream":"stdout","text":"duplicate"}))?;
        ensure!(
            state
                .project_log(&json!({"task_id":id,"seq":262,"stream":"stdout","text":"gap"}))
                .is_err(),
            "log gap accepted"
        );
        {
            let records = state.records.lock().unwrap();
            let task = &records.tasks[0];
            ensure!(
                task.logs.len() == 256 && task.log_start_seq == 4 && task.next_log_seq == 260,
                "unbounded or duplicated log projection"
            );
        }
        let mut summary = {
            let records = state.records.lock().unwrap();
            serde_json::to_value(&records.tasks[0])?
        };
        summary["logs"] = json!([]);
        state.project_task(summary)?;
        ensure!(
            state.records.lock().unwrap().tasks[0].logs.len() == 256,
            "status summary discarded log tail"
        );
        state.reconcile().await?;
        ensure!(
            state.records.lock().unwrap().tasks[0].logs.is_empty(),
            "reconciliation did not replace logs"
        );
        let streams = state.streams.clone().acquire_many_owned(16).await?;
        let response = client
            .get(format!("{}/api/events", state.origin))
            .header("x-wx-token", &state.token)
            .send()
            .await?;
        ensure!(
            response.status() == StatusCode::TOO_MANY_REQUESTS,
            "SSE client limit"
        );
        response.bytes().await?;
        drop(streams);
        let mut response = client
            .get(format!("{}/api/events", state.origin))
            .header("x-wx-token", &state.token)
            .header("last-event-id", "1")
            .send()
            .await?;
        ensure!(response.status() == StatusCode::OK, "SSE reconnect");
        let ready = sse_frame(&mut response).await?;
        ensure!(
            ready.contains("event: reset"),
            "SSE reconnect omitted reset"
        );
        state.daemon_event("task", cancelled, 7);
        let delivered = sse_frame(&mut response).await?;
        ensure!(
            delivered.contains("event: task") && delivered.contains("id: 7"),
            "SSE lost daemon sequence"
        );
        drop(response);
        Ok(())
    }

    async fn http_checks(state: Arc<Shared>) -> Result<()> {
        read_queries::tests::check_http(&state).await?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .pool_max_idle_per_host(0)
            .build()?;
        for (path, content_type, expected) in [
            (
                "/",
                "text/html; charset=utf-8",
                include_bytes!("assets/index.html").as_slice(),
            ),
            (
                "/index.html",
                "text/html; charset=utf-8",
                include_bytes!("assets/index.html").as_slice(),
            ),
            (
                "/app.css",
                "text/css; charset=utf-8",
                include_bytes!("assets/app.css").as_slice(),
            ),
            (
                "/app.js",
                "text/javascript; charset=utf-8",
                include_bytes!("assets/app.js").as_slice(),
            ),
        ] {
            // 生产策略允许无令牌读取静态资源，但仍施加来源校验和安全响应头。
            let response = client.get(format!("{}{path}", state.origin)).send().await?;
            ensure!(response.status() == StatusCode::OK, "static status: {path}");
            ensure!(
                response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    == Some(content_type),
                "static MIME: {path}"
            );
            for (name, expected) in [
                ("cache-control", "no-store"),
                ("x-content-type-options", "nosniff"),
                ("x-frame-options", "DENY"),
                ("referrer-policy", "no-referrer"),
            ] {
                ensure!(
                    response.headers().get(name).and_then(|v| v.to_str().ok()) == Some(expected),
                    "missing security header: {name}"
                );
            }
            ensure!(
                response.headers().contains_key("content-security-policy"),
                "missing CSP"
            );
            let body = response.bytes().await?;
            ensure!(
                !body.is_empty() && body.as_ref() == expected,
                "static body: {path}"
            );
        }
        for path in ["/api/state", "/api/events", "/stream"] {
            for token in [None, Some("wrong-synthetic-token")] {
                let mut request = client.get(format!("{}{path}", state.origin));
                if let Some(token) = token {
                    request = request.header("x-wx-token", token);
                }
                let response = request.send().await?;
                ensure!(
                    response.status() == StatusCode::UNAUTHORIZED,
                    "token rejection: {path}"
                );
                let body: Value = serde_json::from_slice(&response.bytes().await?)?;
                ensure!(body["error"].is_string(), "missing rejection body");
            }
        }
        for (name, value) in [
            ("host", "untrusted.invalid"),
            ("origin", "http://untrusted.invalid"),
            ("sec-fetch-site", "cross-site"),
        ] {
            for path in ["/", "/api/state"] {
                let response = client
                    .get(format!("{}{path}", state.origin))
                    .header("x-wx-token", &state.token)
                    .header(name, value)
                    .send()
                    .await?;
                ensure!(
                    response.status() == StatusCode::FORBIDDEN,
                    "source rejection: {name} {path}"
                );
                response.bytes().await?;
            }
        }
        for bearer in [false, true] {
            let request = client.get(format!("{}/api/state", state.origin));
            let request = if bearer {
                request.bearer_auth(&state.token)
            } else {
                request.header("x-wx-token", &state.token)
            };
            let response = request.send().await?;
            ensure!(response.status() == StatusCode::OK, "state status");
            let bytes = response.bytes().await?;
            let body: Value = serde_json::from_slice(&bytes)?;
            ensure!(
                body["runtime_id"] == state.runtime.id
                    && body["engine"] == "rust"
                    && body["api_version"] == 1,
                "state identity"
            );
            ensure!(
                body["running"] == 0 && body.get("transcription").is_none(),
                "state fixture settings"
            );
            ensure!(
                !String::from_utf8_lossy(&bytes).contains(&state.token),
                "state leaked token"
            );
            ensure!(
                !String::from_utf8_lossy(&bytes).contains("SYNTHETIC_CONFIG_SECRET"),
                "state leaked config credential"
            );
        }
        for body in [
            r#"{"kind":"export_all","path":"injected"}"#,
            r#"{"kind":"export_all","options":{"command":"cmd.exe"}}"#,
            r#"{"kind":"shell"}"#,
        ] {
            let response = client
                .post(format!("{}/api/tasks", state.origin))
                .header("x-wx-token", &state.token)
                .header("origin", &state.origin)
                .header("content-type", "application/json")
                .body(body)
                .send()
                .await?;
            ensure!(
                response.status() == StatusCode::BAD_REQUEST,
                "unknown task fields accepted"
            );
            response.bytes().await?;
        }
        let response = client
            .get(format!("{}/api/tasks", state.origin))
            .header("x-wx-token", &state.token)
            .send()
            .await?;
        ensure!(response.status() == StatusCode::OK, "backend list");
        let body: Value = serde_json::from_slice(&response.bytes().await?)?;
        ensure!(
            body["tasks"] == json!([]) && body["history_persisted"] == true,
            "authoritative list"
        );
        task_checks(&client, &state).await?;
        // 无效自动图片请求必须在读取配置或启动后台前被拒绝。
        for (query, body) in [
            ("", ""),
            ("?source=message%2Fmessage_0.db", "unexpected-body"),
            ("?source=message%2Fmessage_0.db", ""),
        ] {
            let response = client
                .post(format!("{}/api/images/invalid/decode{query}", state.origin))
                .header("x-wx-token", &state.token)
                .header("origin", &state.origin)
                .body(body)
                .send()
                .await?;
            ensure!(
                response.status() == StatusCode::BAD_REQUEST,
                "invalid automatic image"
            );
            response.bytes().await?;
        }
        for (token, origin, status) in [
            (
                "wrong-synthetic-token",
                state.origin.as_str(),
                StatusCode::UNAUTHORIZED,
            ),
            (
                state.token.as_str(),
                "http://untrusted.invalid",
                StatusCode::FORBIDDEN,
            ),
        ] {
            let response = client
                .post(format!("{}/api/images/invalid/decode", state.origin))
                .header("x-wx-token", token)
                .header("origin", origin)
                .send()
                .await?;
            ensure!(
                response.status() == status,
                "automatic image security rejection"
            );
            response.bytes().await?;
        }
        let mut stopped = state.shutdown.subscribe();
        let response = client
            .post(format!("{}/api/shutdown", state.origin))
            .header("x-wx-token", &state.token)
            .send()
            .await?;
        ensure!(
            response.status() == StatusCode::FORBIDDEN,
            "shutdown without origin must fail"
        );
        response.bytes().await?;
        ensure!(!*stopped.borrow(), "rejected shutdown changed channel");
        let response = client
            .post(format!("{}/api/shutdown", state.origin))
            .header("x-wx-token", &state.token)
            .header("origin", &state.origin)
            .send()
            .await?;
        ensure!(response.status() == StatusCode::OK, "shutdown status");
        let body: Value = serde_json::from_slice(&response.bytes().await?)?;
        ensure!(body == json!({"stopping": true}), "shutdown response");
        tokio::time::timeout(Duration::from_secs(2), stopped.wait_for(|value| *value)).await??;
        Ok(())
    }

    #[tokio::test]
    async fn production_router_http_assets_security_state_and_shutdown() -> Result<()> {
        use std::{fs, future::IntoFuture};
        let root = tempfile::tempdir()?;
        let config_path = root.path().join("config.json");
        let config = crate::config::Config {
            key_store: None,
            db_dir: root.path().join("account/db_storage"),
            keys_file: root.path().join("keys.json"),
            decrypted_dir: root.path().join("decrypted"),
            wechat_process: "SyntheticNeverLaunched.exe".into(),
        };
        fs::create_dir_all(&config.db_dir)?;
        let mut config_json = serde_json::to_value(&config)?;
        config_json["openai_api_key"] = json!("SYNTHETIC_CONFIG_SECRET");
        let original = serde_json::to_vec(&config_json)?;
        fs::write(&config_path, &original)?;
        fs::write(&config.keys_file, b"{}")?;
        let runtime = crate::runtime::RuntimeContext::from_config(
            config_path.clone(),
            config,
            root.path().join("runtime"),
        )?;
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let (events, _) = broadcast::channel(256);
        let (shutdown, mut stop) = watch::channel(false);
        let state = Arc::new(Shared {
            runtime,
            token: "synthetic-http-test-token".into(),
            allow_plan_scan: false,
            allow_media_write: false,
            authority: address.to_string(),
            origin: format!("http://{address}"),
            records: Mutex::new(Records {
                tasks: VecDeque::new(),
                monitor_session: None,
                journal_ok: false,
            }),
            events,
            shutdown,
            queries: Arc::new(Semaphore::new(4)),
            query_waiters: Arc::new(Semaphore::new(8)),
            streams: Arc::new(Semaphore::new(16)),
            task_requests: Arc::new(Semaphore::new(4)),
        });
        let (backend_stop, mut backend) = mock_service(
            &state.runtime,
            crate::service::settings::Settings {
                ..Default::default()
            },
        )
        .await?;
        // Exercise the production Router and authenticated pipe; no monitor or browser.

        let server =
            axum::serve(listener, router(state.clone())).with_graceful_shutdown(async move {
                if !*stop.borrow() {
                    let _ = stop.changed().await;
                }
            });
        let mut server = tokio::spawn(server.into_future());
        let mut checks = tokio::spawn(http_checks(state.clone()));
        // 即使请求断言 panic 或超时，也先通知关闭并有界回收两个任务，再传播失败。
        let checked = tokio::time::timeout(Duration::from_secs(40), &mut checks).await;
        if checked.is_err() {
            checks.abort();
            let _ = tokio::time::timeout(Duration::from_secs(2), &mut checks).await;
        }
        let _ = state.shutdown.send(true);
        let served = tokio::time::timeout(Duration::from_secs(5), &mut server).await;
        if served.is_err() {
            server.abort();
            let _ = tokio::time::timeout(Duration::from_secs(2), &mut server).await;
        }
        let backend_survived = crate::service::client::request(&state.runtime, Call::Info {})
            .await
            .is_ok();
        let _ = backend_stop.send(true);
        let backend_result = tokio::time::timeout(Duration::from_secs(3), &mut backend).await;
        if backend_result.is_err() {
            backend.abort();
            let _ = backend.await;
        }
        let unchanged = fs::read(&config_path)? == original;
        let no_history = !state.runtime.directory.join("web-history.json").exists();
        let no_outputs = !state.runtime.config.decrypted_dir.exists();
        drop(state);
        root.close()?;
        checked.context("HTTP checks timed out")???;
        served.context("HTTP server did not shut down in time")???;
        backend_result.context("Mock service did not shut down")???;
        ensure!(
            unchanged && no_history && no_outputs && backend_survived,
            "HTTP checks changed fixture data or started runtime work"
        );
        Ok(())
    }

    #[test]
    fn service_error_codes_are_mapped_without_exposing_messages() {
        use crate::service::protocol::ServiceError;
        for (code, expected) in [
            ("invalid_request", StatusCode::BAD_REQUEST),
            ("not_found", StatusCode::NOT_FOUND),
            ("conflict", StatusCode::CONFLICT),
            ("queue_full", StatusCode::TOO_MANY_REQUESTS),
            ("internal", StatusCode::SERVICE_UNAVAILABLE),
        ] {
            let result = backend_error(ServiceError::new(code, "SYNTHETIC_SECRET").into());
            assert_eq!(result.0, expected);
            assert!(!result.1.contains("SYNTHETIC_SECRET"));
        }
    }

    #[test]
    fn tokens_and_query_limits() {
        assert!(token_equal("abc", "abc"));
        assert!(!token_equal("abc", "abd"));
        assert!(!token_equal("a", "aa"));
        assert!(Filter {
            limit: 2001,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(Filter {
            source: "other".into(),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(Filter::default().validate().is_ok());
    }

    #[test]
    fn query_filter_accepts_chat_and_rejects_username_alias() {
        let uri = "/api/history?chat=synthetic-peer".parse().unwrap();
        let Query(filter) = Query::<Filter>::try_from_uri(&uri).unwrap();
        assert_eq!(filter.chat.as_deref(), Some("synthetic-peer"));

        let uri = "/api/history?username=synthetic-peer".parse().unwrap();
        assert!(Query::<Filter>::try_from_uri(&uri).is_err());
    }
}
