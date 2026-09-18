use super::*;
use crate::service::web::Failure;

#[tokio::test]
async fn structured_query_ambiguity_survives_web_rpc_without_private_details() -> Result<()> {
    let (_root, state) = fixture()?;
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    *state.query_fixture.lock().unwrap() = Some(send);
    for (_, wire) in crate::service::web::read_query_cases() {
        let decode = wire["op"].as_str().unwrap().starts_with("decode_");
        for explicit_status in [false, true] {
            let call: Call = serde_json::from_value(wire.clone())?;
            let check = async {
                let (_, reply) = receive.recv().await.unwrap();
                let mut data = json!({"exit_code":2,"text":"SYNTHETIC_PRIVATE_XML_OR_PATH"});
                if explicit_status {
                    data["status"] = json!("ambiguous");
                }
                reply.send(Response::ok(data)).unwrap();
            };
            let (result, ()) = tokio::join!(state.handle(call), check);
            let error = result.unwrap_err();
            assert_eq!(
                error.code,
                if decode || explicit_status {
                    crate::service::web::QUERY_AMBIGUOUS_CODE
                } else {
                    "business_failed"
                }
            );
            assert!(!error.message.contains("SYNTHETIC_PRIVATE"));
        }
    }
    Ok(())
}

// These exercise the production WebService dispatch with an observable query
// boundary, not a claim that a synthetic reply is a real database result.
#[tokio::test]
async fn read_routes_dispatch_every_parameter_and_preserve_business_data() -> Result<()> {
    let (_root, state) = fixture()?;
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    *state.query_fixture.lock().unwrap() = Some(send);
    for (path, wire) in crate::service::web::read_query_cases() {
        let call: Call = serde_json::from_value(wire.clone())?;
        let mut expected = wire;
        let op = expected.as_object_mut().unwrap().remove("op").unwrap();
        expected["cmd"] = op;
        let expected: Request = serde_json::from_value(expected)?;
        let expected = serde_json::to_value(expected)?;
        let payload = json!({"partial":true,"has_more":true,"issues":["synthetic"],
            "meta":{"coverage_proven":false},"fixture_path":path});
        let check = async {
            let (request, reply) = receive.recv().await.unwrap();
            assert_eq!(serde_json::to_value(request).unwrap(), expected, "{path}");
            reply.send(Response::ok(payload.clone())).unwrap();
        };
        let (result, ()) = tokio::join!(state.handle(call), check);
        assert_eq!(result?, payload, "{path}");
    }
    Ok(())
}

#[tokio::test]
async fn read_routes_preserve_business_failure_and_partial_classification() -> Result<()> {
    let (_root, state) = fixture()?;
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    *state.query_fixture.lock().unwrap() = Some(send);
    for (_, wire) in crate::service::web::read_query_cases() {
        for (status, code) in [
            ("refused", "business_refused"),
            ("partial", "business_partial"),
            ("error", "business_failed"),
        ] {
            let call: Call = serde_json::from_value(wire.clone())?;
            let check = async {
                let (_, reply) = receive.recv().await.unwrap();
                reply
                    .send(Response::ok(
                        json!({"status":status,"text":"SYNTHETIC_PRIVATE_DETAIL"}),
                    ))
                    .unwrap();
            };
            let (result, ()) = tokio::join!(state.handle(call), check);
            let error = result.unwrap_err();
            assert_eq!(error.code, code);
            assert!(!error.message.contains("SYNTHETIC_PRIVATE_DETAIL"));
        }
    }
    Ok(())
}

#[tokio::test]
async fn read_route_invalid_calls_never_reach_query_dispatch() -> Result<()> {
    let (_root, state) = fixture()?;
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    *state.query_fixture.lock().unwrap() = Some(send);
    for (_, mut wire) in crate::service::web::read_query_cases() {
        let key = if wire.get("create_time").is_some() {
            "create_time"
        } else if wire.get("limit").is_some() {
            "limit"
        } else {
            "chat"
        };
        wire[key] = if key == "chat" { json!("") } else { json!(0) };
        let call: Call = serde_json::from_value(wire)?;
        assert!(state.handle(call).await.is_err());
        assert!(receive.try_recv().is_err());
    }
    Ok(())
}

