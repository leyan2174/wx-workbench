use super::*;
use crate::infrastructure::publication::ExportTarget;
use base64::Engine;
use std::fs;

pub(super) fn fixture(base: &Path, account: &str, number: u64) -> (RuntimeContext, Task) {
    let config_path = base.join(format!("{account}.json"));
    let config = crate::config::Config {
        key_store: Some(base.join(format!("{account}-keys.dpapi"))),
        db_dir: base.join(account).join("db_storage"),
        keys_file: base.join(format!("{account}-keys.json")),
        decrypted_dir: base.join(format!("{account}-decrypted")),
        wechat_process: "SyntheticNotRunning.exe".into(),
    };
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let runtime = RuntimeContext::from_config(config_path, config, base.join("home")).unwrap();
    fs::create_dir_all(&runtime.directory).unwrap();
    let id = format!("{number:064x}");
    let output_dir = runtime.root.join("web-output").join(&runtime.id).join(&id);
    fs::create_dir_all(output_dir.join("voices")).unwrap();
    artifacts::prepare(&runtime, &id).unwrap();
    let task = Task {
        id,
        kind: Kind::ExportVoices,
        options: Options {
            voice_export: Some(Request::default()),
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
    selected(&runtime, &task.id, selection(&task).unwrap(), 3, None).unwrap();
    (runtime, task)
}

fn publish(runtime: &RuntimeContext, path: &Path, bytes: &[u8]) {
    ExportTarget::new_file(
        path,
        &crate::infrastructure::publication::export_protected(runtime),
    )
    .unwrap()
    .write_bytes(bytes)
    .unwrap();
}

fn files(runtime: &RuntimeContext, task: &Task, name: &str) -> [PathBuf; 2] {
    let output = root(runtime, &task.id).unwrap();
    [
        output.join(format!("{name}.silk")),
        output.join(format!("{name}.voice.json")),
    ]
}

const AUDIO: &[u8] = b"\x02#!SILK_V3synthetic";
const EVIDENCE: &[u8] = b"{\"timestamp_source\":\"media\"}";

fn commit(runtime: &RuntimeContext, task: &Task, paths: &[PathBuf; 2]) -> Result<()> {
    published_group(
        runtime,
        &task.id,
        selection(task).unwrap(),
        [
            (&paths[0], &format!("{:x}", Sha256::digest(AUDIO))),
            (&paths[1], &format!("{:x}", Sha256::digest(EVIDENCE))),
        ],
        true,
    )
}

fn prefix(runtime: &RuntimeContext, task: &Task) {
    let paths = files(runtime, task, "first");
    publish(runtime, &paths[0], AUDIO);
    publish(runtime, &paths[1], EVIDENCE);
    commit(runtime, task, &paths).unwrap();
}

fn terminal(runtime: &RuntimeContext, task: &mut Task, cancelled: bool) {
    task.status = if cancelled { "cancelled" } else { "failed" }.into();
    let mut control = if cancelled {
        FinalizeControl::interrupted()
    } else {
        FinalizeControl::supervised_worker()
    };
    task.result = Some(TaskResult::RawVoices(
        finalize(runtime, task, &mut control).unwrap(),
    ));
}

fn assert_prefix_readable(runtime: &RuntimeContext, task: &Task) {
    let Some(TaskResult::RawVoices(result)) = &task.result else {
        panic!("wrong scope")
    };
    assert_eq!(result.exported, 1);
    assert_eq!(result.associated, 1);
    assert_eq!(result.artifact_count, 2);
    assert!(!result.artifacts_complete);
    assert_eq!(result.outcome, Some(ExportOutcome::Partial));
    let page = list(runtime, task, 0, 100).unwrap();
    assert_eq!(page.total, 2);
    for artifact in page.items {
        let data = read(runtime, task, &artifact.artifact_id, 0, CHUNK_BYTES).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.data_base64)
            .unwrap();
        assert_eq!(
            bytes,
            if artifact.name.ends_with(".silk") {
                AUDIO
            } else {
                EVIDENCE
            }
        );
    }
}

#[test]
fn second_file_publication_failure_never_counts_half_group() {
    let base = tempfile::tempdir().unwrap();
    let (runtime, mut task) = fixture(base.path(), "account", 1);
    prefix(&runtime, &task);
    let paths = files(&runtime, &task, "second");
    publish(&runtime, &paths[0], AUDIO);
    let target = ExportTarget::new_file(
        &paths[1],
        &crate::infrastructure::publication::export_protected(&runtime),
    )
    .unwrap();
    fs::write(&paths[1], b"occupied after capture").unwrap();
    assert!(target.write_bytes(EVIDENCE).is_err());
    terminal(&runtime, &mut task, false);
    assert_prefix_readable(&runtime, &task);
}

#[test]
fn second_register_hash_failure_debits_io_but_never_commits_half_group() {
    let base = tempfile::tempdir().unwrap();
    let (runtime, mut task) = fixture(base.path(), "account", 2);
    prefix(&runtime, &task);
    let paths = files(&runtime, &task, "second");
    publish(&runtime, &paths[0], AUDIO);
    publish(&runtime, &paths[1], b"wrong evidence bytes");
    let before = load_bound(&runtime, &task.id, selection(&task).unwrap())
        .unwrap()
        .chunks_left;
    assert!(commit(&runtime, &task, &paths).is_err());
    let after = load_bound(&runtime, &task.id, selection(&task).unwrap()).unwrap();
    assert_eq!(after.chunks_left, before - 2);
    assert_eq!(after.groups.len(), 1);
    terminal(&runtime, &mut task, false);
    assert_prefix_readable(&runtime, &task);
}

#[test]
fn cancellation_before_group_registration_keeps_only_durable_prefix() {
    let base = tempfile::tempdir().unwrap();
    let (runtime, mut task) = fixture(base.path(), "account", 3);
    prefix(&runtime, &task);
    let paths = files(&runtime, &task, "not-registered");
    publish(&runtime, &paths[0], AUDIO);
    publish(&runtime, &paths[1], EVIDENCE);
    fs::write(
        root(&runtime, &task.id).unwrap().join(".staging"),
        b"temporary",
    )
    .unwrap();
    terminal(&runtime, &mut task, true);
    assert_prefix_readable(&runtime, &task);
}

#[test]
fn summary_failure_does_not_erase_committed_groups() {
    let base = tempfile::tempdir().unwrap();
    let (runtime, mut task) = fixture(base.path(), "account", 4);
    prefix(&runtime, &task);
    publish(
        &runtime,
        &root(&runtime, &task.id)
            .unwrap()
            .join("_voice_export_summary.json"),
        b"{}",
    );
    assert!(finish(
        &runtime,
        &task.id,
        selection(&task).unwrap(),
        2,
        false,
        &"0".repeat(64)
    )
    .is_err());
    terminal(&runtime, &mut task, false);
    assert_prefix_readable(&runtime, &task);
}

#[test]
fn hash_budget_exhaustion_is_explicit_and_preserves_complete_prefix() {
    let base = tempfile::tempdir().unwrap();
    let (runtime, mut task) = fixture(base.path(), "account", 5);
    prefix(&runtime, &task);
    let paths = files(&runtime, &task, "second");
    publish(&runtime, &paths[0], AUDIO);
    publish(&runtime, &paths[1], EVIDENCE);
    let mut index = load_bound(&runtime, &task.id, selection(&task).unwrap()).unwrap();
    index.chunks_left = 1;
    save(&runtime, &index).unwrap();
    let failure = commit(&runtime, &task, &paths).unwrap_err();
    assert_eq!(
        failure.downcast_ref::<ServiceError>().unwrap().code,
        "artifact_limit_exceeded"
    );
    failed(&runtime, &task.id, selection(&task).unwrap(), true).unwrap();
    assert_eq!(
        load_bound(&runtime, &task.id, selection(&task).unwrap())
            .unwrap()
            .chunks_left,
        0
    );
    terminal(&runtime, &mut task, false);
    assert_prefix_readable(&runtime, &task);
    let Some(TaskResult::RawVoices(result)) = task.result else {
        panic!("wrong scope")
    };
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.code == "voice_budget_exceeded"));
}

#[test]
fn account_request_and_result_scope_are_bound() {
    let base = tempfile::tempdir().unwrap();
    let (runtime, mut task) = fixture(base.path(), "first", 6);
    prefix(&runtime, &task);
    terminal(&runtime, &mut task, false);
    let (other, _) = fixture(base.path(), "second", 7);
    assert!(list(&other, &task, 0, 100).is_err());
    task.options.voice_export.as_mut().unwrap().limit = Some(0);
    assert!(list(&runtime, &task, 0, 100).is_err());
    task.options.voice_export.as_mut().unwrap().limit = None;
    task.result = Some(TaskResult::Directory(ExportAllResult::empty(false)));
    assert!(list(&runtime, &task, 0, 100).is_err());
}
