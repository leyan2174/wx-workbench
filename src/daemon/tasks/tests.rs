use super::*;
use std::fs;

fn runtime(root: &std::path::Path, name: &str) -> RuntimeContext {
    let config_path = root.join(format!("{name}.json"));
    let config = crate::config::Config {
        key_store: None,
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
    let query = Arc::new(super::super::query_state::QueryState::new(runtime.clone()));
    let keys = super::super::worker_keys::Broker::new(runtime.clone(), query.clone());
    Service::new(runtime.clone(), query, keys).unwrap()
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
    for kind in [Kind::WechatKeys, Kind::ImageKey] {
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
async fn unsupported_history_records_fail_restore_without_rewriting_history() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "unsupported-history");
    let (first, queue) = service(&runtime);
    configure(&first).await;
    first
        .dispatch(submission(1, Kind::WechatDecrypt))
        .await
        .unwrap();
    drop(queue);
    drop(first);
    let path = runtime.directory.join("tasks-history.json");
    let journal: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for kind in ["toolkit", "unknown_future_task"] {
        let mut unsupported = journal.clone();
        unsupported["tasks"][0]["kind"] = json!(kind);
        let bytes = serde_json::to_vec(&unsupported).unwrap();
        fs::write(&path, &bytes).unwrap();
        let query = Arc::new(super::super::query_state::QueryState::new(runtime.clone()));
        let keys = super::super::worker_keys::Broker::new(runtime.clone(), query.clone());
        let error = Service::new(runtime.clone(), query, keys)
            .err()
            .expect("unsupported history must fail startup");
        assert!(error
            .to_string()
            .contains("Unsupported or invalid task history record at index 0"));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn retired_tasks_are_archived_without_losing_personal_history_or_outputs() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "migration");
    let (first, queue) = service(&runtime);
    configure(&first).await;
    first
        .dispatch(submission(1, Kind::WechatDecrypt))
        .await
        .unwrap();
    drop(queue);
    drop(first);
    let journal_path = runtime.directory.join("tasks-history.json");
    let mut journal: Value = serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    journal["tasks"][0]["options"]["all_conversations"] = json!(false);
    let mut retired = journal["tasks"][0].clone();
    let retired_id = format!("{:064x}", 2);
    retired["id"] = json!(retired_id);
    retired["kind"] = json!("wxwork_export");
    retired["options"]["all_conversations"] = json!(true);
    let old_output = runtime
        .root
        .join("web-output")
        .join(&runtime.id)
        .join(&retired_id);
    fs::create_dir_all(&old_output).unwrap();
    fs::write(old_output.join("keep.txt"), b"synthetic prior output").unwrap();
    retired["output_dir"] = json!(old_output);
    journal["tasks"].as_array_mut().unwrap().push(retired);
    journal["requests"][&retired_id] = json!("b".repeat(64));
    let original = serde_json::to_vec(&journal).unwrap();
    fs::write(&journal_path, &original).unwrap();
    let (restored, mut queue) = service(&runtime);
    let list = restored.dispatch(Call::List {}).await.unwrap();
    assert_eq!(list["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(list["tasks"][0]["status"], "interrupted");
    assert!(list["tasks"][0]["options"]
        .get("all_conversations")
        .is_none());
    assert!(matches!(
        queue.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert_eq!(
        fs::read(old_output.join("keep.txt")).unwrap(),
        b"synthetic prior output"
    );
    let archive = runtime.directory.join(format!(
        "tasks-history-retired-{:x}.json",
        Sha256::digest(&original)
    ));
    assert_eq!(fs::read(&archive).unwrap(), original);
    crate::private_file::assert_private_acl(&archive);
    let current: Value = serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(current["requests"].as_object().unwrap().len(), 1);
    configure(&restored).await;
    assert_eq!(
        restored
            .dispatch(submission(1, Kind::WechatDecrypt))
            .await
            .unwrap()["status"],
        "interrupted"
    );
    drop(queue);
    drop(restored);
    let (_again, _queue) = service(&runtime);
    assert_eq!(fs::read(&archive).unwrap(), original);
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
    crate::private_file::assert_private_acl(&runtime.directory.join("tasks-history.json"));
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
    let input = SettingsInput {
        image_cache_dir: Some(root.path().to_owned()),
    };
    assert_eq!(
        service
            .dispatch(Call::Configure { settings: input })
            .await
            .unwrap_err()
            .code,
        "settings_conflict"
    );
    let mut config = serde_json::to_value(&runtime.config).unwrap();
    config["transcription_backend"] = json!("openai_compatible");
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
