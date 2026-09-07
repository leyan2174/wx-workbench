use super::*;
use std::cell::Cell;

#[test]
fn legacy_json_is_bounded_and_only_missing_files_are_optional() {
    let mut source = std::io::Cursor::new(b"[]         ");
    assert!(read_bounded_json(&mut source, 2).is_err());
    assert_eq!(source.position(), 3);
    assert_eq!(
        read_bounded_json(&b"[]"[..], 2).unwrap(),
        serde_json::json!([])
    );
    assert!(read_bounded_json(&b"["[..], 2).is_err());
    let root = tempfile::tempdir().unwrap();
    assert!(read_legacy_json(&root.path().join("missing"), 2)
        .unwrap()
        .is_none());
    assert!(read_legacy_json(root.path(), 2).is_err());
}

#[test]
fn publication_preserves_source_modified_time_for_new_and_replaced_files() {
    use std::time::{Duration, UNIX_EPOCH};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("out");
    let targets = paths(&["videos/new.mp4", "videos/replaced.mp4"]);
    let tree = fresh(&root, &targets);
    fs::write(root.join(&targets[1]), b"old video").unwrap();
    let expected = UNIX_EPOCH + Duration::new(1_234_567_890, 123_456_700);
    let source = temp.path().join("staged.mp4");
    let file = File::create(&source).unwrap();
    (&file).write_all(b"synthetic video").unwrap();
    file.set_times(fs::FileTimes::new().set_modified(expected))
        .unwrap();
    file.sync_all().unwrap();
    assert_eq!(file.metadata().unwrap().modified().unwrap(), expected);
    drop(file);
    let entries: Vec<_> = targets
        .iter()
        .map(|dest| (dest.clone(), source.clone()))
        .collect();
    tree.publish_all(&entries).unwrap();
    for target in targets {
        assert_eq!(fs::read(root.join(&target)).unwrap(), b"synthetic video");
        assert_eq!(
            fs::metadata(root.join(target)).unwrap().modified().unwrap(),
            expected
        );
    }
    assert_eq!(fs::metadata(source).unwrap().modified().unwrap(), expected);
}

#[derive(Clone, Copy, Default, Debug)]
struct Work {
    preflights: usize,
    candidates: usize,
    tree_identities: usize,
    source_guards: usize,
}

thread_local! {
    static WORK: Cell<Work> = Cell::new(Work::default());
}

pub(super) fn record_preflight(candidates: usize) {
    WORK.with(|work| {
        let mut count = work.get();
        count.preflights += 1;
        count.candidates += candidates;
        work.set(count);
    });
}

pub(super) fn record_tree_identity() {
    WORK.with(|work| {
        let mut count = work.get();
        count.tree_identities += 1;
        work.set(count);
    });
}

pub(super) fn record_source_guard() {
    WORK.with(|work| {
        let mut count = work.get();
        count.source_guards += 1;
        work.set(count);
    });
}

#[test]
fn publication_scans_candidates_twice_and_shares_source_parent_guards() {
    for count in [12, 48] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("out");
        let targets: Vec<_> = (0..count * 4)
            .map(|i| PathBuf::from(format!("images/{i}.jpg")))
            .collect();
        let tree = fresh(&root, &targets);
        let mut entries = Vec::new();
        for parent in ["staging", "staging/images", "staging/videos"] {
            fs::create_dir_all(temp.path().join(parent)).unwrap();
        }
        for (i, target) in targets.iter().take(count).enumerate() {
            let parent = ["staging", "staging/images", "staging/videos"][i % 3];
            let source = temp.path().join(parent).join(format!("{i}.tmp"));
            fs::write(&source, b"new").unwrap();
            fs::write(root.join(target), b"old").unwrap();
            entries.push((target.clone(), source));
        }
        WORK.with(|work| work.set(Work::default()));
        tree.publish_all(&entries).unwrap();
        let work = WORK.with(Cell::get);
        assert_eq!(work.preflights, 2, "{work:?}");
        assert_eq!(work.candidates, targets.len() * 2, "{work:?}");
        assert_eq!(work.source_guards, 3, "{work:?}");
        assert_eq!(work.tree_identities, count + 2, "{work:?}");
    }
}

