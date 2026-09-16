use super::*;
use serde_json::json;

#[test]
fn refuses_callback_changes_even_with_same_owner() {
    for mode in ["owner", "content", "replace", "delete", "create"] {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.json");
        let output = dir.path().join("output.json");
        let original = serde_json::to_vec(&chat()).unwrap();
        fs::write(&input, &original).unwrap();
        if mode != "create" {
            fs::write(&output, &original).unwrap();
        }
        let mut changed = chat();
        if mode == "owner" {
            changed["username"] = json!("bob");
        }
        changed["sentinel"] = json!("external edit");
        let changed = serde_json::to_vec(&changed).unwrap();
        let mut once = false;
        let result = transcribe_file(&input, &output, |_| {
            if !once {
                once = true;
                match mode {
                    "delete" => fs::remove_file(&output).unwrap(),
                    "replace" => {
                        fs::remove_file(&output).unwrap();
                        fs::write(&output, &original).unwrap();
                    }
                    _ => fs::write(&output, &changed).unwrap(),
                }
            }
            Ok("transcript".into())
        });
        assert!(result.is_err(), "mode {mode}");
        match mode {
            "delete" => assert!(!output.exists()),
            "replace" => assert_eq!(fs::read(&output).unwrap(), original),
            _ => assert_eq!(fs::read(&output).unwrap(), changed),
        }
        assert_eq!(fs::read(&input).unwrap(), original);
        assert!(!fs::read_dir(dir.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("wx-asr")));
    }
}

#[test]
fn cooperating_publisher_is_rejected_before_callback() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.json");
    let output = dir.path().join("output.json");
    fs::write(&input, serde_json::to_vec(&chat()).unwrap()).unwrap();
    transcribe_file(&input, &output, |_| {
        let error = transcribe_file(&input, &output, |_| panic!("locked callback must not run"))
            .unwrap_err();
        assert!(error.to_string().contains("locked"));
        Ok("ok".into())
    })
    .unwrap();
}

#[test]
fn checks_again_after_serialization_before_publication() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("output.json");
    fs::write(&output, "old").unwrap();
    let result = atomic_write(&output, |file| {
        file.write_all(b"new")?;
        fs::write(&output, "external")?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(fs::read(&output).unwrap(), b"external");
}

fn chat() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/asr-writeback/chat.json"
    ))
    .unwrap()
}

#[test]
fn preserves_unknowns_and_cross_shard_identity() {
    let mut data = chat();
    let original = data.clone();
    let mut seen = Vec::new();
    let report = transcribe_json(&mut data, |id| {
        seen.push(id.clone());
        Ok(format!("{}:{}:{}", id.username, id.source, id.local_id))
    })
    .unwrap();
    assert_eq!(report.transcribed, 2);
    assert_eq!(report.skipped_non_voice, 2);
    assert_ne!(seen[0], seen[1]);
    assert_ne!(
        data["messages"][0]["transcription"],
        data["messages"][1]["transcription"]
    );
    for message in data["messages"].as_array_mut().unwrap() {
        if let Some(object) = message.as_object_mut() {
            object.remove("transcription");
        }
    }
    assert_eq!(data, original);
}

#[test]
fn existing_fields_including_null_are_skipped() {
    let mut data = json!({"username":"synthetic","messages":[
        {"type":"voice","transcription":null},
        {"type":"voice","transcription":""},
        {"type":"voice","transcription":"done"},
        {"type":"voice","transcription":{"custom":true}}
    ]});
    let original = data.clone();
    let report = transcribe_json(&mut data, |_| panic!("must skip")).unwrap();
    assert_eq!(report.skipped_existing, 4);
    assert_eq!(data, original);
}

#[test]
fn invalid_identities_never_reach_callback() {
    let mut data = json!({"username":"synthetic","messages":[
        {"type":"voice","source":null,"local_id":1},
        {"type":"voice","local_id":1},
        {"type":"voice","source":"a","local_id":null},
        {"type":"voice","source":"a","local_id":"1"},
        {"type":"voice","source":"a","local_id":0},
        {"type":"voice","source":"a","local_id":-1},
        {"type":"voice","source":"a","local_id":1.5},
        {"type":"voice","source":"a","local_id":18446744073709551615_u64},
        {"type":"voice","source":"a","local_id":1,"username":"other"}
    ]});
    let original = data.clone();
    let report = transcribe_json(&mut data, |_| panic!("invalid identity")).unwrap();
    assert_eq!(report.failed, 9);
    assert_eq!(report.errors.len(), 9);
    assert_eq!(data, original);
}

