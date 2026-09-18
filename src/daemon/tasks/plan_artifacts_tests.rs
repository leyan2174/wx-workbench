use super::*;
use crate::service::protocol::Options;
use base64::Engine;
use std::fs;

fn runtime(root: &Path, name: &str) -> RuntimeContext {
    let config_path = root.join(format!("{name}.json"));
    let config = crate::config::Config {
        key_store: None,
        db_dir: root.join(name).join("db_storage"),
        keys_file: root.join(format!("{name}-keys.json")),
        decrypted_dir: root.join("shared-old-cache"),
        wechat_process: "SyntheticNotRunning.exe".into(),
    };
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let runtime = RuntimeContext::from_config(config_path, config, root.join("home")).unwrap();
    fs::create_dir_all(&runtime.directory).unwrap();
    runtime
}
fn request() -> Request {
    Request {
        users: None,
        exclude_users: Vec::new(),
        size_mode: SizeMode::Estimate,
        threads: 1,
        start: None,
        end: None,
    }
}
fn task(runtime: &RuntimeContext, id: u64, kind: Kind, options: Options) -> Task {
    let id = format!("{id:064x}");
    let output_dir = super::root(runtime, &id).unwrap();
    fs::create_dir_all(&output_dir).unwrap();
    artifacts::prepare(runtime, &id).unwrap();
    let task = Task {
        id,
        kind,
        options,
        status: "running".into(),
        created_at: 1,
        started_at: Some(2),
        finished_at: None,
        exit_code: None,
        logs: Default::default(),
        log_start_seq: 0,
        next_log_seq: 0,
        output_dir,
        error: None,
        result: None,
    };
    start(runtime, &task).unwrap();
    task
}
fn row(username: &str, index: usize) -> PlanRow {
    PlanRow {
        export: String::new(),
        index,
        username: username.into(),
        chat_name: "same display".into(),
        chat_type: "single".into(),
        message_count: 3,
        message_body_bytes: 17,
        first_ts: Some(1),
        last_ts: Some(2),
        first_time: "first".into(),
        last_time: "last".into(),
        attachment_estimated_bytes: 0,
        attachment_scanned_bytes: None,
        total_estimated_bytes: 17,
        size_status: "partial:resource_missing".into(),
    }
}
fn finish(runtime: &RuntimeContext, task: &mut Task, status: &str, control: &mut FinalizeControl) {
    let result = finalize(runtime, task, control).unwrap();
    task.status = status.into();
    task.finished_at = Some(3);
    crate::daemon::tasks::worker::attach_export_report(task, result);
}
fn generated(runtime: &RuntimeContext, id: u64, rows: Vec<PlanRow>) -> (Task, PlanRef) {
    let mut task = task(
        runtime,
        id,
        Kind::ChatPlan,
        Options {
            chat_plan: Some(request()),
            ..Default::default()
        },
    );
    publish_plan(runtime, &task.id, rows, (None, None)).unwrap();
    finish(
        runtime,
        &mut task,
        "succeeded",
        &mut FinalizeControl::supervised_worker(),
    );
    let Some(TaskResult::Plan(result)) = &task.result else {
        panic!("wrong result");
    };
    let reference = result.published_plan_ref.clone().unwrap();
    (task, reference)
}

