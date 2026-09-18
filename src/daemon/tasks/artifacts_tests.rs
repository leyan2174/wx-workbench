use super::*;
use crate::{
    application::chat_directory,
    service::protocol::{Kind, Options},
};
use serde_json::json;

fn finalize(runtime: &RuntimeContext, task: &Task) -> Result<ExportAllResult> {
    super::finalize(runtime, task, &mut FinalizeControl::supervised_worker())
}

#[test]
fn cancellation_and_deadline_preserve_worker_prefix_without_processing_remaining_chats() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "account");
    let mut task = task(&runtime, 80);
    published_named(&runtime, &task, false, "first");
    let cp_path = private_dir(&runtime, &task.id)
        .unwrap()
        .join("step-report.json");
    let cp: Checkpoint = json(&cp_path, CHECKPOINT_LIMIT).unwrap();
    cp.register_chat(&runtime, &task.output_dir.join("chats"), "first")
        .unwrap();
    let prefix = read_index(&runtime, &task.id).unwrap();
    let ids: Vec<_> = prefix
        .entries
        .iter()
        .map(|e| e.artifact.artifact_id.clone())
        .collect();
    published_named(&runtime, &task, false, "second");
    published_named(&runtime, &task, false, "third");
    let (cancel, receiver) = tokio::sync::watch::channel(false);
    let mut control = FinalizeControl::new(
        receiver,
        tokio::sync::watch::channel(false).1,
        tokio::time::Instant::now() + std::time::Duration::from_secs(60),
    );
    control.cancel_after_blocks = Some((1, cancel.clone()));
    task.result = Some(super::finalize(&runtime, &task, &mut control).unwrap());
    assert!(*cancel.borrow());
    assert_eq!(control.blocks_read, 1);
    let result = task.result.as_ref().unwrap();
    assert_eq!(result.exported_chats, 1);
    assert!(!result.artifacts_complete);
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.code == "export_interrupted"));
    let index = read_index(&runtime, &task.id).unwrap();
    assert_eq!(index.chats.len(), 1);
    assert!(index.chunks_left < prefix.chunks_left);
    assert_eq!(
        index
            .entries
            .iter()
            .map(|e| e.artifact.artifact_id.clone())
            .collect::<Vec<_>>(),
        ids
    );
    assert!(read(&runtime, &task, &ids[0], 0, 32).is_ok());
    let budget = index.chunks_left;
    let mut cancelled = FinalizeControl::interrupted();
    task.result = Some(super::finalize(&runtime, &task, &mut cancelled).unwrap());
    assert_eq!(cancelled.blocks_read, 0);
    assert_eq!(read_index(&runtime, &task.id).unwrap().chunks_left, budget);
    let mut expired = FinalizeControl::new(
        tokio::sync::watch::channel(false).1,
        tokio::sync::watch::channel(false).1,
        tokio::time::Instant::now(),
    );
    task.result = Some(super::finalize(&runtime, &task, &mut expired).unwrap());
    assert_eq!(expired.blocks_read, 0);
    assert_eq!(task.result.as_ref().unwrap().exported_chats, 1);
    assert!(task
        .result
        .as_ref()
        .unwrap()
        .diagnostics
        .iter()
        .any(|d| d.code == "artifact_finalization_timeout"));
}

#[test]
fn failed_hash_consumes_budget_and_prevents_further_io() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("large.bin");
    fs::write(&path, vec![7u8; CHUNK_BYTES as usize + 1]).unwrap();
    let mut budget = 2;
    let mut control = FinalizeControl::supervised_worker();
    assert!(register(
        &path,
        "large.bin".into(),
        "large.bin",
        Some(&"0".repeat(64)),
        &mut budget,
        &mut control
    )
    .is_err());
    assert_eq!(budget, 0);
    assert_eq!(control.blocks_read, 2);
    assert!(register(
        &path,
        "large.bin".into(),
        "large.bin",
        None,
        &mut budget,
        &mut control
    )
    .is_err());
    assert_eq!(control.blocks_read, 2);
}