#[test]
fn concurrent_later_target_change_stops_at_that_file_after_partial_commit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("out");
    let tree = fresh(&root, &paths(&["a", "b", "summary"]));
    let source = temp.path().join("stage");
    let protected = temp.path().join("protected");
    fs::write(&source, b"new").unwrap();
    fs::write(&protected, b"untouched").unwrap();
    fs::write(root.join("summary"), b"old summary").unwrap();
    let entries = vec![
        ("a".into(), source.clone()),
        ("b".into(), source.clone()),
        ("summary".into(), source),
    ];
    let error = tree
        .publish_with(&entries, |index| {
            if index == 0 {
                fs::hard_link(&protected, root.join("b"))?;
            }
            Ok(())
        })
        .unwrap_err();
    assert!(error.to_string().contains("1 committed"));
    assert_eq!(fs::read(root.join("a")).unwrap(), b"new");
    assert_eq!(fs::read(root.join("b")).unwrap(), b"untouched");
    assert_eq!(fs::read(&protected).unwrap(), b"untouched");
    assert_eq!(fs::read(root.join("summary")).unwrap(), b"old summary");
}

#[test]
fn denied_policy_or_binding_never_creates_planned_directories() {
    let mut album = binding();
    album.tree_kind = "album".into();
    for policy in [
        ExistingPolicy::Reject,
        ExistingPolicy::Update,
        ExistingPolicy::Adopt,
    ] {
        let temp = tempfile::tempdir().unwrap();
        drop(fresh(temp.path(), &[]));
        let before = fs::read(temp.path().join(MANIFEST)).unwrap();
        assert!(prepare(
            temp.path(),
            &album,
            policy,
            &paths(&["extra/a"]),
            &[],
            |_| panic!()
        )
        .is_err());
        for directory in ["images", "videos", "extra"] {
            assert!(!temp.path().join(directory).exists());
        }
        assert_eq!(fs::read(temp.path().join(MANIFEST)).unwrap(), before);
    }
    for policy in [ExistingPolicy::Reject, ExistingPolicy::Update] {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("old"), b"legacy").unwrap();
        assert!(prepare(temp.path(), &album, policy, &[], &[], |_| panic!()).is_err());
        assert!(!temp.path().join("images").exists());
        assert!(!temp.path().join("videos").exists());
        assert!(!temp.path().join(MANIFEST).exists());
        assert_eq!(fs::read(temp.path().join("old")).unwrap(), b"legacy");
    }
    let temp = tempfile::tempdir().unwrap();
    drop(fresh(temp.path(), &[]));
    assert!(prepare(
        temp.path(),
        &binding(),
        ExistingPolicy::Reject,
        &paths(&["images/a"]),
        &[],
        |_| panic!()
    )
    .is_err());
    assert!(!temp.path().join("images").exists());
}

#[test]
fn candidate_count_above_previous_limit_is_accepted() {
    let temp = tempfile::tempdir().unwrap();
    let targets: Vec<_> = (0..20_004)
        .map(|i| PathBuf::from(format!("images/{i}.jpg")))
        .collect();
    let tree = fresh(temp.path(), &targets);
    assert_eq!(tree.targets.len(), targets.len());
    assert!(!tree.legacy_unverified());
}

#[test]
fn fifty_thousand_media_with_four_suffixes_reach_file_preflight() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("images")).unwrap();
    // 在完整候选规划之后触发文件类型错误，避免为规模测试创建二十万个文件。
    fs::create_dir(temp.path().join("images/00000.gif")).unwrap();
    let targets: Vec<_> = (0..50_000)
        .flat_map(|i| {
            ["jpg", "png", "gif", "webp"]
                .map(move |ext| PathBuf::from(format!("images/{i:05}.{ext}")))
        })
        .collect();
    let error = prepare(
        temp.path(),
        &binding(),
        ExistingPolicy::Adopt,
        &targets,
        &[],
        |_| panic!(),
    )
    .err()
    .expect("目录不能冒充候选文件");
    assert!(format!("{error:#}").contains("replaceable target must be a file"));
    assert!(!temp.path().join(MANIFEST).exists());
}