#[test]
fn plan_projection_keeps_business_index_and_empty_tail_pages() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(temp.path(), "a");
    let (task, reference) = generated(&runtime, 1, vec![row("alpha", 9), row("beta", 21)]);
    let page = read_plan(&runtime, &task, reference.clone(), Mode::Blacklist, 0, 1).unwrap();
    assert_eq!(
        (page.total, page.selected_count, page.next_offset),
        (2, 2, Some(1))
    );
    assert_eq!(page.rows[0].index, 9);
    let wire = serde_json::to_value(&page).unwrap();
    for private in ["message_body_bytes", "first_ts", "last_ts"] {
        assert!(wire["rows"][0].get(private).is_none());
    }
    assert_eq!(
        read_plan(&runtime, &task, reference.clone(), Mode::Whitelist, 0, 100)
            .unwrap()
            .selected_count,
        0
    );
    for offset in [2, 3, u64::MAX] {
        let page = read_plan(
            &runtime,
            &task,
            reference.clone(),
            Mode::Blacklist,
            offset,
            100,
        )
        .unwrap();
        assert!(page.rows.is_empty());
        assert_eq!(page.offset, offset);
        assert_eq!(page.next_offset, None);
    }
    for limit in [0, 101] {
        assert!(read_plan(
            &runtime,
            &task,
            reference.clone(),
            Mode::Blacklist,
            0,
            limit
        )
        .is_err());
    }
    let Some(TaskResult::Plan(result)) = &task.result else {
        panic!();
    };
    assert!(result.finalized && result.artifacts_complete);
    assert_eq!(result.outcome, Some(ExportOutcome::Partial));
    assert_eq!(result.partial_rows, 2);
}

#[test]
fn review_is_immutable_and_unknown_or_duplicate_edits_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(temp.path(), "a");
    let (parent, reference) = generated(&runtime, 2, vec![row("alpha", 1), row("beta", 2)]);
    let before = fs::read(parent.output_dir.join("plan.csv")).unwrap();
    let mut resolved = resolve(&runtime, &parent, &reference).unwrap();
    let review = ReviewRequest {
        plan_ref: reference.clone(),
        changes: vec![
            Change {
                username: "alpha".into(),
                export: "1".into(),
            },
            Change {
                username: "beta".into(),
                export: "0".into(),
            },
        ],
    };
    validate_changes(&resolved.rows, &review).unwrap();
    for change in &review.changes {
        resolved
            .rows
            .iter_mut()
            .find(|r| r.username == change.username)
            .unwrap()
            .export
            .clone_from(&change.export);
    }
    let mut child = task(
        &runtime,
        3,
        Kind::ChatPlanReview,
        Options {
            chat_plan_review: Some(review.clone()),
            ..Default::default()
        },
    );
    publish_plan(&runtime, &child.id, resolved.rows, (None, None)).unwrap();
    finish(
        &runtime,
        &mut child,
        "succeeded",
        &mut FinalizeControl::supervised_worker(),
    );
    let Some(TaskResult::Plan(result)) = &child.result else {
        panic!();
    };
    assert_eq!(result.parent_ref.as_ref(), Some(&reference));
    assert_ne!(
        result.published_plan_ref.as_ref().unwrap().artifact_id,
        reference.artifact_id
    );
    let page = read_plan(
        &runtime,
        &child,
        result.published_plan_ref.clone().unwrap(),
        Mode::Whitelist,
        0,
        100,
    )
    .unwrap();
    assert_eq!(page.selected_count, 1);
    assert!(page.rows[0].selected);
    assert!(!page.rows[1].selected);
    assert_eq!(
        fs::read(parent.output_dir.join("plan.csv")).unwrap(),
        before
    );
    let mut bad = review.clone();
    bad.changes[0].username = "unknown".into();
    assert!(validate_changes(&resolve(&runtime, &parent, &reference).unwrap().rows, &bad).is_err());
    let first = bad.changes[0].clone();
    bad.changes.push(first);
    assert!(bad.validate().is_err());
}

#[test]
fn references_bind_account_hash_request_config_and_published_file() {
    let temp = tempfile::tempdir().unwrap();
    let a = runtime(temp.path(), "a");
    let b = runtime(temp.path(), "b");
    let (task, reference) = generated(&a, 4, vec![row("alpha", 7)]);
    assert!(resolve(&b, &task, &reference).is_err());
    let mut bad = reference.clone();
    bad.sha256 = "f".repeat(64);
    assert!(resolve(&a, &task, &bad).is_err());
    let mut changed = task.clone();
    changed.options.chat_plan.as_mut().unwrap().users = Some(vec![]);
    assert!(resolve(&a, &changed, &reference).is_err());
    let config = fs::read(&a.config_path).unwrap();
    fs::write(&a.config_path, [config.as_slice(), b" "].concat()).unwrap();
    assert!(resolve(&a, &task, &reference).is_err());
    fs::write(&a.config_path, &config).unwrap();
    assert!(resolve(&a, &task, &reference).is_ok());
    let path = task.output_dir.join("plan.csv");
    let original = fs::read(&path).unwrap();
    fs::write(&path, b"tampered").unwrap();
    assert!(resolve(&a, &task, &reference).is_err());
    fs::remove_file(&path).unwrap();
    fs::write(&path, original).unwrap();
    assert!(resolve(&a, &task, &reference).is_err());
}

