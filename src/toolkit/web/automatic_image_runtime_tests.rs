//! Production HTTP and named-pipe composition against an isolated synthetic account.
#![cfg(windows)]

use super::server_types::{Records, Shared};
use crate::{
    attachment::{decoder, AttachmentId, AttachmentKind},
    ipc::{Request, Response},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    future::IntoFuture,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{broadcast, watch, Semaphore};

const CHAT: &str = "wxid_automatic_image_fixture";
const LOCAL_ID: i64 = 71;
const TIMESTAMP: i64 = 1_700_000_123;
const SOURCE: &str = "message/message_0.db";
const AES: &[u8; 16] = b"Ab\"\\cdEF01234567";
const XOR: u8 = 0xa2;
// Complete 1x1 RGBA PNG, encoded by System.Drawing, including chunk CRCs and zlib data.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x01, 0x73, 0x52, 0x47, 0x42, 0x00, 0xae, 0xce, 0x1c, 0xe9, 0x00, 0x00,
    0x00, 0x04, 0x67, 0x41, 0x4d, 0x41, 0x00, 0x00, 0xb1, 0x8f, 0x0b, 0xfc, 0x61, 0x05, 0x00, 0x00,
    0x00, 0x09, 0x70, 0x48, 0x59, 0x73, 0x00, 0x00, 0x0e, 0xc3, 0x00, 0x00, 0x0e, 0xc3, 0x01, 0xc7,
    0x6f, 0xa8, 0x64, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x18, 0x57, 0x63, 0x60, 0x48,
    0x58, 0xf0, 0x1f, 0x00, 0x03, 0x64, 0x02, 0x00, 0xe6, 0xeb, 0x11, 0xec, 0x00, 0x00, 0x00, 0x00,
    0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];
static CASES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone)]
struct PublishedPaths {
    temporary: PathBuf,
    output: PathBuf,
    key: PathBuf,
    image: PathBuf,
}

