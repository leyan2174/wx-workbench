use super::*;
use std::fs;

fn runtime(root: &std::path::Path, name: &str) -> RuntimeContext {
    let config_path = root.join(format!("{name}.json"));
    let config = crate::config::Config {
        db_dir: root.join(name).join("db_storage"),
        keys_file: root.join(format!("{name}-keys.json")),
        decrypted_dir: root.join(format!("{name}-decrypted")),
        wechat_process: "SyntheticNotRunning.exe".into(),
    };
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let runtime = RuntimeContext::from_config(config_path, config, root.join("home")).unwrap();
    fs::create_dir_all(&runtime.directory).unwrap();
    runtime
}

fn service(runtime: &RuntimeContext) -> (Arc<Service>, mpsc::Receiver<Work>) {
    Service::new(
        runtime.clone(),
        Arc::new(super::super::query_state::QueryState::new(runtime.clone())),
    )
    .unwrap()
}

fn submission(id: u64, kind: Kind) -> Call {
    Call::Submit {
        idempotency_key: format!("{id:064x}"),
        task: Submission {
            kind,
            options: Options::default(),
        },
    }
}

async fn configure(service: &Arc<Service>) {
    service
        .dispatch(Call::Configure {
            settings: SettingsInput::default(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn missing_query_keys_do_not_block_task_configuration_or_rejected_consent() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "a");
    let (service, _queue) = service(&runtime);
    assert!(!runtime.config.keys_file.exists());
    let info = service.dispatch(Call::Info {}).await.unwrap();
    assert_eq!(info["configured"], false);
    assert_eq!(
        service
            .dispatch(submission(1, Kind::WechatDecrypt))
            .await
            .unwrap_err()
            .code,
        "not_configured"
    );
    configure(&service).await;
    for kind in [Kind::WechatKeys, Kind::ImageKey, Kind::WxworkScan] {
        assert_eq!(
            service
                .dispatch(submission(2, kind))
                .await
                .unwrap_err()
                .code,
            "invalid_task"
        );
    }
    assert!(service.records.lock().unwrap().tasks.is_empty());
    assert!(!runtime.config.keys_file.exists());
    assert!(!runtime.config.decrypted_dir.exists());
}

#[tokio::test]
async fn queue_is_bounded_and_submit_is_durable_and_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "a");
    let (service, mut queue) = service(&runtime);
    configure(&service).await;
    let first = service
        .dispatch(submission(1, Kind::WechatDecrypt))
        .await
        .unwrap();
    assert_eq!(first["status"], "queued");
    let duplicate = service
        .dispatch(submission(1, Kind::WechatDecrypt))
        .await
        .unwrap();
    assert_eq!(first, duplicate);
    assert_eq!(service.records.lock().unwrap().tasks.len(), 1);
    assert_eq!(
        service
            .dispatch(submission(1, Kind::DecodeImages))
            .await
            .unwrap_err()
            .code,
        "submission_conflict"
    );
    for id in 2..=QUEUE_LIMIT as u64 {
        service
            .dispatch(submission(id, Kind::WechatDecrypt))
            .await
            .unwrap();
    }
    assert_eq!(
        service
            .dispatch(submission(9, Kind::WechatDecrypt))
            .await
            .unwrap_err()
            .code,
        "queue_full"
    );
    assert_eq!(queue.recv().await.unwrap().id, format!("{:064x}", 1));
    let journal: Value =
        serde_json::from_slice(&fs::read(runtime.directory.join("tasks-history.json")).unwrap())
            .unwrap();
    assert_eq!(journal["tasks"].as_array().unwrap().len(), QUEUE_LIMIT);
    assert_eq!(journal["requests"].as_object().unwrap().len(), QUEUE_LIMIT);
    crate::toolkit::private_file::assert_private_acl(&runtime.directory.join("tasks-history.json"));
}

#[tokio::test]
async fn queued_cancellation_is_terminal_and_cross_account_ids_are_unknown() {
    let root = tempfile::tempdir().unwrap();
    let a = runtime(root.path(), "a");
    let b = runtime(root.path(), "b");
    let (first, mut queue) = service(&a);
    let (second, _other) = service(&b);
    configure(&first).await;
    let task = first
        .dispatch(submission(1, Kind::WechatDecrypt))
        .await
        .unwrap();
    let id = task["id"].as_str().unwrap().to_owned();
    for call in [
        Call::Get { id: id.clone() },
        Call::Cancel { id: id.clone() },
    ] {
        assert_eq!(second.dispatch(call).await.unwrap_err().code, "not_found");
    }
    assert_eq!(
        first
            .dispatch(Call::Cancel { id: id.clone() })
            .await
            .unwrap()["status"],
        "cancelled"
    );
    let work = queue.recv().await.unwrap();
    assert!(*work.cancel.borrow());
    first.update(&id, |task| task.status = "running".into());
    assert_eq!(
        first.dispatch(Call::Get { id }).await.unwrap()["status"],
        "cancelled"
    );
}