fn binding() -> Binding {
    Binding {
        version: 1,
        tree_kind: "timeline".into(),
        source_kind: "synthetic".into(),
        source_id: "account-a".into(),
        user_name: "contact-a".into(),
    }
}
fn paths(names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(PathBuf::from).collect()
}
fn fresh(root: &Path, targets: &[PathBuf]) -> OutputTree {
    prepare(
        root,
        &binding(),
        ExistingPolicy::Reject,
        targets,
        &[],
        |_| panic!("unexpected adoption"),
    )
    .unwrap()
}

#[test]
fn fresh_nested_publish_order_and_unknown_files_survive() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("out/parent/tree");
    let targets = paths(&[
        "images/a.jpg",
        "videos/b.mp4",
        "timeline.json",
        "timeline.html",
        "export_summary.json",
    ]);
    let tree = fresh(&root, &targets);
    assert_eq!(tree.root(), root);
    assert!(!tree.legacy_unverified());
    assert!(tree.guard(Path::new("")).is_ok());
    assert!(tree.guard(Path::new("images")).is_ok());
    assert!(tree.guard(Path::new("unknown")).is_err());
    fs::write(root.join("old.part"), b"unrelated").unwrap();
    let entries: Vec<_> = targets
        .iter()
        .enumerate()
        .map(|(i, target)| {
            let source = temp.path().join(format!("stage-{i}"));
            fs::write(&source, format!("payload-{i}")).unwrap();
            (target.clone(), source)
        })
        .collect();
    tree.publish_all(&entries).unwrap();
    for (i, target) in targets.iter().enumerate() {
        assert_eq!(
            fs::read(root.join(target)).unwrap(),
            format!("payload-{i}").as_bytes()
        );
    }
    assert_eq!(fs::read(root.join("old.part")).unwrap(), b"unrelated");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(MANIFEST)).unwrap()).unwrap();
    assert_eq!(manifest["version"], 1);
    assert_eq!(manifest["legacy_unverified"], false);
}

#[test]
fn policy_matrix_and_binding_conflicts() {
    for policy in [
        ExistingPolicy::Reject,
        ExistingPolicy::Update,
        ExistingPolicy::Adopt,
    ] {
        let temp = tempfile::tempdir().unwrap();
        drop(prepare(temp.path(), &binding(), policy, &[], &[], |_| panic!()).unwrap());
        assert!(prepare(
            temp.path(),
            &binding(),
            ExistingPolicy::Reject,
            &[],
            &[],
            |_| panic!()
        )
        .is_err());
        for policy in [ExistingPolicy::Update, ExistingPolicy::Adopt] {
            drop(prepare(temp.path(), &binding(), policy, &[], &[], |_| panic!()).unwrap());
            for field in 0..5 {
                let mut other = binding();
                match field {
                    0 => other.version = 2,
                    1 => other.tree_kind = "album".into(),
                    2 => other.source_kind = "db".into(),
                    3 => other.source_id = "account-b".into(),
                    _ => other.user_name = "contact-b".into(),
                }
                assert!(prepare(temp.path(), &other, policy, &[], &[], |_| panic!()).is_err());
            }
        }
    }
}

#[test]
fn legacy_adoption_only_after_preflight_and_under_lock() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("timeline.json"),
        br#"{"user_name":"contact-a"}"#,
    )
    .unwrap();
    for policy in [ExistingPolicy::Reject, ExistingPolicy::Update] {
        assert!(prepare(temp.path(), &binding(), policy, &[], &[], |_| panic!()).is_err());
        assert!(!temp.path().join(MANIFEST).exists());
    }
    let called = Cell::new(0);
    let tree = prepare(
        temp.path(),
        &binding(),
        ExistingPolicy::Adopt,
        &[],
        &[],
        |root| {
            called.set(called.get() + 1);
            assert!(prepare(
                root,
                &binding(),
                ExistingPolicy::Adopt,
                &[],
                &[],
                |_| panic!()
            )
            .is_err());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(called.get(), 1);
    assert!(tree.legacy_unverified());
    drop(tree);
    assert!(prepare(
        temp.path(),
        &binding(),
        ExistingPolicy::Update,
        &[],
        &[],
        |_| panic!()
    )
    .unwrap()
    .legacy_unverified());
}

#[test]
fn rejected_adoption_and_corrupt_manifests_never_overwrite() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("old"), b"legacy").unwrap();
    assert!(prepare(
        temp.path(),
        &binding(),
        ExistingPolicy::Adopt,
        &[],
        &[],
        |_| anyhow::bail!("known contact conflict")
    )
    .is_err());
    assert!(!temp.path().join(MANIFEST).exists());
    for data in [b"broken".as_slice(), br#"{"version":1}"#] {
        fs::write(temp.path().join(MANIFEST), data).unwrap();
        assert!(prepare(
            temp.path(),
            &binding(),
            ExistingPolicy::Adopt,
            &[],
            &[],
            |_| panic!()
        )
        .is_err());
        assert_eq!(fs::read(temp.path().join(MANIFEST)).unwrap(), data);
    }
}

