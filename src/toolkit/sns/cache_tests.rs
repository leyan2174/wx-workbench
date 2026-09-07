use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn golden() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/sns/cache_golden.json"
    ))
    .unwrap()
}
fn unhex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
        .chunks_exact(2)
        .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
        .collect()
}
fn key(value: &Value) -> Option<[u8; 16]> {
    value.as_str().map(|s| unhex(s).try_into().unwrap())
}
fn keys() -> CacheKeys {
    CacheKeys {
        image_aes_key: key(&golden()["key_hex"]),
        image_xor_key: 0x88,
    }
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "wx-sns-cache-synthetic-{}-{}-{}",
            std::process::id(),
            nonce,
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        // 仅删除本测试通过 create_dir 创建的唯一临时根；不使用用户输入路径。
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> (Temp, CacheRoots, CacheIndex) {
    let temp = Temp::new();
    let golden = golden();
    for file in golden["files"].as_array().unwrap() {
        let path = temp.0.join(file["path"].as_str().unwrap());
        assert!(!file["path"].as_str().unwrap().contains(".."));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut target = File::create(&path).unwrap();
        target
            .write_all(&unhex(file["input_hex"].as_str().unwrap()))
            .unwrap();
        let time = UNIX_EPOCH + std::time::Duration::from_secs(file["mtime"].as_u64().unwrap());
        target
            .set_times(fs::FileTimes::new().set_modified(time).set_accessed(time))
            .unwrap();
    }
    let roots = CacheRoots {
        xwechat: Some(temp.0.join("xwechat")),
        file_storage_sns: Some(temp.0.join("storage")),
    };
    let index = build_cache_index(&roots, &keys(), CacheLimits::default()).unwrap();
    (temp, roots, index)
}
fn relative(temp: &Temp, path: &Path) -> String {
    // Windows canonicalize 会增加设备前缀，两个路径均规范化后再比较。
    let root = fs::canonicalize(&temp.0).unwrap();
    fs::canonicalize(path)
        .unwrap()
        .strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn dat_decryption_matches_legacy_synthetic_vectors() {
    let golden = golden();
    for case in golden["decrypt_cases"].as_array().unwrap() {
        let keys = CacheKeys {
            image_aes_key: key(&case["key_hex"]),
            image_xor_key: case["xor_key"].as_u64().unwrap() as u8,
        };
        let result = decrypt_dat(
            &unhex(case["input_hex"].as_str().unwrap()),
            &keys,
            1024 * 1024,
        );
        if case["strict_reject"] == true || case["expected_hex"].is_null() {
            assert!(result.is_err(), "{}", case["name"]);
        } else {
            assert_eq!(
                result.unwrap(),
                unhex(case["expected_hex"].as_str().unwrap()),
                "{}",
                case["name"]
            );
        }
    }
}

#[test]
fn image_headers_match_legacy() {
    for case in golden()["dimensions"].as_array().unwrap() {
        let input = unhex(case["input_hex"].as_str().unwrap());
        assert_eq!(
            detect_image_format(&input),
            case["format"].as_str().unwrap()
        );
        assert_eq!(json!(image_dimensions(&input)), case["dimensions"]);
    }
}

#[test]
fn cache_index_matches_legacy_scan_and_skips_thumbnails() {
    let (temp, _, index) = fixture();
    let images: Vec<_> = index.images().iter().map(|entry| {
        let restored = decrypt_dat(&fs::read(&entry.path).unwrap(), &keys(), index.limits.max_image_bytes);
        let decoded = restored.ok().map(|data| data.iter().map(|b| format!("{b:02x}")).collect::<String>());
        json!({"path": relative(&temp, &entry.path), "mtime": entry.mtime, "estimated_size": entry.estimated_size,
            "format": entry.format, "width": entry.width, "height": entry.height, "decoded_hex": decoded})
    }).collect();
    assert_eq!(json!(images), golden()["expected_images"]);
    let videos: BTreeMap<_, _> = index
        .videos()
        .iter()
        .map(|(key, entry)| (key.clone(), relative(&temp, &entry.path)))
        .collect();
    assert_eq!(json!(videos), golden()["expected_videos"]);
    assert!(!index
        .images()
        .iter()
        .any(|entry| entry.path.to_string_lossy().contains("_t")));
    assert!(!index.warnings.is_empty());
}

#[test]
fn image_matching_matches_legacy_window_size_and_uniqueness() {
    let (temp, _, index) = fixture();
    for case in golden()["matches"].as_array().unwrap() {
        let requests: Vec<_> = case["media"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| CacheMedia::from_json(m).unwrap())
            .collect();
        let actual: Vec<_> =
            match_cache_images(&index, case["create_time"].as_i64().unwrap(), &requests)
                .iter()
                .map(|i| i.map(|i| relative(&temp, &index.images()[i].path)))
                .collect();
        assert_eq!(json!(actual), case["expected_paths"], "{}", case["name"]);
    }
}

#[test]
fn video_key_lookup_matches_legacy_signed_and_unsigned_ids() {
    let (temp, _, index) = fixture();
    for case in golden()["video_lookups"].as_array().unwrap() {
        let entry = find_cached_video(
            &index,
            &scalar(&case["post"]["post_id"]),
            case["post"]["tid"].as_i64(),
            &scalar(&case["media"]["id"]),
        );
        assert_eq!(
            json!(entry.map(|e| relative(&temp, &e.path))),
            case["expected_path"]
        );
    }
    assert_eq!(
        video_cache_key(" 123 ", " 456 "),
        format!("{:x}", md5::compute(b"123_456_3"))
    );
}

#[test]
fn video_copy_bytes_and_reference_fields_match_legacy() {
    let (temp, _, index) = fixture();
    for case in golden()["video_copies"].as_array().unwrap() {
        // 旧 copy_cached_video 独立函数没有媒体编号；给旧文件名补上与新 API 相同的 _0。
        let stem = case["stem"].as_str().unwrap();
        let mut post = json!({"id": case["post_id"], "create_time": 1700000000, "media": [{"type": 6, "id": case["media_id"]}]});
        let report = recover_post_media(
            &index,
            &post,
            &temp.0.join("out"),
            stem,
            &keys(),
            RecoveryOptions {
                allow_partial_video: case["allow_partial"].as_bool().unwrap(),
            },
        )
        .unwrap();
        let item = &report.media[0];
        if case["legacy_status"].is_null() {
            assert_ne!(item.status, "recovered");
            assert!(item.reference.is_none());
        } else {
            assert_eq!(item.status, "recovered");
            let mut expected = case["expected_reference"].clone();
            expected["local_file"] = json!(format!("videos/{stem}_0.mp4"));
            assert_eq!(item.reference.as_ref().unwrap(), &expected);
            let file = temp
                .0
                .join("out")
                .join(expected["local_file"].as_str().unwrap());
            assert_eq!(
                fs::read(&file).unwrap(),
                unhex(case["expected_hex"].as_str().unwrap())
            );
            let source = temp.0.join(case["source"].as_str().unwrap());
            assert_eq!(
                fs::metadata(file).unwrap().modified().unwrap(),
                fs::metadata(source).unwrap().modified().unwrap()
            );
            apply_media_references(&mut post, &report).unwrap();
            assert_eq!(
                post["media"][0]["video_complete"],
                expected["video_complete"]
            );
        }
    }
}

#[test]
fn recovered_images_are_attached_without_changing_remote_fields() {
    let (temp, _, index) = fixture();
    let expected = golden()["expected_images"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "xwechat/2023-11/Sns/Img/aa/v1")
        .unwrap()
        .clone();
    let mut post = json!({"id": "synthetic", "tid": -6, "create_time": 1700000000,
        "media": [{"type": "2", "width": "8", "height": "6", "total_size": expected["estimated_size"].to_string(),
            "url": "https://example.invalid/synthetic", "url_key": "synthetic-reference-only", "url_token": "synthetic-token"}]});
    let original = post.clone();
    let report = recover_post_media(
        &index,
        &post,
        &temp.0.join("out"),
        "20231115061320000",
        &keys(),
        RecoveryOptions::default(),
    )
    .unwrap();
    assert_eq!(report.media[0].status, "recovered");
    assert_eq!(
        report.media[0].match_method.as_deref(),
        Some("legacy_image_heuristic")
    );
    let serialized = serde_json::to_string(&report).unwrap();
    assert!(!serialized.contains("synthetic-token") && !serialized.contains("original_post"));
    apply_media_references(&mut post, &report).unwrap();
    for field in ["url", "url_key", "url_token", "width", "height"] {
        assert_eq!(post["media"][0][field], original["media"][0][field]);
    }
    let restored = temp
        .0
        .join("out")
        .join(post["media"][0]["local_file"].as_str().unwrap());
    assert_eq!(
        fs::read(restored).unwrap(),
        unhex(expected["decoded_hex"].as_str().unwrap())
    );
    assert_eq!(post["media"][0]["image_source"], "cache");
    assert!(apply_media_references(&mut post, &report).is_err());
}

#[test]
fn invalid_changed_or_unavailable_media_do_not_stop_other_items() {
    let (temp, _, index) = fixture();
    let post = json!({"create_time": 1700000000, "media": [
        {"type": "2", "width": "bad"},
        {"type": "2", "width": 33, "height": 44},
        {"type": "6"},
        {"type": "28"},
        {"type": "2", "local_file": "already.jpg"},
        {"type": "2", "width": 8, "height": 6},
    ]});
    let report = recover_post_media(
        &index,
        &post,
        &temp.0.join("out"),
        "mixed",
        &keys(),
        RecoveryOptions::default(),
    )
    .unwrap();
    let states: Vec<_> = report.media.iter().map(|m| m.status.as_str()).collect();
    assert_eq!(
        states,
        [
            "invalid_metadata",
            "failed",
            "missing_media_id",
            "unsupported_media_type",
            "existing_reference",
            "recovered"
        ]
    );
    assert!(report.warnings[0].contains("padding"));
    let mut wrong = post.clone();
    wrong["create_time"] = json!(99);
    assert!(apply_media_references(&mut wrong, &report).is_err());
}

#[test]
fn changed_sources_and_existing_outputs_are_not_overwritten() {
    let (temp, _, index) = fixture();
    let post =
        json!({"create_time": 1700000000, "media": [{"type": "2", "width": 8, "height": 6}]});
    let out = temp.0.join("out");
    let report = recover_post_media(
        &index,
        &post,
        &out,
        "same",
        &keys(),
        RecoveryOptions::default(),
    )
    .unwrap();
    let local = out.join(
        report.media[0].reference.as_ref().unwrap()["local_file"]
            .as_str()
            .unwrap(),
    );
    let before = fs::read(&local).unwrap();
    let repeated = recover_post_media(
        &index,
        &post,
        &out,
        "same",
        &keys(),
        RecoveryOptions::default(),
    )
    .unwrap();
    assert_eq!(repeated.media[0].status, "failed");
    assert_eq!(fs::read(&local).unwrap(), before);
    let request = CacheMedia::from_json(&post["media"][0]).unwrap();
    let selected = match_cache_images(&index, 1700000000, &[request])[0].unwrap();
    fs::write(&index.images()[selected].path, b"changed synthetic cache").unwrap();
    let changed = recover_post_media(
        &index,
        &post,
        &out,
        "changed",
        &keys(),
        RecoveryOptions::default(),
    )
    .unwrap();
    assert_eq!(changed.media[0].status, "failed");
    assert!(changed.warnings[0].contains("changed since indexing"));
}

#[test]
fn account_roots_limits_and_output_path_guards() {
    let (temp, roots, index) = fixture();
    assert!(build_cache_index(
        &CacheRoots {
            xwechat: Some(temp.0.join("missing")),
            ..Default::default()
        },
        &keys(),
        CacheLimits::default()
    )
    .is_err());
    assert!(build_cache_index(
        &roots,
        &keys(),
        CacheLimits {
            max_entries: 1,
            ..Default::default()
        }
    )
    .is_err());
    let bounded = build_cache_index(
        &roots,
        &keys(),
        CacheLimits {
            max_image_bytes: 1,
            max_video_bytes: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(bounded.images().is_empty() && bounded.videos().is_empty());
    assert!(decrypt_dat(&[0; 32], &keys(), 16).is_err());
    let empty = build_cache_index(&CacheRoots::default(), &keys(), CacheLimits::default()).unwrap();
    assert!(empty.images().is_empty() && empty.videos().is_empty());
    let post =
        json!({"create_time": 1700000000, "media": [{"type": "2", "width": 8, "height": 6}]});
    for stem in ["../escape", "a/b", "C:\\escape", "", "x.jpg"] {
        assert!(recover_post_media(
            &index,
            &post,
            &temp.0.join("out"),
            stem,
            &keys(),
            RecoveryOptions::default()
        )
        .is_err());
    }
    let inside = roots.xwechat.as_ref().unwrap().join("must-not-be-created");
    let rejected = recover_post_media(
        &index,
        &post,
        &inside,
        "safe",
        &keys(),
        RecoveryOptions::default(),
    )
    .unwrap();
    assert_eq!(rejected.media[0].status, "failed");
    assert!(!inside.exists());
    #[cfg(windows)]
    assert!(reject_network_path(Path::new(r"\\example.invalid\share")).is_err());
}

#[test]
fn missing_v2_key_keeps_v1_and_xor_available() {
    let (temp, roots, _) = fixture();
    let index = build_cache_index(&roots, &CacheKeys::default(), CacheLimits::default()).unwrap();
    assert!(index
        .images()
        .iter()
        .any(|e| relative(&temp, &e.path).ends_with("/v1")));
    assert!(index
        .images()
        .iter()
        .any(|e| relative(&temp, &e.path).ends_with("/xor")));
    assert!(!index
        .images()
        .iter()
        .any(|e| relative(&temp, &e.path).ends_with("/v2")));
    assert!(index
        .warnings
        .iter()
        .any(|s| s.contains("V2 image key not supplied")));
}

#[test]
fn video_only_does_not_scan_images_or_require_legacy_root() {
    let temp = Temp::new();
    let root = temp.0.join("xwechat");
    let images = root.join("2026-09/Sns/Img/aa");
    fs::create_dir_all(&images).unwrap();
    for i in 0..32 {
        fs::write(images.join(format!("broken-{i}")), b"bad DAT").unwrap();
    }
    fs::write(images.join("broken-v2"), V2).unwrap();
    let key = video_cache_key("post", "media");
    let video = root
        .join("2026-09/Sns/Video")
        .join(&key[..2])
        .join(format!("{}.mp4", &key[2..]));
    fs::create_dir_all(video.parent().unwrap()).unwrap();
    fs::write(&video, b"synthetic video").unwrap();
    let limits = CacheLimits {
        max_entries: 1,
        max_image_bytes: 0,
        ..Default::default()
    };
    let index = build_video_cache_index(&root, limits).unwrap();
    assert_eq!(index.scanned, 1);
    assert!(index.images().is_empty());
    assert!(index.warnings.is_empty());
    assert_eq!(index.videos().len(), 1);
    assert_eq!(
        find_cached_video(&index, "post", None, "media")
            .unwrap()
            .path,
        fs::canonicalize(video).unwrap()
    );
    let roots = CacheRoots {
        xwechat: Some(root),
        file_storage_sns: Some(temp.0.join("missing-legacy")),
    };
    // 无密钥模式连旧图片根的规范化也跳过；完整模式保留缺失根错误。
    let index = build_index(&roots, None, limits).unwrap();
    assert_eq!(index.scanned, 1);
    assert_eq!(index.roots.len(), 1);
    assert!(build_cache_index(&roots, &CacheKeys::default(), limits).is_err());
    let roots = CacheRoots {
        file_storage_sns: None,
        ..roots
    };
    assert!(build_cache_index(&roots, &CacheKeys::default(), limits)
        .unwrap_err()
        .to_string()
        .contains("entry limit"));
}

#[test]
fn video_only_matches_original_candidates_and_lookup() {
    let (temp, roots, original) = fixture();
    let index =
        build_video_cache_index(roots.xwechat.as_deref().unwrap(), CacheLimits::default()).unwrap();
    assert!(index.images().is_empty());
    assert_eq!(index.videos().len(), original.videos().len());
    assert!(index.videos().values().any(|e| e.complete));
    assert!(index.videos().values().any(|e| !e.complete));
    for (key, expected) in original.videos() {
        let actual = &index.videos()[key];
        assert_eq!(actual.path, expected.path);
        assert_eq!(actual.complete, expected.complete);
        assert_eq!(actual.source_size, expected.source_size);
        assert_eq!(actual.modified, expected.modified);
    }
    for case in golden()["video_lookups"].as_array().unwrap() {
        let entry = find_cached_video(
            &index,
            &scalar(&case["post"]["post_id"]),
            case["post"]["tid"].as_i64(),
            &scalar(&case["media"]["id"]),
        );
        assert_eq!(
            json!(entry.map(|e| relative(&temp, &e.path))),
            case["expected_path"]
        );
    }
}

#[test]
fn video_only_preserves_limits_and_root_guards() {
    let (temp, roots, _) = fixture();
    let root = roots.xwechat.as_deref().unwrap();
    assert!(build_video_cache_index(&temp.0.join("missing"), CacheLimits::default()).is_err());
    let file = temp.0.join("not-directory");
    fs::write(&file, b"synthetic").unwrap();
    assert!(build_video_cache_index(&file, CacheLimits::default()).is_err());
    assert!(build_video_cache_index(
        root,
        CacheLimits {
            max_entries: 0,
            ..Default::default()
        }
    )
    .unwrap_err()
    .to_string()
    .contains("entry limit"));
    let bounded = build_video_cache_index(
        root,
        CacheLimits {
            max_video_bytes: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(bounded.videos().is_empty());
    assert!(bounded
        .warnings
        .iter()
        .any(|w| w.contains("video exceeds size limit")));
    let index = build_video_cache_index(&root.join("."), CacheLimits::default()).unwrap();
    assert_eq!(index.roots, vec![fs::canonicalize(root).unwrap()]);
    assert!(checked_source(&file, &index.roots).is_err());
    let inside = root.join("must-not-be-created");
    assert!(output_directory(&inside, "videos", &index).is_err());
    assert!(!inside.exists());
    let empty = temp.0.join("empty-cache");
    fs::create_dir(&empty).unwrap();
    let empty = build_video_cache_index(&empty, CacheLimits::default()).unwrap();
    assert_eq!(empty.scanned, 0);
    assert!(empty.images().is_empty() && empty.videos().is_empty());
    #[cfg(windows)]
    for path in [
        r"\\example.invalid\share",
        r"\\?\UNC\example.invalid\share",
        r"\\.\pipe\synthetic",
    ] {
        assert!(
            build_video_cache_index(Path::new(path), CacheLimits::default())
                .unwrap_err()
                .to_string()
                .contains("only local filesystem")
        );
    }
}

#[test]
fn unsafe_media_directory_and_reference_application_are_atomic() {
    let (temp, _, index) = fixture();
    let mut post =
        json!({"create_time": 1700000000, "media": [{"type": "2", "width": 8, "height": 6}]});
    let out = temp.0.join("out");
    fs::create_dir(&out).unwrap();
    fs::write(out.join("images"), b"do not replace").unwrap();
    let report = recover_post_media(
        &index,
        &post,
        &out,
        "safe",
        &keys(),
        RecoveryOptions::default(),
    )
    .unwrap();
    assert_eq!(report.media[0].status, "failed");
    assert_eq!(fs::read(out.join("images")).unwrap(), b"do not replace");
    let mut report = recover_post_media(
        &index,
        &post,
        &temp.0.join("valid"),
        "safe",
        &keys(),
        RecoveryOptions::default(),
    )
    .unwrap();
    let original = post.clone();
    let mut invalid = report.media[0].clone();
    invalid.media_index = 100;
    report.media.push(invalid);
    assert!(apply_media_references(&mut post, &report).is_err());
    assert_eq!(post, original);
}

#[test]
#[cfg_attr(
    windows,
    ignore = "Requires Windows symlink privilege; run explicitly with --ignored when available"
)]
fn reparse_cache_and_output_directories_do_not_escape_roots() {
    let (temp, roots, index) = fixture();
    let outside = Temp::new();
    let link = roots.xwechat.as_ref().unwrap().join("linked-month");
    #[cfg(windows)]
    let created = std::os::windows::fs::symlink_dir(&outside.0, &link);
    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(&outside.0, &link);
    created.expect("filesystem fixture requires symlink creation permission");
    let rebuilt = build_cache_index(&roots, &keys(), CacheLimits::default()).unwrap();
    assert_eq!(rebuilt.images().len(), index.images().len());
    assert!(rebuilt.warnings.iter().any(|s| s.contains("reparse")));
    let video_only =
        build_video_cache_index(roots.xwechat.as_deref().unwrap(), CacheLimits::default()).unwrap();
    assert_eq!(video_only.videos().len(), index.videos().len());
    assert!(video_only.warnings.iter().any(|s| s.contains("reparse")));
    let out = temp.0.join("out-link");
    fs::create_dir(&out).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&outside.0, out.join("images")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside.0, out.join("images")).unwrap();
    let post =
        json!({"create_time": 1700000000, "media": [{"type": "2", "width": 8, "height": 6}]});
    let report = recover_post_media(
        &index,
        &post,
        &out,
        "safe",
        &keys(),
        RecoveryOptions::default(),
    )
    .unwrap();
    assert_eq!(report.media[0].status, "failed");
    assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 0);
}