#[test]
fn empty_plan_does_not_expand_selection_and_unregistered_csv_is_not_published() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(temp.path(), "a");
    let (task, reference) = generated(&runtime, 5, vec![]);
    for mode in [Mode::Blacklist, Mode::Whitelist] {
        let resolved = resolve(&runtime, &task, &reference).unwrap();
        let plan = Plan::read(resolved.csv.as_slice(), mode).unwrap();
        assert!(plan.select(["alpha", "beta"]).unwrap().is_empty());
    }
    let mut unregistered = self::task(
        &runtime,
        6,
        Kind::ChatPlan,
        Options {
            chat_plan: Some(request()),
            ..Default::default()
        },
    );
    fs::write(unregistered.output_dir.join("plan.csv"), b"not published").unwrap();
    finish(
        &runtime,
        &mut unregistered,
        "failed",
        &mut FinalizeControl::supervised_worker(),
    );
    assert_eq!(unregistered.status, "failed");
    assert_eq!(list(&runtime, &unregistered, 0, 100).unwrap().total, 0);
    let Some(TaskResult::Plan(result)) = unregistered.result else {
        panic!();
    };
    assert!(!result.finalized && !result.artifacts_complete && result.published_plan_ref.is_none());
    assert_eq!(result.outcome, Some(ExportOutcome::Failure));
}

#[test]
fn cancelled_and_failed_apply_keep_only_registered_chat_prefix() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(temp.path(), "a");
    let (_, reference) = generated(&runtime, 7, vec![row("alpha", 1), row("beta", 2)]);
    for (id, status) in [(8, "cancelled"), (9, "failed")] {
        let mut task = task(
            &runtime,
            id,
            Kind::ChatPlanApply,
            Options {
                chat_plan_apply: Some(ApplyRequest {
                    plan_ref: reference.clone(),
                    plan_mode: Mode::Blacklist,
                    dry_run: false,
                }),
                ..Default::default()
            },
        );
        apply_selected(&runtime, &task.id, 2).unwrap();
        let path = task.output_dir.join("archive/alpha.json");
        let document = serde_json::json!({"username":"alpha","messages":[{"text":"synthetic"}]});
        crate::infrastructure::publication::ExportTarget::new_file(
            &path,
            &crate::infrastructure::publication::export_protected(&runtime),
        )
        .unwrap()
        .write_json(&document)
        .unwrap();
        publish_chat(&runtime, &task.id, &path, &document).unwrap();
        fs::write(task.output_dir.join("archive/beta.json"), b"unregistered").unwrap();
        fs::write(task.output_dir.join("archive/staging.tmp"), b"staging").unwrap();
        finish(
            &runtime,
            &mut task,
            status,
            &mut FinalizeControl::interrupted(),
        );
        assert_eq!(task.status, status);
        let page = list(&runtime, &task, 0, 100).unwrap();
        assert_eq!(page.total, 1);
        assert!(!page.complete);
        assert_eq!(page.scope, "chat_plan_apply");
        let bytes = read(&runtime, &task, &page.items[0].artifact_id, 0, CHUNK_BYTES).unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(bytes.data_base64)
                .unwrap(),
            serde_json::to_vec_pretty(&document).unwrap()
        );
        let Some(TaskResult::PlanApply(result)) = &task.result else {
            panic!();
        };
        assert!(!result.finalized && !result.artifacts_complete);
        assert_eq!(
            (
                result.selected_count,
                result.published_count,
                result.messages
            ),
            (2, 1, 1)
        );
        assert_eq!(result.outcome, Some(ExportOutcome::Partial));
        assert!(result
            .diagnostics
            .iter()
            .any(|d| d.code == "export_interrupted"));
    }
}

