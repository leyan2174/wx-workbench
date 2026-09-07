//! Production HTTP and named-pipe composition against an isolated synthetic account.
#![cfg(windows)]

use super::server_types::{Records, Settings, Shared};
use crate::{
    attachment::{decoder, AttachmentId, AttachmentKind},
    ipc::{Request, Response},
};
use anyhow::{ensure, Context, Result};
use interprocess::local_socket::{tokio::prelude::*, GenericNamespaced, ListenerOptions};
use serde_json::{json, Value};
use std::{
    fs,
    future::IntoFuture,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{broadcast, watch, Semaphore},
};

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
        db_dir: root.path().join("account/db_storage"),
        keys_file: root.path().join("keys.json"),
        decrypted_dir: root.path().join("decrypted"),
        wechat_process: "SyntheticNeverLaunched.exe".into(),
    };
    fs::create_dir_all(&config.db_dir)?;
    let mut config_json = serde_json::to_value(&config)?;
    config_json["image_aes_key"] = json!(std::str::from_utf8(AES)?);
    config_json["image_xor_key"] = json!(XOR);
    let original = serde_json::to_vec(&config_json)?;
    fs::write(&config_path, &original)?;
    fs::write(&config.keys_file, b"{}")?;
    let runtime = crate::runtime::RuntimeContext::from_config(
        config_path.clone(),
        config,
        root.path().join("runtime"),
    )?;
    let pipe = runtime.pipe_name();
    let ipc_listener = ListenerOptions::new()
        .name(pipe.to_ns_name::<GenericNamespaced>()?)
        .create_tokio()?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let address = listener.local_addr()?;
    let (events, _) = broadcast::channel(8);
    let (shutdown, mut stop) = watch::channel(false);
    let state = Arc::new(Shared {
        runtime,
        settings: Settings::default(),
        token: "synthetic-image-runtime-token".into(),
        authority: address.to_string(),
        origin: format!("http://{address}"),
        records: Mutex::new(Records {
            tasks: Default::default(),
            messages: Default::default(),
            message_bytes: 0,
            journal_ok: false,
            enterprise_snapshot: None,
        }),
        events,
        shutdown,
        queries: Arc::new(Semaphore::new(2)),
        streams: Arc::new(Semaphore::new(1)),
        task_requests: Arc::new(Semaphore::new(4)),
    });
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
    let mut ipc = tokio::spawn(async move {
        let stream = ipc_listener.accept().await?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let wire: Value = serde_json::from_str(&line)?;
        ensure!(
            wire["cmd"] == "decode_image" && wire.as_object().is_some_and(|map| map.len() == 6),
            "unexpected IPC command or fields"
        );
        let Request::DecodeImage {
            chat,
            local_id,
            create_time,
            output_root,
            image_key_file,
        } = serde_json::from_str::<Request>(&line)?
        else {
            anyhow::bail!("expected actual DecodeImage request");
        };
        ensure!(
            chat == CHAT && local_id == LOCAL_ID && create_time == TIMESTAMP,
            "IPC lost exact message identity"
        );
        let output = PathBuf::from(output_root);
        let key = PathBuf::from(image_key_file.context("missing private image key path")?);
        let temporary = output.parent().context("output parent missing")?.to_owned();
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
        ensure!(
            key == temporary.join("image-key.json") && key.is_file(),
            "key not prepared"
        );
        ensure!(
            !temporary.canonicalize()?.starts_with(&account_fixture),
            "automatic output must be outside account/config fixture"
        );
        crate::toolkit::private_file::assert_private_acl(&key);
        // Use the real consumer's ASCII parser, never a fixture-specific hex decoder.
        let key_json: Value = serde_json::from_slice(&fs::read(&key)?)?;
        let aes = crate::toolkit::parse_image_aes(
            key_json["aes_key"]
                .as_str()
                .context("missing AES material")?,
        )?;
        let xor = crate::toolkit::parse_image_xor(&key_json["xor_key"].to_string())?;
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
        reader.get_mut().write_all(frame.as_bytes()).await?;
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
        tokio::time::timeout(Duration::from_secs(3), &mut ipc)
            .await
            .context("synthetic IPC did not finish")???;
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
            !state.runtime.root.exists(),
            "unexpected daemon/runtime work"
        );
        ensure!(
            !state.runtime.config.decrypted_dir.exists(),
            "unexpected account output"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await;
    // Always reap IPC/HTTP tasks before propagating a failed check or dropping pinned files.
    if !ipc.is_finished() {
        ipc.abort();
        let _ = ipc.await;
    }
    let _ = state.shutdown.send(true);
    let served = tokio::time::timeout(Duration::from_secs(5), &mut server).await;
    if served.is_err() {
        server.abort();
        let _ = server.await;
    }
    let drained = super::automatic_image::drain().await;
    drop(state);
    let closed = root.close();
    checked.context("automatic image HTTP checks timed out")??;
    served.context("automatic image HTTP server did not stop")???;
    ensure!(drained, "automatic image cleanup did not drain");
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