#[tokio::test]
async fn settings_and_configuration_changes_never_mutate_a_queued_task() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "a");
    let (service, _queue) = service(&runtime);
    configure(&service).await;
    service
        .dispatch(submission(1, Kind::WechatDecrypt))
        .await
        .unwrap();
    let mut input = SettingsInput::default();
    input.enterprise_discovery_root = Some(root.path().to_owned());
    assert_eq!(
        service
            .dispatch(Call::Configure { settings: input })
            .await
            .unwrap_err()
            .code,
        "settings_conflict"
    );
    let mut config = serde_json::to_value(&runtime.config).unwrap();
    config["transcription_backend"] = json!("openai");
    fs::write(&runtime.config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    assert_eq!(
        service
            .dispatch(submission(2, Kind::WechatDecrypt))
            .await
            .unwrap_err()
            .code,
        "configuration_changed"
    );
    assert_eq!(service.records.lock().unwrap().tasks.len(), 1);
}

#[tokio::test]
async fn failed_journal_prevents_submission_and_preserves_unrelated_file() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "a");
    let (service, mut queue) = service(&runtime);
    configure(&service).await;
    let path = runtime.directory.join("tasks-history.json");
    let bytes = fs::read(&path).unwrap();
    let unrelated = root.path().join("keep.json");
    fs::hard_link(&path, &unrelated).unwrap();
    assert_eq!(
        service
            .dispatch(submission(1, Kind::WechatDecrypt))
            .await
            .unwrap_err()
            .code,
        "journal_unavailable"
    );
    assert!(queue.try_recv().is_err());
    assert!(service.records.lock().unwrap().tasks.is_empty());
    assert_eq!(fs::read(unrelated).unwrap(), bytes);
}

#[tokio::test]
async fn restart_marks_unfinished_tasks_interrupted_and_never_replays_them() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "a");
    let (first, queue) = service(&runtime);
    configure(&first).await;
    first
        .dispatch(submission(1, Kind::WechatDecrypt))
        .await
        .unwrap();
    first.update(&format!("{:064x}", 1), |task| {
        task.status = "running".into()
    });
    drop(first);
    drop(queue);
    let (restarted, mut queue) = service(&runtime);
    configure(&restarted).await;
    let task = restarted
        .dispatch(submission(1, Kind::WechatDecrypt))
        .await
        .unwrap();
    assert_eq!(task["status"], "interrupted");
    assert!(queue.try_recv().is_err());
}

#[tokio::test]
async fn logs_and_events_are_bounded_redacted_and_report_cursor_gaps() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "a");
    let (service, _queue) = service(&runtime);
    configure(&service).await;
    let task = service
        .dispatch(submission(1, Kind::WechatDecrypt))
        .await
        .unwrap();
    let id = task["id"].as_str().unwrap();
    let before = service.records.lock().unwrap().next_event - 1;
    service.log(id, "stdout", "API_KEY=synthetic-private-material");
    assert_eq!(
        service.records.lock().unwrap().tasks[0].logs[0].text,
        "[sensitive output redacted]"
    );
    for index in 0..300 {
        service.log(id, "stdout", &format!("line {index}"));
    }
    let task = service.dispatch(Call::Get { id: id.into() }).await.unwrap();
    assert_eq!(task["logs"].as_array().unwrap().len(), LOG_LIMIT);
    assert!(task["log_start_seq"].as_u64().unwrap() > 0);
    let events = service
        .dispatch(Call::Events {
            after: before,
            limit: 128,
            wait_ms: 0,
        })
        .await
        .unwrap();
    assert_eq!(events["reset"], true);
    let head = events["cursor"].as_u64().unwrap();
    service.log(id, "system", "latest");
    let events = service
        .dispatch(Call::Events {
            after: head,
            limit: 1,
            wait_ms: 0,
        })
        .await
        .unwrap();
    assert_eq!(events["reset"], false);
    assert_eq!(events["events"].as_array().unwrap().len(), 1);
    assert_eq!(
        service
            .dispatch(Call::Events {
                after: 0,
                limit: 129,
                wait_ms: 0
            })
            .await
            .unwrap_err()
            .code,
        "invalid_events"
    );
}
