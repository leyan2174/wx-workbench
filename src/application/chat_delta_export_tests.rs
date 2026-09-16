use super::*;

fn golden() -> Value {
    serde_json::from_str(include_str!("../../tests/fixtures/chat-delta-golden.json")).unwrap()
}

fn setup() -> (DeltaWindow, DeltaChat) {
    let g = golden();
    (
        serde_json::from_value(g["window"].clone()).unwrap(),
        serde_json::from_value(g["cases"][0]["input"].clone()).unwrap(),
    )
}

#[test]
fn python_oracle_documents_results_and_manifest() {
    let g = golden();
    let window = serde_json::from_value(g["window"].clone()).unwrap();
    let mut results = Vec::new();
    for case in g["cases"].as_array().unwrap() {
        let chat = serde_json::from_value(case["input"].clone()).unwrap();
        let prepared = prepare_delta(&chat, &window).unwrap();
        assert_eq!(prepared.result, case["result"]);
        assert_eq!(prepared.document.unwrap_or(Value::Null), case["document"]);
        results.push(prepared.result);
    }
    assert_eq!(
        delta_manifest(&window, results.len(), &results).unwrap(),
        g["manifest"]
    );
}

#[test]
fn required_start_inclusive_end_stable_duplicates_and_open_end() {
    let (mut window, chat) = setup();
    window.start = None;
    assert!(prepare_delta(&chat, &window).is_err());
    window.start = Some(100);
    window.end = Some(100);
    let document = prepare_delta(&chat, &window).unwrap().document.unwrap();
    assert_eq!(document["message_count"], 2);
    assert_eq!(document["messages"][0], document["messages"][1]);
    window.end = None;
    assert_eq!(
        prepare_delta(&chat, &window).unwrap().result["message_count"],
        8
    );
    window.end = Some(99);
    assert!(prepare_delta(&chat, &window).unwrap().document.is_none());
    assert_eq!(window.date(0).unwrap(), "");
}

#[test]
fn uid_depends_on_raw_source_and_windows_basename() {
    let raw = RawContent::Text("source".into());
    let uid = delta_msg_uid("u", "C:\\a\\message.db", 1, 2, "text", &raw);
    assert_eq!(
        uid,
        delta_msg_uid("u", "D:/b/message.db", 1, 2, "text", &raw)
    );
    assert_eq!(uid, delta_msg_uid("u", "D:message.db", 1, 2, "", &raw));
    assert_ne!(uid, delta_msg_uid("u", "other.db", 1, 2, "text", &raw));
    assert_ne!(
        delta_msg_uid("u", "", 1, 2, "", &RawContent::Null),
        delta_msg_uid("u", "", 1, 2, "", &RawContent::Text("".into()))
    );
}