#[test]
fn large_file_cancels_after_one_block_and_terminal_report_keeps_partial() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "account");
    let mut task = task(&runtime, 81);
    let path = root.path().join("large.bin");
    fs::write(&path, vec![1u8; CHUNK_BYTES as usize * 3]).unwrap();
    let (sender, receiver) = tokio::sync::watch::channel(false);
    let mut control = FinalizeControl::new(
        receiver,
        tokio::sync::watch::channel(false).1,
        tokio::time::Instant::now() + std::time::Duration::from_secs(60),
    );
    control.cancel_after_blocks = Some((1, sender));
    let mut budget = 10;
    let error = register(
        &path,
        "large.bin".into(),
        "large.bin",
        None,
        &mut budget,
        &mut control,
    )
    .err()
    .unwrap();
    assert_eq!(
        error.downcast_ref::<ServiceError>().unwrap().code,
        "artifact_finalization_cancelled"
    );
    assert_eq!(control.blocks_read, 1);
    assert_eq!(budget, 7);
    let mut report = ExportAllResult::empty(false);
    report.exported_chats = 1;
    report.artifact_count = 2;
    report.artifacts_complete = false;
    report.outcome = Some(ExportOutcome::Partial);
    diagnostic(&mut report, "export_interrupted");
    super::super::worker::attach_export_report(&mut task, report.clone());
    assert_eq!(task.status, "cancelled");
    assert_eq!(task.result.as_ref().unwrap().artifact_count, 2);
    task.status = "succeeded".into();
    super::super::worker::attach_export_report(&mut task, report);
    assert_eq!(task.status, "failed");
    assert_eq!(task.result.as_ref().unwrap().exported_chats, 1);
}

fn runtime(root: &Path, name: &str) -> RuntimeContext {
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

fn task(runtime: &RuntimeContext, id: u64) -> Task {
    let id = format!("{id:064x}");
    let output_dir = runtime.root.join("web-output").join(&runtime.id).join(&id);
    fs::create_dir_all(&output_dir).unwrap();
    prepare(runtime, &id).unwrap();
    Task {
        id,
        kind: Kind::ExportAll,
        options: Options {
            include_images: false,
            ..Default::default()
        },
        status: "cancelled".into(),
        created_at: 1,
        started_at: Some(2),
        finished_at: Some(3),
        exit_code: None,
        logs: Default::default(),
        log_start_seq: 0,
        next_log_seq: 0,
        output_dir,
        error: None,
        result: None,
    }
}

fn published(runtime: &RuntimeContext, task: &Task, finalized: bool) -> PathBuf {
    published_named(runtime, task, finalized, "peer")
}

fn published_named(
    runtime: &RuntimeContext,
    task: &Task,
    finalized: bool,
    username: &str,
) -> PathBuf {
    let target = crate::message::export::Target {
        username: username.into(),
        chat: username.into(),
        is_group: false,
    };
    let directory = chat_directory::directory_name(&target);
    let checkpoint_path = private_dir(runtime, &task.id)
        .unwrap()
        .join("step-report.json");
    let mut checkpoint = if checkpoint_path.exists() {
        let mut checkpoint: Checkpoint = json(&checkpoint_path, CHECKPOINT_LIMIT).unwrap();
        checkpoint.candidates.push(Candidate {
            username: username.into(),
            directory: directory.clone(),
        });
        checkpoint.result.planned_chats = Some(checkpoint.candidates.len() as u64);
        checkpoint.save(runtime).unwrap();
        checkpoint
    } else {
        Checkpoint::start(
            runtime,
            &task.id,
            false,
            vec![Candidate {
                username: target.username.clone(),
                directory: directory.clone(),
            }],
        )
        .unwrap()
    };
    let output = task.output_dir.join("chats").join(&directory);
    let document = json!({"username":username,"is_group":false,"messages":[{
        "source":"message/message_0.db","local_id":7,"local_type":1,
        "server_id":987,"sort_seq":1,"status":null,"sender_username":null,
        "sender":"","timestamp":123,"raw_content":"x".repeat(CHUNK_BYTES as usize + 211)
    }]});
    chat_directory::export_document(
        runtime,
        &json!({}),
        &target,
        &document,
        &output,
        &chat_directory::Options {
            formats: [chat_directory::Format::Json].into(),
            media_enabled: false,
            update: false,
            max_media_bytes: DEFAULT_MEDIA_BYTES,
            max_total_media_bytes: DEFAULT_TOTAL_MEDIA_BYTES,
        },
        chat_directory::MediaInput::current(None),
    )
    .unwrap();
    // Simulate cancellation between publish_all and the next checkpoint write.
    if finalized {
        checkpoint.result.finalized = true;
        checkpoint.save(runtime).unwrap();
    }
    output
}

#[test]
fn cancelled_publication_is_readable_bounded_and_stable_without_stdout() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "account");
    let mut task = task(&runtime, 1);
    let output = published(&runtime, &task, false);
    fs::write(output.join("unlisted-secret.txt"), b"not an artifact").unwrap();
    task.result = Some(finalize(&runtime, &task).unwrap());
    let result = task.result.as_ref().unwrap();
    assert!(!result.finalized);
    assert_eq!(result.outcome, Some(ExportOutcome::Partial));
    assert_eq!(result.exported_chats, 1);
    let page = list(&runtime, &task, 0, 100).unwrap();
    assert!(!page
        .items
        .iter()
        .any(|item| item.name.contains("secret") || item.name == "_source_binding.json"));
    let artifact = page
        .items
        .iter()
        .find(|item| item.role == "chat_document")
        .unwrap();
    let original = fs::read(output.join(&artifact.name)).unwrap();
    assert!(original.len() > CHUNK_BYTES as usize);
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let part = read(&runtime, &task, &artifact.artifact_id, offset, CHUNK_BYTES).unwrap();
        bytes.extend(
            base64::engine::general_purpose::STANDARD
                .decode(&part.data_base64)
                .unwrap(),
        );
        offset = part.next_offset;
        if part.eof {
            break;
        }
    }
    assert_eq!(bytes, original);
    let cross = read(
        &runtime,
        &task,
        &artifact.artifact_id,
        CHUNK_BYTES as u64 - 3,
        12,
    )
    .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(cross.data_base64)
            .unwrap(),
        original[CHUNK_BYTES as usize - 3..CHUNK_BYTES as usize + 9]
    );
    let eof = read(&runtime, &task, &artifact.artifact_id, artifact.size, 1).unwrap();
    assert!(eof.eof && eof.data_base64.is_empty());
    assert_eq!(
        read(&runtime, &task, &artifact.artifact_id, artifact.size + 1, 1)
            .unwrap_err()
            .code,
        "invalid_artifact_request"
    );
    assert_eq!(
        list(&runtime, &task, 0, 0).unwrap_err().code,
        "invalid_artifact_request"
    );
    assert_eq!(
        list(&runtime, &task, 0, 100).unwrap().items[0].artifact_id,
        page.items[0].artifact_id
    );
    let tail = list(&runtime, &task, page.total, 1).unwrap();
    assert!(tail.items.is_empty() && tail.next_offset.is_none());
    assert_eq!(
        list(&runtime, &task, page.total + 1, 1).unwrap_err().code,
        "invalid_artifact_request"
    );
    let first = list(&runtime, &task, 0, 1).unwrap();
    assert_eq!(first.next_offset, Some(1));
    assert_eq!(
        list(&runtime, &task, 1, 1).unwrap().items[0].artifact_id,
        page.items[1].artifact_id
    );
    task.result = Some(finalize(&runtime, &task).unwrap());
    assert_eq!(
        list(&runtime, &task, 0, 100).unwrap().items[0].artifact_id,
        page.items[0].artifact_id
    );
}

