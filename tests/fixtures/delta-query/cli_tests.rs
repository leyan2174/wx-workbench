use super::*;
use std::fs;

fn golden() -> Value {
    serde_json::from_str(include_str!("golden.json")).unwrap()
}

#[test]
fn callback_publishes_complete_delta_and_manifest_with_query_failures() {
    let g = golden();
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("new-output");
    let window: DeltaWindow = serde_json::from_value(g["window"].clone()).unwrap();
    let mut usernames: Vec<String> = g["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["username"].as_str().unwrap().into())
        .collect();
    usernames.push(usernames[0].clone());
    let mut calls = 0;
    let result = cmd_export_delta(&output, &usernames, window.clone(), |request| {
        calls += 1;
        assert_eq!(request.start, 100);
        assert_eq!(request.end, Some(200));
        let case = g["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["username"] == request.username)
            .unwrap();
        if case["model"].is_null() {
            anyhow::bail!("no tables");
        }
        Ok(case["model"].clone())
    })
    .unwrap();
    assert_eq!(calls, 4);
    assert_eq!(result["success"], false);
    let manifest: Value =
        serde_json::from_slice(&fs::read(result["manifest_path"].as_str().unwrap()).unwrap())
            .unwrap();
    for key in [
        "schema_version",
        "export_kind",
        "run_id",
        "generated_at",
        "range",
        "chats_checked",
        "chats_with_messages",
        "messages_exported",
        "files",
    ] {
        assert_eq!(manifest[key], g["manifest"][key], "{key}");
    }
    assert_eq!(manifest["errors"][0]["username"], "wxid_missing");
    assert!(manifest["errors"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("no tables"));
    for case in &g["cases"].as_array().unwrap()[..2] {
        let path = output
            .join("deltas")
            .join(&window.run_id)
            .join(case["result"]["path"].as_str().unwrap());
        let mut actual: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        for message in actual["messages"].as_array_mut().unwrap() {
            message.as_object_mut().unwrap().remove("source");
        }
        assert_eq!(actual, case["document"]);
    }
}

#[test]
fn validation_precedes_dispatch_and_preserves_existing_outputs() {
    let g = golden();
    let mut window: DeltaWindow = serde_json::from_value(g["window"].clone()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("new-root");
    window.start = None;
    assert!(
        cmd_export_delta(&path, &["wxid_peer".into()], window.clone(), |_| panic!(
            "must not dispatch"
        ))
        .is_err()
    );
    assert!(!path.exists());
    window.start = Some(100);
    assert!(cmd_export_delta(&path, &[], window.clone(), |_| panic!("must not dispatch")).is_err());
    assert!(!path.exists());
    fs::write(
        root.path().join("full.json"),
        b"synthetic existing full export",
    )
    .unwrap();
    assert!(
        cmd_export_delta(root.path(), &["wxid_peer".into()], window, |_| panic!(
            "must not dispatch"
        ))
        .is_err()
    );
    assert_eq!(
        fs::read(root.path().join("full.json")).unwrap(),
        b"synthetic existing full export"
    );
}

#[test]
fn protected_runtime_paths_reject_before_query_and_manifest_failure_is_not_success() {
    let g = golden();
    let window: DeltaWindow = serde_json::from_value(g["window"].clone()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("account-private");
    fs::create_dir(&private).unwrap();
    let denied = private.join("archive");
    assert!(export_delta_with_mode(
        &denied,
        &["wxid_peer".into()],
        window.clone(),
        false,
        &[private],
        |_| panic!("protected request must not dispatch")
    )
    .is_err());
    assert!(!denied.exists());

    let output = root.path().join("public-output");
    let manifest = output
        .join("deltas")
        .join(&window.run_id)
        .join("manifest.json");
    let result = cmd_export_delta(&output, &["wxid_peer".into()], window, |_| {
        fs::write(&manifest, b"synthetic competing manifest")?;
        Ok(g["cases"][0]["model"].clone())
    });
    assert!(result.is_err());
    assert_eq!(
        fs::read(&manifest).unwrap(),
        b"synthetic competing manifest"
    );
}

#[test]
fn rejects_history_or_wrong_identity_and_preserves_metadata_warnings() {
    let g = golden();
    let window: DeltaWindow = serde_json::from_value(g["window"].clone()).unwrap();
    let root = tempfile::tempdir().unwrap();
    for (name, response) in [
        ("history", json!({"username":"wxid_peer", "messages":[]})),
        ("identity", g["cases"][1]["model"].clone()),
    ] {
        let result = cmd_export_delta(
            &root.path().join(name),
            &["wxid_peer".into()],
            window.clone(),
            |_| Ok(response.clone()),
        )
        .unwrap();
        assert_eq!(result["success"], false);
        assert_eq!(result["messages_exported"], 0);
        let manifest: Value =
            serde_json::from_slice(&fs::read(result["manifest_path"].as_str().unwrap()).unwrap())
                .unwrap();
        assert_eq!(manifest["errors"].as_array().unwrap().len(), 1);
    }
    let result = cmd_export_delta(
        &root.path().join("warnings"),
        &["wxid_peer".into()],
        window,
        |_| {
            let mut model = g["cases"][0]["model"].clone();
            model["metadata_warnings"] = json!(["synthetic fallback"]);
            Ok(model)
        },
    )
    .unwrap();
    assert_eq!(result["success"], true);
    assert_eq!(
        result["results"][0]["metadata_warnings"],
        json!(["synthetic fallback"])
    );
}