#[test]
fn malformed_root_is_transactional() {
    for mut data in [
        Value::Null,
        json!({"messages":[]}),
        json!({"username":null,"messages":[]}),
        json!({"username":"x","messages":null}),
    ] {
        let original = data.clone();
        assert!(transcribe_json(&mut data, |_| panic!("invalid root")).is_err());
        assert_eq!(data, original);
    }
}

#[test]
fn failures_are_counted_and_retryable() {
    let mut data = chat();
    let original = data.clone();
    let report = transcribe_json(&mut data, |id| {
        if id.source == "message_0.db" {
            anyhow::bail!("synthetic failure");
        }
        Ok("  ".into())
    })
    .unwrap();
    assert_eq!(report.failed, 2);
    assert_eq!(report.errors[0].index, 0);
    assert_eq!(data, original);
    assert_eq!(
        transcribe_json(&mut data, |_| Ok("ok".into()))
            .unwrap()
            .transcribed,
        2
    );
    assert_eq!(
        transcribe_json(&mut data, |_| panic!("already done"))
            .unwrap()
            .skipped_existing,
        2
    );
}

#[test]
fn one_failure_does_not_block_later_success() {
    let mut data = chat();
    let report = transcribe_json(&mut data, |id| {
        if id.source == "message_0.db" {
            anyhow::bail!("failed");
        }
        Ok("success".into())
    })
    .unwrap();
    assert_eq!((report.failed, report.transcribed), (1, 1));
    assert!(data["messages"][0].get("transcription").is_none());
    assert_eq!(data["messages"][1]["transcription"], "success");
}

#[test]
fn separate_output_preserves_input_and_same_path_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.json");
    let output = dir.path().join("output.json");
    let bytes = serde_json::to_vec(&chat()).unwrap();
    fs::write(&input, &bytes).unwrap();
    assert_eq!(
        transcribe_file(&input, &output, |_| Ok("ok".into()))
            .unwrap()
            .transcribed,
        2
    );
    assert_eq!(fs::read(&input).unwrap(), bytes);
    assert_eq!(
        transcribe_file(&output, &output, |_| panic!("resume"))
            .unwrap()
            .skipped_existing,
        2
    );
    assert_eq!(
        transcribe_file(&input, &input, |_| Ok("in place".into()))
            .unwrap()
            .transcribed,
        2
    );
}

#[test]
fn rejects_database_and_other_chat_before_callback() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.json");
    fs::write(&input, serde_json::to_vec(&chat()).unwrap()).unwrap();
    for (name, bytes) in [
        ("source.db", b"SQLite format 3\0".as_slice()),
        ("source.json", b"SQLite format 3\0".as_slice()),
        (
            "other.json",
            br#"{"username":"other","messages":[]}"#.as_slice(),
        ),
    ] {
        let output = dir.path().join(name);
        fs::write(&output, bytes).unwrap();
        assert!(transcribe_file(&input, &output, |_| panic!("unsafe target")).is_err());
        assert_eq!(fs::read(&output).unwrap(), bytes);
    }
}

#[test]
fn partial_write_failure_keeps_old_bytes_and_cleans_temp() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("output.json");
    fs::write(&output, b"old bytes").unwrap();
    assert!(atomic_write(&output, |file| {
        file.write_all(b"partial new bytes")?;
        anyhow::bail!("injected disk failure")
    })
    .is_err());
    assert_eq!(fs::read(&output).unwrap(), b"old bytes");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn hard_link_output_does_not_truncate_source() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.json");
    let output = dir.path().join("linked.json");
    let bytes = serde_json::to_vec(&chat()).unwrap();
    fs::write(&input, &bytes).unwrap();
    fs::hard_link(&input, &output).unwrap();
    transcribe_file(&input, &output, |_| Ok("new".into())).unwrap();
    assert_eq!(fs::read(&input).unwrap(), bytes);
    assert_ne!(fs::read(&output).unwrap(), bytes);
}

#[cfg(windows)]
#[test]
fn locked_target_replace_failure_preserves_old_results() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("output.json");
    let bytes = serde_json::to_vec(&chat()).unwrap();
    fs::write(&output, &bytes).unwrap();
    // 允许读写但禁止删除共享，使最后的原子替换失败。
    let _lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(&output)
        .unwrap();
    assert!(transcribe_file(&output, &output, |_| Ok("new".into())).is_err());
    assert_eq!(fs::read(&output).unwrap(), bytes);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}