#[test]
fn publish_is_exclusive_noclobber_and_failure_is_in_manifest() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("new-root");
    let (window, chat) = setup();
    let mut writer = DeltaRunWriter::create(&root, window.clone()).unwrap();
    assert!(DeltaRunWriter::create(&root, window.clone()).is_err());
    let first = writer.write_chat(&chat);
    assert_eq!(first["success"], true);
    let path = root
        .join("deltas")
        .join(&window.run_id)
        .join(first["path"].as_str().unwrap());
    let original = fs::read(&path).unwrap();
    assert_eq!(writer.write_chat(&chat)["success"], false);
    assert_eq!(fs::read(&path).unwrap(), original);
    let mut failed = chat.clone();
    failed.source_error = Some("synthetic source failure".into());
    writer.write_chat(&failed);
    let manifest_path = writer.finish().unwrap();
    let manifest: Value = serde_json::from_slice(&fs::read(manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["chats_checked"], 3);
    assert_eq!(manifest["chats_with_messages"], 1);
    assert_eq!(manifest["errors"].as_array().unwrap().len(), 2);
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn traversal_devices_and_existing_full_root_are_rejected() {
    let (mut window, _) = setup();
    for run in [
        "../escape",
        "..",
        "a/b",
        "a\\b",
        "C:evil",
        "CON",
        "lpt1.json",
        "x.",
        "x ",
        "a\0b",
    ] {
        window.run_id = run.into();
        assert!(window.validate().is_err(), "{run:?}");
    }
    let (window, _) = setup();
    let parent = tempfile::tempdir().unwrap();
    let full = parent.path().join("full.json");
    fs::write(&full, b"synthetic full JSON must stay untouched").unwrap();
    assert!(DeltaRunWriter::create(parent.path(), window.clone()).is_err());
    assert!(DeltaRunWriter::create(&parent.path().join("..\\escape"), window.clone()).is_err());
    assert!(DeltaRunWriter::create(Path::new("relative"), window).is_err());
    assert_eq!(
        fs::read(&full).unwrap(),
        b"synthetic full JSON must stay untouched"
    );
}

#[test]
fn manifest_collision_returns_error_without_overwrite() {
    let parent = tempfile::tempdir().unwrap();
    let (window, _) = setup();
    let root = parent.path().join("new-root");
    let writer = DeltaRunWriter::create(&root, window.clone()).unwrap();
    let target = root
        .join("deltas")
        .join(window.run_id)
        .join("manifest.json");
    fs::write(&target, b"existing").unwrap();
    assert!(writer.finish().is_err());
    assert_eq!(fs::read(target).unwrap(), b"existing");
}

#[test]
fn protected_delta_roots_are_rejected_before_creating_any_batch() {
    let parent = tempfile::tempdir().unwrap();
    let (window, _) = setup();
    let private = parent.path().join("synthetic-private");
    let root = private.join("output");
    fs::create_dir(&private).unwrap();
    assert!(DeltaRunWriter::create_protected(
        &root,
        window.clone(),
        std::slice::from_ref(&private)
    )
    .is_err());
    assert!(!root.exists());
    fs::create_dir(&root).unwrap();
    assert!(DeltaRunWriter::append_protected(&root, window, &[private]).is_err());
    assert!(!root.join("deltas").exists());
}

#[test]
fn delta_files_use_shared_private_publication_and_preserve_collisions() {
    let parent = tempfile::tempdir().unwrap();
    let (window, chat) = setup();
    let root = parent.path().join("published");
    let mut writer = DeltaRunWriter::create_protected(&root, window.clone(), &[]).unwrap();
    let first = writer.write_chat(&chat);
    assert_eq!(first["success"], true);
    let path = root
        .join("deltas")
        .join(&window.run_id)
        .join(first["path"].as_str().unwrap());
    let bytes = fs::read(&path).unwrap();
    assert_eq!(writer.write_chat(&chat)["success"], false);
    assert_eq!(fs::read(path).unwrap(), bytes);
    let manifest: Value =
        serde_json::from_slice(&fs::read(writer.finish().unwrap()).unwrap()).unwrap();
    assert_eq!(manifest["errors"].as_array().unwrap().len(), 1);
}

#[test]
fn published_run_matches_python_oracle() {
    let g = golden();
    let window: DeltaWindow = serde_json::from_value(g["window"].clone()).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("published");
    let mut writer = DeltaRunWriter::create(&root, window.clone()).unwrap();
    for case in g["cases"].as_array().unwrap() {
        let chat = serde_json::from_value(case["input"].clone()).unwrap();
        let result = writer.write_chat(&chat);
        assert_eq!(result, case["result"]);
        if let Some(relative) = result["path"].as_str() {
            let path = root.join("deltas").join(&window.run_id).join(relative);
            let actual: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            assert_eq!(actual, case["document"]);
        }
    }
    let path = writer.finish().unwrap();
    let actual: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(actual, g["manifest"]);
}

#[test]
fn invalid_filename_is_reported_without_partial_file() {
    let (window, mut chat) = setup();
    chat.display_name = "bad\0name".into();
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("published");
    let mut writer = DeltaRunWriter::create(&root, window.clone()).unwrap();
    assert_eq!(writer.write_chat(&chat)["success"], false);
    let path = writer.finish().unwrap();
    let manifest: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(manifest["errors"].as_array().unwrap().len(), 1);
    assert_eq!(manifest["messages_exported"], 0);
    assert_eq!(
        fs::read_dir(root.join("deltas").join(window.run_id).join("chats"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn existing_root_accepts_two_runs_without_changing_old_files() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path();
    let full = root.join("full.json");
    fs::write(&full, b"not JSON: old full export").unwrap();
    let (mut window, chat) = setup();
    window.run_id = "first".into();
    let mut first = DeltaRunWriter::create_run_in_existing_root(root, window.clone()).unwrap();
    let result = first.write_chat(&chat);
    assert_eq!(result["success"], true);
    let chat_path = root
        .join("deltas/first")
        .join(result["path"].as_str().unwrap());
    let chat_bytes = fs::read(&chat_path).unwrap();
    let manifest_path = first.finish().unwrap();
    let manifest_bytes = fs::read(&manifest_path).unwrap();
    assert!(DeltaRunWriter::create_run_in_existing_root(root, window.clone()).is_err());
    assert!(DeltaRunWriter::create(root, window.clone()).is_err());
    window.run_id = "second".into();
    let mut second = DeltaRunWriter::create_run_in_existing_root(root, window).unwrap();
    assert_eq!(second.write_chat(&chat)["success"], true);
    let manifest: Value =
        serde_json::from_slice(&fs::read(second.finish().unwrap()).unwrap()).unwrap();
    assert_eq!(manifest["run_id"], "second");
    assert_eq!(fs::read(full).unwrap(), b"not JSON: old full export");
    assert_eq!(fs::read(chat_path).unwrap(), chat_bytes);
    assert_eq!(fs::read(manifest_path).unwrap(), manifest_bytes);
    assert_eq!(fs::read_dir(root.join("deltas")).unwrap().count(), 2);
}

#[test]
fn existing_root_rejects_missing_roots_files_and_unfinished_runs() {
    let parent = tempfile::tempdir().unwrap();
    let (window, _) = setup();
    let missing = parent.path().join("missing");
    assert!(DeltaRunWriter::create_run_in_existing_root(&missing, window.clone()).is_err());
    assert!(!missing.exists());
    let file = parent.path().join("file");
    fs::write(&file, b"keep").unwrap();
    assert!(DeltaRunWriter::create_run_in_existing_root(&file, window.clone()).is_err());
    let deltas = parent.path().join("deltas");
    fs::write(&deltas, b"keep deltas").unwrap();
    assert!(DeltaRunWriter::create_run_in_existing_root(parent.path(), window.clone()).is_err());
    assert_eq!(fs::read(&deltas).unwrap(), b"keep deltas");
    fs::remove_file(&deltas).unwrap();
    fs::create_dir(&deltas).unwrap();
    let run = deltas.join(&window.run_id);
    fs::create_dir(&run).unwrap();
    assert!(DeltaRunWriter::create_run_in_existing_root(parent.path(), window).is_err());
    assert_eq!(fs::read_dir(run).unwrap().count(), 0);
    assert_eq!(fs::read(file).unwrap(), b"keep");
}

#[test]
fn concurrent_existing_root_run_has_exactly_one_winner() {
    let parent = tempfile::tempdir().unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
    let handles: Vec<_> = (0..6)
        .map(|_| {
            let root = parent.path().to_path_buf();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                DeltaRunWriter::create_run_in_existing_root(&root, setup().0)
                    .and_then(DeltaRunWriter::finish)
                    .is_ok()
            })
        })
        .collect();
    let winners = handles
        .into_iter()
        .filter_map(|h| h.join().unwrap().then_some(()))
        .count();
    assert_eq!(winners, 1);
    let run = parent.path().join("deltas").join(setup().0.run_id);
    assert_eq!(fs::read_dir(run).unwrap().count(), 2);
}

#[cfg(windows)]
fn junction(link: &Path, target: &Path) {
    let script = "$ErrorActionPreference = 'Stop'\nNew-Item -ItemType Junction -Path $env:WX_DELTA_TEST_LINK -Target $env:WX_DELTA_TEST_TARGET";
    eprintln!("powershell.exe -NoProfile -NonInteractive -Command {script:?}\nWX_DELTA_TEST_LINK={}\nWX_DELTA_TEST_TARGET={}", link.display(), target.display());
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("WX_DELTA_TEST_LINK", link)
        .env("WX_DELTA_TEST_TARGET", target)
        .output()
        .unwrap();
    eprintln!(
        "status={}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success());
}

#[cfg(windows)]
#[test]
fn existing_root_rejects_junctions_at_every_directory_boundary() {
    for boundary in ["root", "ancestor", "deltas", "run"] {
        let parent = tempfile::tempdir().unwrap();
        let target = parent.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("sentinel"), b"unchanged").unwrap();
        let root = parent.path().join("root");
        let (window, _) = setup();
        let (link, output_root) = match boundary {
            "root" => (root.clone(), root),
            "ancestor" => {
                fs::create_dir(target.join("child")).unwrap();
                (root.clone(), root.join("child"))
            }
            "deltas" => {
                fs::create_dir(&root).unwrap();
                (root.join("deltas"), root)
            }
            _ => {
                fs::create_dir(&root).unwrap();
                fs::create_dir(root.join("deltas")).unwrap();
                (root.join("deltas").join(&window.run_id), root)
            }
        };
        junction(&link, &target);
        let rejected = DeltaRunWriter::create_run_in_existing_root(&output_root, window).is_err();
        fs::remove_dir(&link).unwrap();
        assert!(rejected, "{boundary}");
        assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"unchanged");
        assert_eq!(
            fs::read_dir(&target).unwrap().count(),
            if boundary == "ancestor" { 2 } else { 1 }
        );
        if boundary == "ancestor" {
            assert_eq!(fs::read_dir(target.join("child")).unwrap().count(), 0);
        }
    }
}

#[cfg(windows)]
#[test]
fn publication_rechecks_directory_after_writer_creation() {
    let parent = tempfile::tempdir().unwrap();
    let (window, chat) = setup();
    let root = parent.path().join("root");
    fs::create_dir(&root).unwrap();
    let mut writer = DeltaRunWriter::create_run_in_existing_root(&root, window.clone()).unwrap();
    let chats = root.join("deltas").join(window.run_id).join("chats");
    fs::remove_dir(&chats).unwrap();
    let target = parent.path().join("target");
    fs::create_dir(&target).unwrap();
    junction(&chats, &target);
    let result = writer.write_chat(&chat);
    fs::remove_dir(&chats).unwrap();
    assert_eq!(result["success"], false);
    assert_eq!(fs::read_dir(target).unwrap().count(), 0);
    let manifest: Value =
        serde_json::from_slice(&fs::read(writer.finish().unwrap()).unwrap()).unwrap();
    assert_eq!(manifest["errors"].as_array().unwrap().len(), 1);
}