async fn run_case(wrong_source: bool) -> Result<()> {
    let _case = CASES.lock().await;
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
    let original = serde_json::to_vec(&config)?;
    fs::write(&config_path, &original)?;
    fs::write(&config.keys_file, b"{}")?;
    let runtime = crate::runtime::RuntimeContext::from_config(
        config_path.clone(),
        config,
        root.path().join("runtime"),
    )?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let store = crate::key_store::Store::for_runtime(&runtime)?;
    store.update(
        Some(0),
        &[crate::key_store::Update::Image(
            AES,
            XOR,
            crate::key_store::Verification::Verified,
        )],
    )?;
    let address = listener.local_addr()?;
    let (events, _) = broadcast::channel(8);
    let (shutdown, mut stop) = watch::channel(false);
    let state = Arc::new(Shared {
        runtime,
        token: "synthetic-image-runtime-token".into(),
        authority: address.to_string(),
        origin: format!("http://{address}"),
        records: Mutex::new(Records {
            tasks: Default::default(),
            monitor_session: None,
            journal_ok: false,
        }),
        events,
        shutdown,
        queries: Arc::new(Semaphore::new(2)),
        query_waiters: Arc::new(Semaphore::new(8)),
        streams: Arc::new(Semaphore::new(1)),
        task_requests: Arc::new(Semaphore::new(4)),
    });
    let daemon = crate::daemon::web_service::WebService::new(
        state.runtime.clone(),
        Arc::new(crate::daemon::query_state::QueryState::new(
            state.runtime.clone(),
        )),
    );
    let (query_tx, mut query_rx) = tokio::sync::mpsc::channel(1);
    *daemon.query_fixture.lock().unwrap() = Some(query_tx);
    let (backend_stop, mut backend) = start_service(&state.runtime, daemon.clone()).await?;
    let id = AttachmentId {
        v: 1,
        chat: CHAT.into(),
        local_id: LOCAL_ID,
        create_time: TIMESTAMP,
        kind: AttachmentKind::Image,
        db: None,
    };
    let url = format!(
        "{}/api/images/{}/decode?source=message%2Fmessage_0.db",
        state.origin,
        id.encode()?
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(25))
        .pool_max_idle_per_host(0)
        .build()?;
    let published = Arc::new(Mutex::new(None::<PublishedPaths>));
    let capture = published.clone();
    let account_fixture = root.path().canonicalize()?;
    let mut database = tokio::spawn(async move {
        let (request, reply_sender) = query_rx
            .recv()
            .await
            .context("missing daemon image query")?;
        let Request::DecodeImage {
            chat,
            local_id,
            create_time,
            output_root,
            image_key_file,
        } = request
        else {
            anyhow::bail!("expected actual DecodeImage request");
        };
        ensure!(
            chat == CHAT && local_id == LOCAL_ID && create_time == TIMESTAMP,
            "IPC lost exact message identity"
        );
        let output = PathBuf::from(output_root);
        ensure!(
            image_key_file.is_none(),
            "automatic request leaked a plaintext key path"
        );
        let temporary = output.parent().context("output parent missing")?.to_owned();
        let key = temporary.join("image-key.json");
        let image = output.join(format!("{:x}.png", md5::compute(PNG)));
        *capture.lock().unwrap() = Some(PublishedPaths {
            temporary: temporary.clone(),
            output: output.clone(),
            key: key.clone(),
            image: image.clone(),
        });
        ensure!(
            output.is_absolute() && output.is_dir(),
            "output root not prepared"
        );
        ensure!(!key.exists(), "automatic decoding wrote plaintext keys");
        ensure!(
            !temporary.canonicalize()?.starts_with(&account_fixture),
            "automatic output must be outside account/config fixture"
        );
        crate::toolkit::private_file::assert_private_acl(store.path());
        let (aes, xor) = store.load()?.image_material();
        let aes = aes.context("missing encrypted AES material")?;
        ensure!(aes == *AES && xor == XOR, "consumer key bytes changed");
        let dat: Vec<_> = PNG.iter().map(|byte| byte ^ XOR).collect();
        let decoded = decoder::dispatch(
            &dat,
            decoder::V2KeyMaterial {
                aes_key: Some(&aes),
                xor_key: xor,
            },
        )?;
        ensure!(
            decoded.data == PNG && decoded.format == "png" && decoded.decoder == "legacy_xor",
            "native decoder did not produce the complete PNG"
        );
        // Stub only daemon resolution/reporting; HTTP and host publication checks are production.
        fs::write(&image, &decoded.data)?;
        let reply = Response::ok(json!({
            "exit_code": 0,
            "status": "published",
            "image": {
                "message": {
                    "username": chat, "local_id": local_id, "create_time": create_time,
                    "local_type": 3,
                    "source": if wrong_source { "message/message_1.db" } else { SOURCE }
                },
                "decoded_md5": format!("{:x}", md5::compute(&decoded.data)),
                "path": image,
                "size": decoded.data.len(),
                "format": decoded.format
            }
        }));
        let frame = serde_json::to_string(&reply)? + "\n";
        let flattened: Value = serde_json::from_str(&frame)?;
        ensure!(
            flattened["ok"] == true && flattened.get("data").is_none(),
            "IPC must be flat"
        );
        reply_sender
            .send(reply)
            .map_err(|_| anyhow::anyhow!("daemon dropped image query"))?;
        Ok::<_, anyhow::Error>(())
    });
    // Router only: no production serve lifecycle, worker, monitor, daemon or browser.
    let server =
        axum::serve(listener, super::router(state.clone())).with_graceful_shutdown(async move {
            if !*stop.borrow() {
                let _ = stop.changed().await;
            }
        });
    let mut server = tokio::spawn(server.into_future());
    let checked = tokio::time::timeout(Duration::from_secs(35), async {
        let request = || {
            client
                .post(&url)
                .header("x-wx-token", &state.token)
                .header("origin", &state.origin)
        };
        let rejected = request().body("unexpected-body").send().await?;
        ensure!(
            rejected.status() == reqwest::StatusCode::BAD_REQUEST,
            "nonempty body accepted"
        );
        rejected.bytes().await?;
        ensure!(
            published.lock().unwrap().is_none(),
            "rejected body reached DecodeImage"
        );
        let response = request().body("").send().await?;
        let status = response.status();
        let mime = response
            .headers()
            .get("content-type")
            .context("HTTP MIME missing")?
            .to_str()?
            .to_owned();
        ensure!(
            response
                .headers()
                .get("cache-control")
                .is_some_and(|v| v == "no-store"),
            "cache header"
        );
        ensure!(
            response
                .headers()
                .get("x-content-type-options")
                .is_some_and(|v| v == "nosniff"),
            "sniff header"
        );
        let body = response.bytes().await?;
        tokio::time::timeout(Duration::from_secs(3), &mut database).await???;
        if wrong_source {
            ensure!(
                status == reqwest::StatusCode::SERVICE_UNAVAILABLE,
                "wrong source was accepted"
            );
            ensure!(
                mime == "application/json" && body.as_ref() != PNG,
                "image bytes escaped rejection"
            );
            let error: Value = serde_json::from_slice(&body)?;
            ensure!(
                error["error"]["code"] == "decode_failed",
                "wrong rejection code"
            );
            ensure!(
                error.get("image").is_none(),
                "rejection included image report"
            );
        } else {
            ensure!(
                status == reqwest::StatusCode::OK,
                "positive decode HTTP status: {status}"
            );
            ensure!(
                mime == "image/png" && body.as_ref() == PNG,
                "HTTP PNG bytes or MIME changed"
            );
        }
        let paths = published
            .lock()
            .unwrap()
            .clone()
            .context("IPC did not receive decode")?;
        for path in [&paths.key, &paths.image, &paths.output, &paths.temporary] {
            ensure!(
                !path.exists(),
                "automatic temporary artifact survived HTTP completion"
            );
        }
        ensure!(fs::read(&config_path)? == original, "config changed");
        ensure!(
            fs::read(&state.runtime.config.keys_file)? == b"{}",
            "account keys changed"
        );
        ensure!(
            fs::read_dir(&state.runtime.config.db_dir)?.next().is_none(),
            "account data changed"
        );
        ensure!(
            !state.runtime.cache_dir().exists(),
            "unexpected query cache work"
        );
        ensure!(
            !state.runtime.config.decrypted_dir.exists(),
            "unexpected account output"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await;
    // Always reap IPC/HTTP tasks before propagating a failed check or dropping pinned files.
    let _ = state.shutdown.send(true);
    let served = tokio::time::timeout(Duration::from_secs(5), &mut server).await;
    if served.is_err() {
        server.abort();
        let _ = server.await;
    }
    if !database.is_finished() {
        database.abort();
        let _ = database.await;
    }
    let drained = daemon.shutdown().await;
    backend_stop.send_replace(true);
    let backend_result = tokio::time::timeout(Duration::from_secs(3), &mut backend).await;
    if backend_result.is_err() {
        backend.abort();
        let _ = backend.await;
    }
    drop(daemon);
    drop(state);
    let closed = root.close();
    checked.context("automatic image HTTP checks timed out")??;
    served.context("automatic image HTTP server did not stop")???;
    drained?;
    backend_result.context("fixture service failed to drain")???;
    closed?;
    Ok(())
}

#[tokio::test]
async fn production_http_decode_consumes_private_key_publishes_png_and_cleans_up() -> Result<()> {
    run_case(false).await
}

#[tokio::test]
async fn production_http_decode_rejects_wrong_report_source_and_cleans_up() -> Result<()> {
    run_case(true).await
}

async fn start_service(
    runtime: &crate::runtime::RuntimeContext,
    business: Arc<crate::daemon::web_service::WebService>,
) -> Result<(watch::Sender<bool>, tokio::task::JoinHandle<Result<()>>)> {
    use crate::service::protocol::{Call, ServiceError};
    use windows::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };
    let (mut birth, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut birth,
            &mut exit,
            &mut kernel,
            &mut user,
        )?;
    }
    fs::create_dir_all(&runtime.directory)?;
    fs::write(
        runtime.pid_path(),
        serde_json::to_vec(&json!({
            "pid": std::process::id(), "exe": std::env::current_exe()?,
            "created": (u64::from(birth.dwHighDateTime) << 32) | u64::from(birth.dwLowDateTime),
            "runtime_id": runtime.id,
        }))?,
    )?;
    let handler = Arc::new(move |call| {
        let business = business.clone();
        async move {
            match call {
                Call::Info {} => Ok(json!({})),
                Call::Web { request } => {
                    ensure_web_call(&request).map_err(|_| {
                        ServiceError::new("invalid_request", "invalid fixture request")
                    })?;
                    business.handle(*request).await
                }
                _ => Err(ServiceError::new(
                    "invalid_request",
                    "unexpected fixture call",
                )),
            }
        }
    });
    let (shutdown, stop) = watch::channel(false);
    let mut handle = tokio::spawn(crate::service::transport::serve(
        runtime.clone(),
        handler,
        stop,
    ));
    let ready = crate::service::client::wait_ready(runtime).await;
    if let Err(error) = ready {
        shutdown.send_replace(true);
        if tokio::time::timeout(Duration::from_secs(3), &mut handle)
            .await
            .is_err()
        {
            handle.abort();
            let _ = handle.await;
        }
        return Err(error);
    }
    Ok((shutdown, handle))
}

fn ensure_web_call(call: &crate::service::web::Call) -> Result<()> {
    ensure!(
        matches!(call, crate::service::web::Call::DecodeImage { .. }),
        "unexpected Web operation"
    );
    let wire = serde_json::to_string(call)?;
    ensure!(
        !wire.contains("image_key_file") && !wire.contains("output_root"),
        "Web supplied business paths"
    );
    Ok(())
}