#[test]
fn unsafe_duplicate_and_case_alias_targets_fail_before_manifest() {
    let cases = [
        paths(&["a", "a"]),
        paths(&["a", "A"]),
        paths(&["Images/a", "images/b"]),
        paths(&["images", "images/a"]),
        paths(&["../a"]),
        paths(&["a/../b"]),
        paths(&["./a"]),
        paths(&["a//b"]),
        paths(&["NUL.txt"]),
        paths(&["a:stream"]),
        paths(&["a."]),
        paths(&["a "]),
        paths(&["C:\\escape"]),
        paths(&["/escape"]),
        paths(&["a/b/c/d/e"]),
        paths(&[MANIFEST]),
        paths(&["_SOURCE_BINDING.JSON"]),
        paths(&[LOCK]),
    ];
    for targets in cases {
        let temp = tempfile::tempdir().unwrap();
        assert!(
            prepare(
                temp.path(),
                &binding(),
                ExistingPolicy::Adopt,
                &targets,
                &[],
                |_| panic!()
            )
            .is_err(),
            "{targets:?}"
        );
        assert!(!temp.path().join(MANIFEST).exists());
    }
}

#[test]
fn all_candidates_and_sidecars_preflight_even_when_not_published() {
    for name in [
        "images/a.png",
        "videos/a.mp4.part",
        "timeline.html",
        MANIFEST,
        LOCK,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("out");
        fs::create_dir_all(root.join("images")).unwrap();
        fs::create_dir_all(root.join("videos")).unwrap();
        let protected = temp.path().join("source");
        fs::write(&protected, b"never change").unwrap();
        fs::hard_link(&protected, root.join(name)).unwrap();
        let targets = paths(&[
            "images/a.jpg",
            "images/a.png",
            "videos/a.mp4.part",
            "timeline.html",
        ]);
        assert!(prepare(
            &root,
            &binding(),
            ExistingPolicy::Adopt,
            &targets,
            &[],
            |_| panic!()
        )
        .is_err());
        assert_eq!(fs::read(&protected).unwrap(), b"never change");
        if name != MANIFEST {
            assert!(!root.join(MANIFEST).exists());
        }
    }
}

#[test]
fn inputs_overlap_and_target_directory_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("out");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("source.db"), b"source").unwrap();
    for input in [
        root.clone(),
        root.join("source.db"),
        temp.path().to_path_buf(),
    ] {
        assert!(prepare(
            &root,
            &binding(),
            ExistingPolicy::Adopt,
            &[],
            &[input],
            |_| panic!()
        )
        .is_err());
    }
    fs::create_dir(root.join("timeline.json")).unwrap();
    assert!(prepare(
        &root,
        &binding(),
        ExistingPolicy::Adopt,
        &paths(&["timeline.json"]),
        &[],
        |_| panic!()
    )
    .is_err());
}

#[test]
fn late_candidate_hardlink_and_bad_staged_source_prevent_all_commits() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("out");
    let tree = fresh(&root, &paths(&["a", "b", "unused.part"]));
    fs::write(root.join("a"), b"old").unwrap();
    let source = temp.path().join("stage");
    fs::write(&source, b"new").unwrap();
    let entries = vec![
        ("a".into(), source.clone()),
        ("b".into(), temp.path().join("missing")),
    ];
    assert!(tree
        .publish_all(&entries)
        .unwrap_err()
        .to_string()
        .contains("0 committed"));
    assert_eq!(fs::read(root.join("a")).unwrap(), b"old");
    fs::hard_link(&source, root.join("unused.part")).unwrap();
    assert!(tree.publish_all(&entries[..1]).is_err());
    assert_eq!(fs::read(root.join("a")).unwrap(), b"old");
}