fn fixture() -> Result<(tempfile::TempDir, Arc<WebService>)> {
    let root = tempfile::tempdir()?;
    let config_path = root.path().join("config.json");
    let config = crate::config::Config {
        key_store: Some(root.path().join("keys.dpapi")),
        db_dir: root.path().join("account/db_storage"),
        keys_file: root.path().join("keys.json"),
        decrypted_dir: root.path().join("decrypted"),
        wechat_process: "SyntheticNeverLaunched.exe".into(),
    };
    std::fs::create_dir_all(&config.db_dir)?;
    std::fs::write(&config_path, serde_json::to_vec(&config)?)?;
    std::fs::write(&config.keys_file, b"{}")?;
    let runtime = RuntimeContext::from_config(config_path, config, root.path().join("runtime"))?;
    crate::key_store::Store::for_runtime(&runtime)?.update(
        Some(0),
        &[crate::key_store::Update::Image(
            b"syntheticAESkey1",
            0xa2,
            crate::key_store::Verification::Verified,
        )],
    )?;
    let query = Arc::new(crate::daemon::query_state::QueryState::new(runtime.clone()));
    Ok((root, WebService::new(runtime, query)))
}

async fn assert_pending<F: std::future::Future>(mut future: std::pin::Pin<&mut F>) {
    std::future::poll_fn(|context| {
        assert!(future.as_mut().poll(context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn query_waiters_are_call_bounded_and_only_four_dispatch_after_release() -> Result<()> {
    let (_root, state) = fixture()?;
    let held = state.queries.clone().acquire_many_owned(4).await?;
    let (send, mut receive) = tokio::sync::mpsc::channel(32);
    *state.query_fixture.lock().unwrap() = Some(send);
    let mut waiting = Vec::new();
    for _ in 0..32 {
        let mut call = Box::pin(state.handle(Call::Tags { name: None }));
        assert_pending(call.as_mut()).await;
        waiting.push(call);
    }
    assert_eq!(state.calls.available_permits(), 0);
    assert_eq!(
        state
            .handle(Call::Tags { name: None })
            .await
            .unwrap_err()
            .code,
        "busy"
    );
    assert!(receive.try_recv().is_err());
    drop(held);
    for call in &mut waiting {
        assert_pending(call.as_mut()).await;
    }
    assert_eq!(state.queries.available_permits(), 0);
    for _ in 0..4 {
        let (request, reply) = receive.try_recv()?;
        assert!(matches!(request, Request::ContactTags));
        reply.send(Response::ok(json!({"tags":[]}))).unwrap();
    }
    assert!(receive.try_recv().is_err());
    for call in waiting.drain(..4) {
        assert_eq!(call.await?["tags"], json!([]));
    }
    drop(waiting);
    assert_eq!(state.calls.available_permits(), 32);
    assert_eq!(state.queries.available_permits(), 4);
    assert!(receive.try_recv().is_err());
    Ok(())
}

#[tokio::test]
async fn query_wait_timeout_is_busy_and_releases_call_without_dispatch() -> Result<()> {
    let (_root, state) = fixture()?;
    let held = state.queries.clone().acquire_many_owned(4).await?;
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    *state.query_fixture.lock().unwrap() = Some(send);
    let error = state.handle(Call::Tags { name: None }).await.unwrap_err();
    assert_eq!(error.code, "busy");
    assert_eq!(error.message, "Web business service is busy");
    assert_eq!(state.calls.available_permits(), 32);
    assert!(receive.try_recv().is_err());
    drop(held);
    assert_eq!(state.queries.available_permits(), 4);
    Ok(())
}

#[tokio::test]
async fn cancelled_query_waiter_returns_call_and_never_consumes_later_query() -> Result<()> {
    let (_root, state) = fixture()?;
    let held = state.queries.clone().acquire_many_owned(4).await?;
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    *state.query_fixture.lock().unwrap() = Some(send);
    let mut call = Box::pin(state.handle(Call::Tags { name: None }));
    assert_pending(call.as_mut()).await;
    assert_eq!(state.calls.available_permits(), 31);
    drop(call);
    assert_eq!(state.calls.available_permits(), 32);
    drop(held);
    let mut next = Box::pin(state.handle(Call::Tags { name: None }));
    assert_pending(next.as_mut()).await;
    let (request, reply) = receive.try_recv()?;
    assert!(matches!(request, Request::ContactTags));
    reply.send(Response::ok(json!({"tags":[]}))).unwrap();
    assert_eq!(next.await?["tags"], json!([]));
    assert!(receive.try_recv().is_err());
    assert_eq!(state.calls.available_permits(), 32);
    assert_eq!(state.queries.available_permits(), 4);
    Ok(())
}

#[tokio::test]
async fn handle_nonbusy_errors_remain_distinct_and_redacted() -> Result<()> {
    let (_root, state) = fixture()?;
    let invalid = state
        .handle(Call::Tags {
            name: Some("PRIVATE\nSECRET".into()),
        })
        .await
        .unwrap_err();
    assert_eq!(invalid.code, "unavailable");
    assert_eq!(invalid.message, "Web business operation unavailable");
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    *state.query_fixture.lock().unwrap() = Some(send);
    let mut call = Box::pin(state.handle(Call::Tags { name: None }));
    assert_pending(call.as_mut()).await;
    let (_, reply) = receive.try_recv()?;
    reply
        .send(Response::err("PRIVATE path SECRET key 查询繁忙"))
        .unwrap();
    let business = call.await.unwrap_err();
    assert_eq!(business.code, "business_failed");
    assert_eq!(business.message, "Business operation failed");
    assert!(!format!("{business:?}").contains("PRIVATE"));
    assert!(!format!("{business:?}").contains("SECRET"));
    assert_eq!(state.calls.available_permits(), 32);
    assert_eq!(state.queries.available_permits(), 4);
    Ok(())
}

#[tokio::test]
async fn image_business_codes_keep_missing_distinct_from_export_failure_and_clean_up() -> Result<()>
{
    for (code, expected) in [
        (1, Failure::Unavailable),
        (2, Failure::Ambiguous),
        (3, Failure::DecodeFailed),
    ] {
        let (_root, state) = fixture()?;
        let (send, mut receive) = tokio::sync::mpsc::channel(1);
        *state.query_fixture.lock().unwrap() = Some(send);
        let id = crate::attachment::AttachmentId {
            v: 1,
            chat: "alice".into(),
            local_id: 7,
            create_time: 123,
            kind: crate::attachment::AttachmentKind::Image,
            db: None,
        };
        let owner = state.clone();
        let task = tokio::spawn(async move {
            owner
                .handle(Call::DecodeImage {
                    encoded: id.encode().unwrap(),
                    source: "message/message_0.db".into(),
                })
                .await
        });
        let (request, reply) = tokio::time::timeout(Duration::from_secs(3), receive.recv())
            .await?
            .unwrap();
        let Request::DecodeImage {
            output_root,
            image_key_file,
            ..
        } = request
        else {
            anyhow::bail!("unexpected query");
        };
        let output = std::path::PathBuf::from(output_root);
        assert!(image_key_file.is_none());
        let key = output.parent().unwrap().join("image-key.json");
        assert!(
            !key.exists(),
            "automatic decoding must not write plaintext keys"
        );
        reply
            .send(Response::ok(
                json!({"exit_code":code,"status":"error","message":"PRIVATE SECRET"}),
            ))
            .unwrap();
        let result = task.await??;
        assert_eq!(result, json!({"failure":expected}));
        assert!(!output.parent().unwrap().exists());
        assert!(!key.exists());
        assert_eq!(state.calls.available_permits(), 32);
        assert_eq!(state.queries.available_permits(), 4);
        assert_eq!(state.decodes.available_permits(), 2);
    }
    Ok(())
}

#[tokio::test]
async fn monitor_history_is_daemon_owned_bounded_and_launch_scoped() -> Result<()> {
    let (_root, state) = fixture()?;
    state.event("message", json!({"local_id":0,"timestamp":0}));
    let start = state.records.lock().unwrap().cursor;
    state
        .records
        .lock()
        .unwrap()
        .sessions
        .push_back(("launch".into(), start));
    for id in 1..=2100 {
        state.event(
            "message",
            json!({
                "local_id":id,"timestamp":id,"web_delivery":{"committed":true,"replay":false}
            }),
        );
    }
    let value = state
        .handle(Call::MonitorHistory {
            session: "launch".into(),
            limit: 2,
            offset: 1,
            since: Some(2097),
        })
        .await?;
    assert_eq!(value["scope"], "launch_monitor");
    assert_eq!(value["messages"][0]["local_id"], 2099);
    assert_eq!(value["messages"][1]["local_id"], 2098);
    assert!(value["messages"][0].get("web_delivery").is_none());
    assert_eq!(state.records.lock().unwrap().messages.len(), 2000);
    assert_eq!(state.records.lock().unwrap().events.len(), 256);
    assert_eq!(state.events(start)["reset"], true);
    let cursor = state.records.lock().unwrap().cursor;
    assert_eq!(state.events(cursor)["events"], json!([]));
    assert_eq!(state.events(cursor + 1)["reset"], true);
    assert!(state
        .handle(Call::MonitorHistory {
            session: "another-daemon".into(),
            limit: 1,
            offset: 0,
            since: None,
        })
        .await
        .is_err());
    assert!(state
        .handle(Call::MonitorEvents {
            epoch: "another-daemon".into(),
            after: 0,
        })
        .await
        .is_err());
    state.shutdown().await?;
    assert!(state.handle(Call::MonitorOpen {}).await.is_err());
    Ok(())
}

#[tokio::test]
async fn event_pages_advance_only_to_delivered_events() -> Result<()> {
    let (_root, state) = fixture()?;
    for id in 0..200 {
        state.event("monitor_status", json!({"status":"ready","id":id}));
    }
    let first = state.events(0);
    assert_eq!(first["events"].as_array().unwrap().len(), 128);
    assert_eq!(first["cursor"], 128);
    let second = state.events(128);
    assert_eq!(second["events"].as_array().unwrap().len(), 72);
    assert_eq!(second["events"][0]["seq"], 129);
    assert_eq!(second["cursor"], 200);
    state.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn cancelled_image_waiter_does_not_own_cleanup_or_daemon_shutdown() -> Result<()> {
    let (_root, state) = fixture()?;
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    *state.query_fixture.lock().unwrap() = Some(send);
    let id = crate::attachment::AttachmentId {
        v: 1,
        chat: "alice".into(),
        local_id: 7,
        create_time: 123,
        kind: crate::attachment::AttachmentKind::Image,
        db: None,
    };
    let worker = state.clone();
    let waiter = tokio::spawn(async move {
        worker
            .handle(Call::DecodeImage {
                encoded: id.encode().unwrap(),
                source: "message/message_0.db".into(),
            })
            .await
    });
    let (request, reply) = tokio::time::timeout(Duration::from_secs(3), receive.recv())
        .await?
        .ok_or_else(|| anyhow::anyhow!("image query missing"))?;
    let Request::DecodeImage {
        output_root,
        image_key_file,
        ..
    } = request
    else {
        anyhow::bail!("unexpected internal query");
    };
    assert!(image_key_file.is_none());
    let temporary = std::path::PathBuf::from(output_root)
        .parent()
        .unwrap()
        .to_owned();
    let key = temporary.join("image-key.json");
    assert!(!key.exists());
    assert!(temporary.is_dir());
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(
        temporary.is_dir(),
        "HTTP cancellation released daemon's work directory"
    );
    assert!(
        !key.exists(),
        "HTTP cancellation created a plaintext key file"
    );
    let owner = state.clone();
    let mut stopping = tokio::spawn(async move { owner.shutdown().await });
    tokio::task::yield_now().await;
    assert!(
        !stopping.is_finished(),
        "daemon shutdown skipped active decode"
    );
    reply
        .send(Response::ok(json!({"exit_code":1,"status":"error"})))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), &mut stopping).await???;
    assert!(
        !temporary.exists(),
        "daemon failed to clean temporary key/output"
    );
    assert_eq!(state.decodes.available_permits(), 2);
    assert_eq!(Failure::Unavailable.http_status(), 404);
    Ok(())
}
