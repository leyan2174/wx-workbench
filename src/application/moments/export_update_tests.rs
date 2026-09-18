use super::*;
use serde_json::{json, Value};

fn golden() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/sns-timeline-upsert-oracle/golden.json"
    )))
    .unwrap()
}

fn data() -> ExportData {
    ExportData {
        timelines: vec![serde_json::from_value(golden()["second_summary"].clone()).unwrap()],
        ..Default::default()
    }
}

fn policy(source: &str, policy: publish::ExistingPolicy) -> TimelinePublication {
    TimelinePublication {
        flat_cache: false,
        source_kind: "snapshot".into(),
        source_id: source.into(),
        policy,
        inputs: Vec::new(),
    }
}

fn write(data: &ExportData, output: &Path, policy: &TimelinePublication) -> Result<ExportReport> {
    write_export_with_publication(
        data,
        output,
        TimeZone::Fixed(chrono::FixedOffset::east_opt(8 * 3600).unwrap()),
        None,
        None,
        Some(policy),
        None,
    )
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn no_cache_fresh_rejects_entry_and_final_rename_verification() {
    for fail_on in [1, 2] {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("out");
        let current = data();
        let calls = std::cell::Cell::new(0);
        let verify = || -> Result<()> {
            calls.set(calls.get() + 1);
            ensure!(
                calls.get() != fail_on,
                "synthetic source changed before fresh rename"
            );
            Ok(())
        };
        let error = write_export_with_publication(
            &current,
            &output,
            TimeZone::Fixed(chrono::FixedOffset::east_opt(0).unwrap()),
            None,
            None,
            None,
            Some(&verify),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("synthetic source changed"));
        assert_eq!(calls.get(), fail_on);
        let author = output.join(&current.timelines[0].display_name);
        assert!(!author.join("SNS").exists());
        assert_eq!(
            fs::read_dir(author).unwrap().count(),
            0,
            "private staging must be removed"
        );
    }
}

#[test]
fn no_cache_update_second_file_rejection_preserves_first_commit_and_old_later_files() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("out");
    let mut current = data();
    let publication = policy("source", publish::ExistingPolicy::Update);
    let report = write(&current, &output, &publication).unwrap();
    let before: Vec<_> = report
        .files
        .iter()
        .map(|path| (path.clone(), fs::read(path).unwrap()))
        .collect();
    let first_post = report
        .files
        .iter()
        .find(|path| {
            path.extension().is_some_and(|ext| ext == "json") && !path.ends_with("timeline.json")
        })
        .unwrap();
    for post in &mut current.timelines[0].posts {
        post.content_desc = "synthetic changed content".into();
    }
    let calls = std::cell::Cell::new(0);
    let verify = || -> Result<()> {
        calls.set(calls.get() + 1);
        // Entry, first persist, then reject immediately before the second persist.
        ensure!(
            calls.get() != 3,
            "synthetic revision changed at second file"
        );
        Ok(())
    };
    let error = write_export_with_publication(
        &current,
        &output,
        TimeZone::Fixed(chrono::FixedOffset::east_opt(8 * 3600).unwrap()),
        None,
        None,
        Some(&publication),
        Some(&verify),
    )
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("1 committed file(s)"));
    assert!(message.contains("synthetic revision changed at second file"));
    assert_eq!(calls.get(), 3);
    for (path, bytes) in before {
        if &path == first_post {
            assert_ne!(fs::read(&path).unwrap(), bytes);
            assert_eq!(read(&path)["content_desc"], "synthetic changed content");
        } else {
            assert_eq!(
                fs::read(path).unwrap(),
                bytes,
                "uncommitted output must retain old bytes"
            );
        }
    }
}

#[test]
fn updated_summary_matches_legacy_oracle_without_merging_or_deleting_old_files() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("out");
    let current = data();
    let mut first = current.clone();
    let mut old = first.timelines[0].posts[0].clone();
    old.tid = Some(2);
    old.create_time -= 100;
    old.content_desc = "OLD POST".into();
    first.timelines[0].posts.push(old);
    first.timelines[0].total_posts = 2;
    let publication = policy("source", publish::ExistingPolicy::Update);
    write(&first, &output, &publication).unwrap();
    let root = output.join("Synthetic Author/SNS");
    fs::create_dir(root.join("notes")).unwrap();
    for name in [
        "notes/keep.txt",
        "unrelated.bin",
        "20231115061140000_0.png",
        "20231115061320000_0.png",
    ] {
        fs::write(root.join(name), b"keep old bytes").unwrap();
    }
    let old_json = fs::read(root.join("20231115061140000.json")).unwrap();
    let report = write(&current, &output, &publication).unwrap();
    assert_eq!(report.posts, 1);
    assert_eq!(report.legacy_unverified, 0);
    assert_eq!(
        read(&root.join("timeline.json")),
        golden()["second_summary"]
    );
    assert_eq!(
        read(&root.join("20231115061320000.json")),
        golden()["second_summary"]["posts"][0]
    );
    assert_eq!(
        fs::read(root.join("20231115061140000.json")).unwrap(),
        old_json
    );
    for name in golden()["second_files"].as_array().unwrap() {
        assert!(root.join(name.as_str().unwrap()).is_file());
    }
    let html = fs::read_to_string(root.join("timeline.html")).unwrap();
    assert!(html.contains("h1{overflow-wrap:anywhere}"));
    assert!(html.contains("CURRENT AFTER"));
    assert!(!html.contains("OLD POST"));
    assert!(!html.contains("<img "), "旧同名图片不能假装本轮恢复成功");
    assert_eq!(
        fs::read(root.join("20231115061320000_0.png")).unwrap(),
        b"keep old bytes"
    );
}

