use super::*;
use crate::service::history_export::Format;
use base64::Engine;
use std::{fs, path::Path};

fn fixture(root: &Path, name: &str, id: u64, format: Format) -> (RuntimeContext, Task) {
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
    let id = format!("{id:064x}");
    let output_dir = runtime.root.join("web-output").join(&runtime.id).join(&id);
    fs::create_dir_all(&output_dir).unwrap();
    artifacts::prepare(&runtime, &id).unwrap();
    let task = Task {
        id,
        kind: Kind::ExportHistory,
        options: Options {
            history_export: Some(Request {
                chat: "peer".into(),
                since: None,
                until: None,
                limit: 500,
                format,
            }),
            ..Default::default()
        },
        status: "failed".into(),
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
    };
    start(&runtime, &task).unwrap();
    (runtime, task)
}

fn document(runtime: &RuntimeContext, task: &Task, bytes: &[u8], commit: bool) -> PathBuf {
    let request = selection(task).unwrap();
    let path = output_path(runtime, &task.id, request).unwrap();
    crate::infrastructure::publication::ExportTarget::new_file(
        &path,
        &crate::infrastructure::publication::export_protected(runtime),
    )
    .unwrap()
    .write_bytes(bytes)
    .unwrap();
    if commit {
        published(
            runtime,
            &task.id,
            request,
            HistoryQuerySummary {
                username: "peer".into(),
                since_ts: None,
                until_ts: None,
                limit: 500,
                messages: 0,
            },
            0,
            &format!("{:x}", Sha256::digest(bytes)),
        )
        .unwrap();
    }
    path
}

fn finish(runtime: &RuntimeContext, task: &mut Task, control: &mut FinalizeControl) {
    task.result = Some(TaskResult::History(
        finalize(runtime, task, control).unwrap(),
    ));
}

#[test]
fn unregistered_file_is_unknown_not_successful_empty_export() {
    let root = tempfile::tempdir().unwrap();
    let (runtime, mut task) = fixture(root.path(), "account", 91, Format::Markdown);
    let path = document(&runtime, &task, b"not registered", false);
    finish(
        &runtime,
        &mut task,
        &mut FinalizeControl::supervised_worker(),
    );
    let Some(TaskResult::History(result)) = &task.result else {
        panic!("wrong scope")
    };
    assert!(result.query.is_none());
    assert_eq!(result.outcome, Some(ExportOutcome::Failure));
    assert!(!result.artifacts_complete);
    assert!(list(&runtime, &task, 0, 100).unwrap().items.is_empty());
    assert!(path.exists());
}

#[test]
fn registered_empty_file_survives_cancel_and_preserves_opaque_id() {
    let root = tempfile::tempdir().unwrap();
    let (runtime, mut task) = fixture(root.path(), "account", 92, Format::Txt);
    document(&runtime, &task, b"", true);
    finish(
        &runtime,
        &mut task,
        &mut FinalizeControl::supervised_worker(),
    );
    let page = list(&runtime, &task, 0, 1).unwrap();
    assert_eq!(page.scope, "chat_history");
    assert_eq!(page.total, 1);
    assert!(page.complete);
    let id = page.items[0].artifact_id.clone();
    task.status = "cancelled".into();
    finish(&runtime, &mut task, &mut FinalizeControl::interrupted());
    let page = list(&runtime, &task, 0, 1).unwrap();
    assert_eq!(page.items[0].artifact_id, id);
    assert!(!page.complete);
    let bytes = read(&runtime, &task, &id, 0, 1).unwrap();
    assert!(bytes.eof && bytes.data_base64.is_empty());
    assert_eq!(
        read(&runtime, &task, &id, 1, 1).unwrap_err().code,
        "invalid_artifact_request"
    );
    assert!(list(&runtime, &task, 1, 1).unwrap().items.is_empty());
    assert!(list(&runtime, &task, 2, 1).is_err());
}

#[test]
fn bytes_are_chunked_and_bound_to_request_account_scope_and_file_identity() {
    let root = tempfile::tempdir().unwrap();
    let (runtime, mut task) = fixture(root.path(), "account", 93, Format::Json);
    let bytes = vec![b'x'; CHUNK_BYTES as usize + 27];
    let path = document(&runtime, &task, &bytes, true);
    finish(
        &runtime,
        &mut task,
        &mut FinalizeControl::supervised_worker(),
    );
    let id = list(&runtime, &task, 0, 1).unwrap().items[0]
        .artifact_id
        .clone();
    let first = read(&runtime, &task, &id, 0, CHUNK_BYTES).unwrap();
    let second = read(&runtime, &task, &id, first.next_offset, CHUNK_BYTES).unwrap();
    let mut actual = base64::engine::general_purpose::STANDARD
        .decode(first.data_base64)
        .unwrap();
    actual.extend(
        base64::engine::general_purpose::STANDARD
            .decode(second.data_base64)
            .unwrap(),
    );
    assert_eq!(actual, bytes);
    assert!(second.eof);
    let mut wrong = task.clone();
    wrong.options.history_export.as_mut().unwrap().chat = "another".into();
    assert!(list(&runtime, &wrong, 0, 1).is_err());
    wrong = task.clone();
    wrong.kind = Kind::ExportAll;
    assert!(list(&runtime, &wrong, 0, 1).is_err());
    wrong = task.clone();
    wrong.result = Some(TaskResult::Directory(ExportAllResult::empty(false)));
    assert!(list(&runtime, &wrong, 0, 1).is_err());
    let (foreign, _) = fixture(root.path(), "foreign", 94, Format::Json);
    assert!(read(&foreign, &task, &id, 0, 1).is_err());
    let original = path.with_extension("saved");
    fs::rename(&path, &original).unwrap();
    fs::copy(&original, &path).unwrap();
    assert_eq!(
        read(&runtime, &task, &id, 0, 1).unwrap_err().code,
        "artifact_changed"
    );
}

#[test]
fn registration_rejects_content_not_produced_by_the_publisher() {
    let root = tempfile::tempdir().unwrap();
    let (runtime, mut task) = fixture(root.path(), "account", 95, Format::Yaml);
    document(&runtime, &task, b"replacement", false);
    let request = selection(&task).unwrap();
    assert!(published(
        &runtime,
        &task.id,
        request,
        HistoryQuerySummary {
            username: "peer".into(),
            since_ts: None,
            until_ts: None,
            limit: 500,
            messages: 0
        },
        0,
        &format!("{:x}", Sha256::digest(b"expected"))
    )
    .is_err());
    finish(
        &runtime,
        &mut task,
        &mut FinalizeControl::supervised_worker(),
    );
    assert!(list(&runtime, &task, 0, 1).unwrap().items.is_empty());
}