#[test]
fn dry_run_and_empty_apply_are_finalized_without_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(temp.path(), "a");
    let (_, reference) = generated(&runtime, 10, vec![]);
    for (id, dry_run, count) in [(11, false, 0), (12, true, 2)] {
        let mut task = task(
            &runtime,
            id,
            Kind::ChatPlanApply,
            Options {
                chat_plan_apply: Some(ApplyRequest {
                    plan_ref: reference.clone(),
                    plan_mode: Mode::Whitelist,
                    dry_run,
                }),
                ..Default::default()
            },
        );
        apply_selected(&runtime, &task.id, count).unwrap();
        finish_apply(&runtime, &task.id, 0).unwrap();
        finish(
            &runtime,
            &mut task,
            "succeeded",
            &mut FinalizeControl::supervised_worker(),
        );
        assert_eq!(task.status, "succeeded");
        assert!(!task.output_dir.join("archive").exists());
        let Some(TaskResult::PlanApply(result)) = task.result else {
            panic!();
        };
        assert!(result.finalized && result.artifacts_complete);
        assert_eq!(result.outcome, Some(ExportOutcome::Success));
        assert_eq!(result.artifact_count, 0);
    }
}

#[test]
fn read_limit_measures_escaped_serialized_reply_not_plain_strings() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(temp.path(), "a");
    let mut large = row("alpha", 1);
    large.chat_name = "\u{1}".repeat(MAX_READ_BYTES / 3);
    let (task, reference) = generated(&runtime, 13, vec![large]);
    let error = read_plan(&runtime, &task, reference, Mode::Blacklist, 0, 1).unwrap_err();
    assert_eq!(error.code, "result_limit");
}

#[test]
fn failed_registration_debits_hash_budget_and_does_not_publish() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(temp.path(), "a");
    let (_, reference) = generated(&runtime, 14, vec![row("alpha", 1)]);
    let task = task(
        &runtime,
        15,
        Kind::ChatPlanApply,
        Options {
            chat_plan_apply: Some(ApplyRequest {
                plan_ref: reference,
                plan_mode: Mode::Blacklist,
                dry_run: false,
            }),
            ..Default::default()
        },
    );
    apply_selected(&runtime, &task.id, 1).unwrap();
    let mut index = load_bound(&runtime, &task.id).unwrap();
    index.chunks_left = 2;
    save(&runtime, &index).unwrap();
    let path = task.output_dir.join("archive/alpha.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"wrong bytes").unwrap();
    let document = serde_json::json!({"username":"alpha","messages":[]});
    for left in [1, 0, 0] {
        assert!(publish_chat(&runtime, &task.id, &path, &document).is_err());
        let index = load_bound(&runtime, &task.id).unwrap();
        assert_eq!(index.chunks_left, left);
        assert!(index.entries.is_empty());
        assert_eq!(index.result.artifact_count(), 0);
    }
}

#[test]
fn finalized_results_cannot_claim_success_with_unpublished_selection() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(temp.path(), "a");
    let (_, reference) = generated(&runtime, 16, vec![row("alpha", 1)]);
    let task = task(
        &runtime,
        17,
        Kind::ChatPlanApply,
        Options {
            chat_plan_apply: Some(ApplyRequest {
                plan_ref: reference,
                plan_mode: Mode::Blacklist,
                dry_run: false,
            }),
            ..Default::default()
        },
    );
    apply_selected(&runtime, &task.id, 1).unwrap();
    assert!(finish_apply(&runtime, &task.id, 0).is_err());
    let index = load_bound(&runtime, &task.id).unwrap();
    let TaskResult::PlanApply(result) = index.result else {
        panic!();
    };
    assert!(!result.finalized);
    assert_eq!(result.outcome, None);
}