#[test]
fn timestamp_collisions_match_run_local_legacy_assignment() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("out");
    let mut first = data();
    let post = &mut first.timelines[0].posts[0];
    post.create_time = 0;
    post.tid = Some(10);
    let mut second = post.clone();
    second.tid = Some(11);
    first.timelines[0].posts.push(second.clone());
    first.timelines[0].total_posts = 2;
    let publication = policy("source", publish::ExistingPolicy::Update);
    write(&first, &output, &publication).unwrap();
    let root = output.join("Synthetic Author/SNS");
    for (name, tid) in golden()["collision_first"].as_object().unwrap() {
        assert_eq!(&read(&root.join(name))["tid"], tid);
    }
    first.timelines[0].posts = vec![second];
    first.timelines[0].total_posts = 1;
    write(&first, &output, &publication).unwrap();
    for (name, tid) in golden()["collision_second"].as_object().unwrap() {
        assert_eq!(&read(&root.join(name))["tid"], tid);
    }
}

#[test]
fn adoption_validates_summary_and_stale_post_database_authors() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let valid = golden()["second_summary"].clone();
    fs::write(
        root.join("timeline.json"),
        serde_json::to_vec(&valid).unwrap(),
    )
    .unwrap();
    legacy_timeline_identity(root, "db_author").unwrap();
    assert!(legacy_timeline_identity(root, "xml_other").is_err());
    fs::write(
        root.join("20000101000000000.json"),
        br#"{"db_user_name":"other"}"#,
    )
    .unwrap();
    assert!(legacy_timeline_identity(root, "db_author").is_err());
    fs::write(
        root.join("20000101000000000.json"),
        br#"{"db_user_name":"db_author","username":"xml_other"}"#,
    )
    .unwrap();
    legacy_timeline_identity(root, "db_author").unwrap();
    fs::write(root.join("timeline.json"), b"[]").unwrap();
    assert!(legacy_timeline_identity(root, "db_author").is_err());
    fs::write(
        root.join("timeline.json"),
        br#"{"user_name":"db_author","posts":[{"db_user_name":"other"}]}"#,
    )
    .unwrap();
    assert!(legacy_timeline_identity(root, "db_author").is_err());
    assert!(legacy_post_identity(&json!({"db_user_name":""}), "unknown").is_ok());
    assert!(legacy_post_identity(&json!({"db_user_name":null}), "unknown").is_err());
}

#[test]
fn later_contact_conflict_cannot_modify_earlier_contact_content() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("out");
    let mut first = data();
    first.timelines[0].display_name = "A".into();
    let mut second = data();
    second.timelines[0].display_name = "B".into();
    let ours = policy("source-a", publish::ExistingPolicy::Update);
    write(&first, &output, &ours).unwrap();
    write(
        &second,
        &output,
        &policy("source-b", publish::ExistingPolicy::Update),
    )
    .unwrap();
    let old = fs::read(output.join("A/SNS/timeline.json")).unwrap();
    first.timelines[0].posts[0].content_desc = "must not commit".into();
    first.timelines.push(second.timelines.remove(0));
    assert!(write(&first, &output, &ours).is_err());
    assert_eq!(fs::read(output.join("A/SNS/timeline.json")).unwrap(), old);
}

#[test]
fn every_media_format_is_planned_and_dangerous_unused_candidate_blocks_publication() {
    let temp = tempfile::tempdir().unwrap();
    let current = data();
    let names = post_names(
        &current.timelines[0],
        TimeZone::Fixed(chrono::FixedOffset::east_opt(8 * 3600).unwrap()),
    )
    .unwrap();
    let targets = publication_targets(&current.timelines[0], &names, true, true, false);
    let stem = &names[0].1;
    for extension in ["jpg", "png", "gif", "webp", "mp4", "mov", "bin"] {
        assert!(targets.contains(&PathBuf::from(format!("{stem}_0.{extension}"))));
    }
    for extension in ["jpg", "png", "gif", "webp"] {
        assert!(targets.contains(&PathBuf::from(format!("images/{stem}_0.{extension}"))));
    }
    assert!(targets.contains(&PathBuf::from(format!("videos/{stem}_0.mp4"))));
    let flat = publication_targets(&current.timelines[0], &names, true, false, true);
    for extension in ["jpg", "png", "gif", "webp", "mp4"] {
        assert!(flat.contains(&PathBuf::from(format!("{stem}_0.{extension}"))));
    }
    assert!(flat.iter().all(|path| path.components().count() == 1));
    let source = temp.path().join("protected.db");
    fs::write(&source, b"protected source").unwrap();
    let output = temp.path().join("out");
    let root = output.join("Synthetic Author/SNS");
    fs::create_dir_all(&root).unwrap();
    fs::hard_link(&source, root.join(format!("{stem}_0.mov"))).unwrap();
    let mut publication = policy("source", publish::ExistingPolicy::Adopt);
    publication.inputs.push(source.clone());
    assert!(write_export_with_publication(
        &current,
        &output,
        TimeZone::Fixed(chrono::FixedOffset::east_opt(8 * 3600).unwrap()),
        None,
        Some(&DownloadOptions::default()),
        Some(&publication),
        None,
    )
    .is_err());
    assert!(!root.join("timeline.json").exists());
    assert!(!root.join("_source_binding.json").exists());
    assert_eq!(fs::read(source).unwrap(), b"protected source");
}

#[test]
fn empty_filtered_export_leaves_output_absent() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("not-created");
    assert_eq!(
        write(
            &ExportData::default(),
            &output,
            &policy("source", publish::ExistingPolicy::Update)
        )
        .unwrap()
        .contacts,
        0
    );
    assert!(!output.exists());
}