#[test]
fn failure_reports_partial_commit_keeps_old_target_and_stops_summary() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("out");
    let targets = paths(&[
        "media",
        "timeline.json",
        "timeline.html",
        "export_summary.json",
    ]);
    let tree = fresh(&root, &targets);
    let entries: Vec<_> = targets
        .iter()
        .enumerate()
        .map(|(i, name)| {
            fs::write(root.join(name), b"old").unwrap();
            let source = temp.path().join(format!("stage-{i}"));
            fs::write(&source, b"new").unwrap();
            (name.clone(), source)
        })
        .collect();
    let error = tree
        .publish_with(&entries, |i| {
            ensure!(i != 1, "synthetic commit fault");
            Ok(())
        })
        .unwrap_err();
    assert!(error.to_string().contains("1 committed"));
    assert_eq!(fs::read(root.join("media")).unwrap(), b"new");
    for name in &targets[1..] {
        assert_eq!(fs::read(root.join(name)).unwrap(), b"old");
    }
    tree.publish_all(&entries).unwrap();
    for name in &targets {
        assert_eq!(fs::read(root.join(name)).unwrap(), b"new");
    }
}

#[test]
fn staged_links_unplanned_duplicates_and_output_as_source_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("out");
    let tree = fresh(&root, &paths(&["a"]));
    let source = temp.path().join("stage");
    fs::write(&source, b"new").unwrap();
    fs::write(root.join("a"), b"old").unwrap();
    for entries in [
        vec![("b".into(), source.clone())],
        vec![("a".into(), root.join("a"))],
        vec![("a".into(), source.clone()), ("a".into(), source.clone())],
    ] {
        assert!(tree.publish_all(&entries).is_err());
    }
    fs::hard_link(&source, temp.path().join("alias")).unwrap();
    assert!(tree.publish_all(&[("a".into(), source)]).is_err());
    assert_eq!(fs::read(root.join("a")).unwrap(), b"old");
}

#[test]
fn reparse_roots_parents_and_sources_rejected() {
    use std::process::Command;
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let junction = temp.path().join("junction");
    assert!(Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .unwrap()
        .status
        .success());
    assert!(prepare(
        &junction.join("tree"),
        &binding(),
        ExistingPolicy::Reject,
        &[],
        &[],
        |_| panic!()
    )
    .is_err());
    assert!(!outside.join("tree").exists());
    let root = temp.path().join("out");
    fs::create_dir(&root).unwrap();
    assert!(Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(root.join("images"))
        .arg(&outside)
        .output()
        .unwrap()
        .status
        .success());
    assert!(prepare(
        &root,
        &binding(),
        ExistingPolicy::Adopt,
        &paths(&["images/a"]),
        &[],
        |_| panic!()
    )
    .is_err());
}

#[test]
fn missing_output_never_created_inside_protected_source() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("must-not-exist/tree");
    assert!(prepare(
        &root,
        &binding(),
        ExistingPolicy::Reject,
        &[],
        &[temp.path().into()],
        |_| panic!()
    )
    .is_err());
    assert!(!temp.path().join("must-not-exist").exists());
}

#[test]
fn actual_replace_failure_keeps_old_target_and_can_retry() {
    use std::{cell::RefCell, os::windows::fs::OpenOptionsExt};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("out");
    let tree = fresh(
        &root,
        &paths(&["media", "timeline.html", "export_summary.json"]),
    );
    let mut entries = Vec::new();
    for (i, target) in ["media", "timeline.html", "export_summary.json"]
        .iter()
        .enumerate()
    {
        fs::write(root.join(target), b"old").unwrap();
        let source = temp.path().join(format!("staged-{i}"));
        fs::write(&source, b"new").unwrap();
        entries.push((PathBuf::from(target), source));
    }
    let held = RefCell::new(None);
    let error = tree
        .publish_with(&entries, |index| {
            if index == 1 {
                *held.borrow_mut() = Some(
                    OpenOptions::new()
                        .read(true)
                        .share_mode(1)
                        .open(root.join("timeline.html"))?,
                );
            }
            Ok(())
        })
        .unwrap_err();
    assert!(error.to_string().contains("1 committed"));
    assert!(format!("{error:#}").contains("SNS file replacement failed"));
    assert_eq!(fs::read(root.join("media")).unwrap(), b"new");
    assert_eq!(fs::read(root.join("timeline.html")).unwrap(), b"old");
    assert_eq!(fs::read(root.join("export_summary.json")).unwrap(), b"old");
    drop(held);
    tree.publish_all(&entries).unwrap();
    assert_eq!(fs::read(root.join("export_summary.json")).unwrap(), b"new");
}