#[test]
fn completed_chat_scope_survives_later_step_failure_and_rejects_file_replacement() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "account");
    let mut task = task(&runtime, 2);
    task.status = "failed".into();
    let output = published(&runtime, &task, true);
    task.result = Some(finalize(&runtime, &task).unwrap());
    assert_eq!(
        task.result.as_ref().unwrap().outcome,
        Some(ExportOutcome::Success)
    );
    let page = list(&runtime, &task, 0, 100).unwrap();
    let artifact = page
        .items
        .iter()
        .find(|item| item.role == "chat_document")
        .unwrap();
    let path = output.join(&artifact.name);
    let old = path.with_extension("saved");
    fs::rename(&path, &old).unwrap();
    fs::copy(&old, &path).unwrap();
    assert_eq!(
        read(&runtime, &task, &artifact.artifact_id, 0, 1)
            .unwrap_err()
            .code,
        "artifact_changed"
    );
    fs::remove_file(&path).unwrap();
    fs::hard_link(&old, &path).unwrap();
    assert_eq!(
        read(&runtime, &task, &artifact.artifact_id, 0, 1)
            .unwrap_err()
            .code,
        "artifact_unsafe"
    );
}

#[test]
fn uncommitted_files_and_foreign_binding_never_become_artifacts() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "account");
    let mut task = task(&runtime, 3);
    let output = published(&runtime, &task, false);
    let binding_path = output.join("_source_binding.json");
    let mut binding: serde_json::Value =
        serde_json::from_slice(&fs::read(&binding_path).unwrap()).unwrap();
    binding["source_id"] = "another-account".into();
    fs::write(&binding_path, serde_json::to_vec(&binding).unwrap()).unwrap();
    task.result = Some(finalize(&runtime, &task).unwrap());
    assert_eq!(task.result.as_ref().unwrap().artifact_count, 0);
    assert!(!task.result.as_ref().unwrap().artifacts_complete);
    fs::remove_file(output.join("_directory_export.json")).unwrap();
    task.result = Some(finalize(&runtime, &task).unwrap());
    assert_eq!(task.result.as_ref().unwrap().exported_chats, 0);
    assert!(list(&runtime, &task, 0, 50).unwrap().items.is_empty());
}