#[test]
fn unknown_subtree_not_scanned_and_prepared_directories_stay_pinned() {
    use std::process::Command;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("out");
    let outside = temp.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("unknown"), b"unchanged").unwrap();
    assert!(Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(root.join("unknown-subtree"))
        .arg(&outside)
        .output()
        .unwrap()
        .status
        .success());
    let tree = prepare(
        &root,
        &binding(),
        ExistingPolicy::Adopt,
        &paths(&["images/a"]),
        &[],
        |_| Ok(()),
    )
    .unwrap();
    assert!(tree.legacy_unverified());
    tree.verify_all().unwrap();
    let source = temp.path().join("stage");
    fs::write(&source, b"new").unwrap();
    if fs::rename(root.join("images"), root.join("renamed")).is_ok() {
        assert!(tree.verify_all().is_err());
        assert!(tree.publish_all(&[("images/a".into(), source)]).is_err());
        assert!(!root.join("renamed/a").exists());
    } else {
        tree.verify_all().unwrap();
    }
    assert_eq!(fs::read(outside.join("unknown")).unwrap(), b"unchanged");
}

#[test]
fn staged_directory_and_reparse_parent_preflight_before_any_publication() {
    use std::process::Command;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("out");
    let tree = fresh(&root, &paths(&["a", "b"]));
    fs::write(root.join("a"), b"old").unwrap();
    let source = temp.path().join("stage");
    fs::write(&source, b"new").unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("source"), b"second").unwrap();
    let junction = temp.path().join("junction");
    assert!(Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .unwrap()
        .status
        .success());
    for second in [outside, junction.join("source")] {
        let err = tree
            .publish_all(&[("a".into(), source.clone()), ("b".into(), second)])
            .unwrap_err();
        assert!(err.to_string().contains("0 committed"));
        assert_eq!(fs::read(root.join("a")).unwrap(), b"old");
    }
}

#[test]
fn empty_album_always_prepares_media_directories_without_placeholders() {
    let temp = tempfile::tempdir().unwrap();
    let mut album = binding();
    album.tree_kind = "album".into();
    for targets in [
        Vec::new(),
        paths(&["timeline.json", "timeline.html", "export_summary.json"]),
    ] {
        let root = temp.path().join(format!("album-{}", targets.len()));
        let tree = prepare(
            &root,
            &album,
            ExistingPolicy::Reject,
            &targets,
            &[],
            |_| panic!(),
        )
        .unwrap();
        for parent in ["images", "videos"] {
            assert_eq!(
                tree.guard(Path::new(parent)).unwrap().output_root(),
                root.join(parent)
            );
            assert_eq!(fs::read_dir(root.join(parent)).unwrap().count(), 0);
        }
        tree.publish_all(&[]).unwrap();
    }
    let root = temp.path().join("timeline");
    let tree = fresh(&root, &[]);
    assert!(tree.guard(Path::new("images")).is_err());
    assert!(!root.join("images").exists());
    assert!(!root.join("videos").exists());
}

#[test]
fn empty_album_media_directories_are_preflighted() {
    let mut album = binding();
    album.tree_kind = "album".into();
    for name in ["images", "videos"] {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(name), b"not a directory").unwrap();
        assert!(prepare(
            temp.path(),
            &album,
            ExistingPolicy::Adopt,
            &[],
            &[],
            |_| panic!()
        )
        .is_err());
        assert!(!temp.path().join(MANIFEST).exists());
        assert_eq!(
            fs::read(temp.path().join(name)).unwrap(),
            b"not a directory"
        );
    }
}