#[test]
fn dry_run_has_no_business_directory_or_artifacts() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "account");
    let mut task = task(&runtime, 4);
    task.options.dry_run = true;
    let mut checkpoint = Checkpoint::start(&runtime, &task.id, true, Vec::new()).unwrap();
    checkpoint.result.finalized = true;
    checkpoint.save(&runtime).unwrap();
    task.result = Some(finalize(&runtime, &task).unwrap());
    assert_eq!(
        task.result.as_ref().unwrap().outcome,
        Some(ExportOutcome::Success)
    );
    assert!(list(&runtime, &task, 0, 50).unwrap().items.is_empty());
    assert!(!task.output_dir.join("chats").exists());
}

#[test]
fn pinned_file_refuses_mutation_and_empty_artifact_reads_exact_eof() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "account");
    let mut task = task(&runtime, 5);
    let directory = task.output_dir.join("chats").join("synthetic");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("empty.bin");
    fs::write(&path, []).unwrap();
    let mut chunks = 1;
    let entry = register(
        &path,
        "synthetic/empty.bin".into(),
        "empty.bin",
        None,
        &mut chunks,
        &mut FinalizeControl::supervised_worker(),
    )
    .unwrap();
    assert!(entry.chunks.is_empty());
    assert_eq!(entry.artifact.sha256, format!("{:x}", Sha256::digest([])));
    let id = entry.artifact.artifact_id.clone();
    let index = Index {
        version: 1,
        runtime_id: runtime.id.clone(),
        task_id: task.id.clone(),
        complete: true,
        entries: vec![entry],
        chats: Vec::new(),
        chunks_left: chunks,
    };
    persist(
        &runtime,
        &task.id,
        "artifact-index.json",
        &index,
        MAX_INDEX_BYTES,
    )
    .unwrap();
    let mut result = ExportAllResult::empty(false);
    result.artifact_count = 1;
    result.artifacts_complete = true;
    task.result = Some(result);
    let part = read(&runtime, &task, &id, 0, 1).unwrap();
    assert!(part.eof && part.data_base64.is_empty());
    assert_eq!(part.bytes_read, 0);
    assert_eq!(part.next_offset, 0);
    assert_eq!(
        read(&runtime, &task, &id, 1, 1).unwrap_err().code,
        "invalid_artifact_request"
    );
    let mut reader = Reader::open(&path).unwrap();
    assert!(fs::write(&path, b"replacement").is_err());
    assert!(fs::remove_file(&path).is_err());
    assert!(fs::rename(&path, directory.join("moved")).is_err());
    assert!(reader.range(0, 0).unwrap().is_empty());
    reader.verify().unwrap();
    drop(reader);
    fs::remove_file(path).unwrap();
    assert_eq!(
        read(&runtime, &task, &id, 0, 1).unwrap_err().code,
        "artifact_unavailable"
    );
}

#[test]
fn rejects_path_injection_and_modified_content_even_with_restored_timestamp() {
    for name in [
        "../file",
        "C:/file",
        "//server/file",
        "a\\file",
        "file:stream",
        "NUL",
        "COM1.txt",
        "a/../b",
        "a/",
        "file.",
    ] {
        assert!(relative(name).is_err(), "{name}");
    }
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path(), "account");
    let mut task = task(&runtime, 6);
    let output = published(&runtime, &task, true);
    task.result = Some(finalize(&runtime, &task).unwrap());
    let artifact = list(&runtime, &task, 0, 100)
        .unwrap()
        .items
        .into_iter()
        .find(|item| item.role == "chat_document")
        .unwrap();
    let path = output.join(&artifact.name);
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let mut contents = fs::read(&path).unwrap();
    contents[0] ^= 1;
    fs::write(&path, &contents).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(
        read(&runtime, &task, &artifact.artifact_id, 0, 1)
            .unwrap_err()
            .code,
        "artifact_changed"
    );
    let other = self::runtime(root.path(), "other");
    assert_eq!(
        list(&other, &task, 0, 50).unwrap_err().code,
        "result_unavailable"
    );
}

#[test]
fn rejects_reparse_parent_without_reading_external_bytes() {
    let root = tempfile::tempdir().unwrap();
    let outside = root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("data.bin"), b"synthetic outside data").unwrap();
    let junction = root.path().join("junction");
    let result = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(result.status.success());
    let refused = Reader::open(&junction.join("data.bin"))
        .err()
        .map(|e| e.code);
    fs::remove_dir(&junction).unwrap();
    assert_eq!(refused.as_deref(), Some("artifact_unsafe"));
    assert_eq!(
        fs::read(outside.join("data.bin")).unwrap(),
        b"synthetic outside data"
    );
}
