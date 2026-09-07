use super::*;
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("../../tests/fixtures/chat-plan-golden.json")).unwrap()
}

fn synthetic() -> (tempfile::TempDir, Value) {
    let temp = tempfile::tempdir().unwrap();
    let fixture = fixture();
    for (name, sql) in fixture["sql"].as_object().unwrap() {
        let conn = Connection::open(temp.path().join(name)).unwrap();
        conn.execute_batch(sql.as_str().unwrap()).unwrap();
    }
    (temp, fixture)
}

fn chats(fixture: &Value) -> Vec<PlanChat> {
    fixture["users"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, user)| PlanChat {
            index: i + 1,
            username: user.as_str().unwrap().into(),
            chat_name: "synthetic".into(),
            chat_type: "single".into(),
        })
        .collect()
}

fn paths(value: &Value) -> Vec<PathBuf> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| PathBuf::from(v.as_str().unwrap()))
        .collect()
}

#[test]
fn legacy_synthetic_sqlite_differential() {
    let (temp, fixture) = synthetic();
    let chats = chats(&fixture);
    let before: Vec<_> = fixture["sql"]
        .as_object()
        .unwrap()
        .keys()
        .map(|name| (name.clone(), fs::read(temp.path().join(name)).unwrap()))
        .collect();
    for scenario in fixture["scenarios"].as_array().unwrap() {
        let dbs = PlanDatabases {
            message: paths(&scenario["message"]),
            resource: scenario["resource"].as_str().map(PathBuf::from),
            media: paths(&scenario["media"]),
        };
        let rows = collect_plan(
            temp.path(),
            &dbs,
            &chats,
            TimeRange {
                start: scenario["start"].as_i64(),
                end: scenario["end"].as_i64(),
            },
            SizeMode::Estimate,
        )
        .unwrap();
        assert_eq!(rows.len(), chats.len());
        for row in rows {
            let actual = serde_json::to_value(&row).unwrap();
            for (key, expected) in scenario["expected"][&row.username].as_object().unwrap() {
                assert_eq!(
                    &actual[key], expected,
                    "scenario={} username={} field={key}",
                    scenario["name"], row.username
                );
            }
            assert_eq!(row.first_time, format_time(row.first_ts).unwrap());
            assert_eq!(row.last_time, format_time(row.last_ts).unwrap());
        }
    }
    for (name, bytes) in before {
        assert_eq!(fs::read(temp.path().join(name)).unwrap(), bytes);
    }
    assert_eq!(
        fs::read_dir(temp.path()).unwrap().count(),
        fixture["sql"].as_object().unwrap().len()
    );
}

#[test]
fn query_null_zero_and_inclusive_endpoints_match_python() {
    let (temp, fixture) = synthetic();
    let conn = open_readonly(&temp.path().join("messages.db")).unwrap();
    let table = format!("Msg_{:x}", md5::compute("alpha"));
    for case in fixture["direct"].as_array().unwrap() {
        let stats = query_message_table_plan_stats(
            &conn,
            &table,
            TimeRange {
                start: case["start"].as_i64(),
                end: case["end"].as_i64(),
            },
        )
        .unwrap();
        assert_eq!(
            serde_json::json!([
                stats.message_count,
                stats.first_ts,
                stats.last_ts,
                stats.message_body_bytes
            ]),
            case["values"]
        );
    }
    assert!(conn.execute("DELETE FROM sqlite_master", []).is_err());
    for name in [
        "sqlite_master",
        "Msg_x",
        "Msg_0000000000000000000000000000000]",
        "Msg_00000000000000000000000000000000;DROP TABLE x",
    ] {
        assert!(query_message_table_plan_stats(&conn, name, TimeRange::default()).is_err());
    }
}

#[test]
fn csv_bytes_match_python_bom_crlf_and_quoting() {
    let (temp, fixture) = synthetic();
    let chat = PlanChat {
        index: 1,
        username: "alpha".into(),
        chat_name: "中文, \"quoted\"\r\nline".into(),
        chat_type: "single".into(),
    };
    let mut rows = collect_plan(
        temp.path(),
        &PlanDatabases::default(),
        &[chat],
        TimeRange::default(),
        SizeMode::Estimate,
    )
    .unwrap();
    rows[0].size_status = "ok".into();
    assert_eq!(
        render_plan_csv(&rows).unwrap(),
        fixture["csv"].as_str().unwrap().as_bytes()
    );
    let empty = render_plan_csv(&[]).unwrap();
    assert_eq!(&empty[..3], &[0xef, 0xbb, 0xbf]);
    assert_eq!(
        &empty[3..],
        format!("{}\r\n", PLAN_CSV_FIELDS.join(",")).as_bytes()
    );
}

#[test]
fn invalid_requests_and_missing_scan_source_are_explicit() {
    let (temp, fixture) = synthetic();
    let chats = chats(&fixture);
    let rows = collect_plan(
        temp.path(),
        &PlanDatabases::default(),
        &chats,
        TimeRange::default(),
        SizeMode::Scan,
    )
    .unwrap();
    assert!(rows
        .iter()
        .all(|row| row.attachment_scanned_bytes == Some(0)
            && row.size_status.contains("scan_base_missing")));
    assert!(collect_plan(
        temp.path(),
        &PlanDatabases::default(),
        &chats,
        TimeRange {
            start: Some(2),
            end: Some(1)
        },
        SizeMode::Estimate
    )
    .is_err());
    for path in ["../outside.db", "C:\\outside.db", "messages.db:stream", ""] {
        let dbs = PlanDatabases {
            message: vec![path.into()],
            ..Default::default()
        };
        assert!(
            collect_plan(
                temp.path(),
                &dbs,
                &chats,
                TimeRange::default(),
                SizeMode::Estimate
            )
            .is_err(),
            "{path}"
        );
    }
    let dbs = PlanDatabases {
        message: vec!["messages.db".into(), "messages.db".into()],
        ..Default::default()
    };
    assert!(collect_plan(
        temp.path(),
        &dbs,
        &chats,
        TimeRange::default(),
        SizeMode::Estimate
    )
    .is_err());
    assert!(collect_plan(
        temp.path(),
        &PlanDatabases::default(),
        &[chats[0].clone(), chats[0].clone()],
        TimeRange::default(),
        SizeMode::Estimate
    )
    .is_err());
    assert!(collect_plan(
        Path::new("."),
        &PlanDatabases::default(),
        &chats,
        TimeRange::default(),
        SizeMode::Estimate
    )
    .is_err());
}

fn scan_fixture(temp: &Path, fixture: &Value) -> PathBuf {
    let source = temp.join("synthetic-source");
    for (name, size) in fixture["scan"]["files"].as_object().unwrap() {
        let path = source.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b's'; size.as_u64().unwrap() as usize]).unwrap();
    }
    for (name, target) in fixture["scan"]["hardlinks"].as_object().unwrap() {
        fs::hard_link(source.join(target.as_str().unwrap()), source.join(name)).unwrap();
    }
    source
}

#[test]
fn scan_matches_legacy_hardlinks_and_parallel_order() {
    let (temp, fixture) = synthetic();
    let source = scan_fixture(temp.path(), &fixture);
    let chats = chats(&fixture);
    let mut previous = None;
    for workers in [1, 2, 6] {
        let rows = collect_plan_with_scan(
            temp.path(),
            &PlanDatabases::default(),
            &chats,
            TimeRange {
                start: Some(9999),
                end: Some(9999),
            },
            &ScanOptions {
                source_dir: Some(source.clone()),
                workers,
                ..Default::default()
            },
        )
        .unwrap();
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row.username, chats[i].username);
            assert_eq!(
                row.attachment_scanned_bytes,
                fixture["scan"]["expected"][&row.username][0].as_i64()
            );
            assert_eq!(
                row.size_status.contains("scan_limited"),
                fixture["scan"]["expected"][&row.username][1] == "scan_limited"
            );
            assert_eq!(row.total_estimated_bytes, 0);
        }
        let serialized = serde_json::to_value(&rows).unwrap();
        if let Some(previous) = previous {
            assert_eq!(serialized, previous);
        }
        previous = Some(serialized);
    }
    let rows = collect_plan_with_scan(
        temp.path(),
        &PlanDatabases::default(),
        &chats,
        TimeRange::default(),
        &ScanOptions {
            media_dir: Some(source.join("msg")),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rows[0].attachment_scanned_bytes, Some(57));
    for (name, size) in fixture["scan"]["files"].as_object().unwrap() {
        assert_eq!(
            fs::read(source.join(name)).unwrap(),
            vec![b's'; size.as_u64().unwrap() as usize]
        );
    }
    let csv = render_plan_csv(&rows).unwrap();
    let mut reader = csv::Reader::from_reader(&csv[3..]);
    assert_eq!(&reader.records().next().unwrap().unwrap()[9], "57");
}

#[test]
fn scan_rejects_invalid_options_and_paths() {
    let (temp, fixture) = synthetic();
    let chats = chats(&fixture);
    let run = |options| {
        collect_plan_with_scan(
            temp.path(),
            &PlanDatabases::default(),
            &chats,
            TimeRange::default(),
            &options,
        )
    };
    for workers in [0, 7, usize::MAX] {
        assert!(run(ScanOptions {
            workers,
            ..Default::default()
        })
        .is_err());
    }
    for path in [
        PathBuf::from("relative"),
        temp.path().join("../outside"),
        temp.path().join("x:stream"),
        temp.path().join("bad\0name"),
        temp.path().join("x".repeat(33000)),
    ] {
        assert!(run(ScanOptions {
            media_dir: Some(path),
            ..Default::default()
        })
        .is_err());
    }
    assert!(run(ScanOptions {
        source_dir: Some(temp.path().into()),
        media_dir: Some(temp.path().into()),
        ..Default::default()
    })
    .is_err());
    let missing = temp.path().join("missing");
    let rows = run(ScanOptions {
        media_dir: Some(missing.clone()),
        ..Default::default()
    })
    .unwrap();
    assert!(rows
        .iter()
        .all(|r| r.size_status.contains("scan_base_missing")));
    assert!(!missing.exists());
}

#[test]
fn scan_large_logical_file_and_deleted_file() {
    let (temp, fixture) = synthetic();
    let media = temp.path().join("media");
    let root = media
        .join("attach")
        .join(format!("{:x}", md5::compute("alpha")));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("large.bin");
    let file = fs::File::create(&path).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[link(name = "kernel32")]
        extern "system" {
            fn DeviceIoControl(
                handle: *mut std::ffi::c_void,
                code: u32,
                input: *const u8,
                input_len: u32,
                output: *mut u8,
                output_len: u32,
                returned: *mut u32,
                overlapped: *mut std::ffi::c_void,
            ) -> i32;
        }
        let mut returned = 0;
        // 设置 NTFS 稀疏属性，避免测试占用实际 4 GiB 磁盘空间。
        let ok = unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                0x000900c4,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
    }
    // 仅测试逻辑长度，不读取或分配同等大小的文件内容。
    let large = (1u64 << 32) + 17;
    file.set_len(large).unwrap();
    drop(file);
    let rows = collect_plan_with_scan(
        temp.path(),
        &PlanDatabases::default(),
        &chats(&fixture),
        TimeRange::default(),
        &ScanOptions {
            media_dir: Some(media),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rows[0].attachment_scanned_bytes, Some(large as i64));
    fs::remove_file(&path).unwrap();
    assert!(matches!(scan_pin(&path), Err("scan_missing")));
    assert_eq!(
        scan_username(root.parent().unwrap().parent().unwrap(), "alpha").bytes,
        0
    );
}

#[cfg(windows)]
fn junction(link: &Path, target: &Path) {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    #[link(name = "kernel32")]
    extern "system" {
        fn DeviceIoControl(
            handle: *mut std::ffi::c_void,
            code: u32,
            input: *const u8,
            input_len: u32,
            output: *mut u8,
            output_len: u32,
            returned: *mut u32,
            overlapped: *mut std::ffi::c_void,
        ) -> i32;
    }
    fs::create_dir(link).unwrap();
    let print = target.to_str().unwrap();
    let substitute = format!("\\??\\{}", print.trim_start_matches("\\\\?\\"));
    let sub: Vec<u16> = substitute.encode_utf16().collect();
    let display: Vec<u16> = print.encode_utf16().collect();
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&0xa0000003u32.to_le_bytes());
    for value in [
        (8 + (sub.len() + display.len() + 2) * 2) as u16,
        0,
        0,
        (sub.len() * 2) as u16,
        ((sub.len() + 1) * 2) as u16,
        (display.len() * 2) as u16,
    ] {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    for value in sub.into_iter().chain([0]).chain(display).chain([0]) {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    let handle = fs::OpenOptions::new()
        .access_mode(0x40000000)
        .share_mode(7)
        .custom_flags(0x02200000)
        .open(link)
        .unwrap();
    let mut returned = 0;
    // 合成目录上设置 junction，不调用 shell、不需要符号链接特权。
    let result = unsafe {
        DeviceIoControl(
            handle.as_raw_handle(),
            0x000900a4,
            buffer.as_ptr(),
            buffer.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(result, 0, "{}", std::io::Error::last_os_error());
}

#[cfg(windows)]
#[test]
fn scan_blocks_junctions_and_pins_ancestors() {
    let (temp, fixture) = synthetic();
    let source = scan_fixture(temp.path(), &fixture);
    let outside = tempfile::tempdir().unwrap();
    fs::write(
        outside.path().join("other-account.bin"),
        b"synthetic other account",
    )
    .unwrap();
    let link = source
        .join("msg/attach")
        .join(format!("{:x}", md5::compute("alpha")))
        .join("junction");
    junction(&link, outside.path());
    let rows = collect_plan_with_scan(
        temp.path(),
        &PlanDatabases::default(),
        &chats(&fixture),
        TimeRange::default(),
        &ScanOptions {
            source_dir: Some(source.clone()),
            workers: 6,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rows[0].attachment_scanned_bytes, Some(57));
    assert!(rows[0].size_status.contains("scan_reparse_skipped"));
    let rows = collect_plan_with_scan(
        temp.path(),
        &PlanDatabases::default(),
        &chats(&fixture),
        TimeRange::default(),
        &ScanOptions {
            media_dir: Some(link.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(rows
        .iter()
        .all(|r| r.attachment_scanned_bytes == Some(0)
            && r.size_status.contains("scan_reparse_skipped")));
    let (pins, status) = pin_scan_root(&source.join("msg")).unwrap();
    assert!(status.is_none());
    assert!(fs::rename(&source, temp.path().join("moved")).is_err());
    drop(pins);
    fs::remove_dir(link).unwrap();
    assert_eq!(
        fs::read(outside.path().join("other-account.bin")).unwrap(),
        b"synthetic other account"
    );
}

#[cfg(windows)]
#[test]
fn scan_counts_busy_and_readonly_files_without_reading_contents() {
    use std::os::windows::fs::OpenOptionsExt;
    let (temp, fixture) = synthetic();
    let source = scan_fixture(temp.path(), &fixture);
    let media = source.join("msg");
    let root = media
        .join("attach")
        .join(format!("{:x}", md5::compute("alpha")));
    let locked = fs::OpenOptions::new()
        .access_mode(0x10000)
        .share_mode(7)
        .open(root.join("a.bin"))
        .unwrap();
    let result = scan_username(&media, "alpha");
    // 属性查询不要求读取正文，即便其他句柄正在占用正文也能统计。
    assert_eq!(result.bytes, 57);
    assert!(result.statuses.is_empty());
    drop(locked);
    let path = root.join("nested/copy.bin");
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    let result = scan_username(&media, "alpha");
    assert_eq!(result.bytes, 57);
    assert!(result.statuses.is_empty());
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
}

#[test]
fn scan_wrong_directory_type_preserves_partial_bytes() {
    let (temp, fixture) = synthetic();
    let source = scan_fixture(temp.path(), &fixture);
    let media = source.join("msg");
    // 不删除既有目录；改用独立合成媒体根验证类型错误。
    let bad_media = temp.path().join("bad-media");
    fs::create_dir(&bad_media).unwrap();
    fs::write(bad_media.join("attach"), b"not a directory").unwrap();
    let result = scan_username(&bad_media, "alpha");
    assert_eq!(result.bytes, 0);
    assert!(result.statuses.contains("scan_error"));
    assert_eq!(scan_username(&media, "alpha").bytes, 57);
}

#[test]
fn scan_depth_bound_is_visible_without_stack_overflow() {
    let temp = tempfile::tempdir().unwrap();
    let mut total = ScanTotal::default();
    let pin = scan_pin(temp.path()).unwrap_or_else(|s| panic!("{s}"));
    scan_tree(temp.path(), pin, 128, &mut total);
    assert_eq!(total.bytes, 0);
    assert!(total.statuses.contains("scan_depth_limited"));
}

#[cfg(windows)]
#[test]
fn decrypted_root_rejects_junction_ancestors_before_any_database_query() {
    let (temp, fixture) = synthetic();
    let container = temp.path().join("container");
    let cache = container.join("cache");
    fs::create_dir_all(&cache).unwrap();
    for name in ["messages.db", "resource.db", "media.db"] {
        fs::copy(temp.path().join(name), cache.join(name)).unwrap();
    }
    let databases = PlanDatabases {
        message: vec!["messages.db".into()],
        resource: Some("resource.db".into()),
        media: vec!["media.db".into()],
    };
    let chats = chats(&fixture);
    let range = TimeRange {
        start: Some(100),
        end: Some(102),
    };
    let before = fs::read(cache.join("messages.db")).unwrap();
    assert_eq!(
        collect_plan(&cache, &databases, &chats, range, SizeMode::Estimate).unwrap()[0]
            .message_count,
        3
    );
    let link = temp.path().join("ancestor-junction");
    junction(&link, &container);
    struct RemoveJunction(PathBuf);
    impl Drop for RemoveJunction {
        fn drop(&mut self) {
            let _ = fs::remove_dir(&self.0);
        }
    }
    let _cleanup = RemoveJunction(link.clone());
    // 末级 cache 是普通目录；还要覆盖直接以 junction 为根的情况。
    for alias in [link.join("cache"), link.clone()] {
        for mode in [SizeMode::Estimate, SizeMode::Scan] {
            let error = collect_plan(&alias, &databases, &chats, range, mode).unwrap_err();
            assert!(error.to_string().contains("scan_reparse_skipped"));
        }
        // 即使没有数据库，也不得把被拒绝的目录包装成成功的空计划。
        assert!(collect_plan(
            &alias,
            &PlanDatabases::default(),
            &chats,
            range,
            SizeMode::Estimate
        )
        .is_err());
        let options = ScanOptions {
            media_dir: Some(temp.path().to_path_buf()),
            workers: 6,
            ..Default::default()
        };
        assert!(collect_plan_with_scan(&alias, &databases, &chats, range, &options).is_err());
    }
    assert_eq!(fs::read(cache.join("messages.db")).unwrap(), before);
    // 普通目录的扫描和统计仍可执行，校验句柄不会泄漏到调用结束之后。
    let options = ScanOptions {
        media_dir: Some(cache.clone()),
        ..Default::default()
    };
    assert_eq!(
        collect_plan_with_scan(&cache, &databases, &chats, range, &options).unwrap()[0]
            .attachment_scanned_bytes,
        Some(0)
    );
    fs::rename(&cache, container.join("renamed-cache")).unwrap();
}

#[test]
fn missing_explicit_files_are_not_created_or_silenced() {
    let (temp, fixture) = synthetic();
    let dbs = PlanDatabases {
        message: vec!["missing-message.db".into()],
        resource: Some("missing-resource.db".into()),
        media: vec!["missing-media.db".into()],
    };
    let rows = collect_plan(
        temp.path(),
        &dbs,
        &chats(&fixture),
        TimeRange::default(),
        SizeMode::Estimate,
    )
    .unwrap();
    for row in rows {
        assert_eq!(
            row.size_status,
            "partial:media_error,message_error,no_message_table,resource_error"
        );
        assert_eq!(row.message_count, 0);
    }
    for path in [
        "missing-message.db",
        "missing-resource.db",
        "missing-media.db",
    ] {
        assert!(!temp.path().join(path).exists());
    }
}

#[test]
fn aggregate_overflow_is_not_wrapped() {
    let mut count = i64::MAX;
    assert!(add(&mut count, 1).is_err());
    assert_eq!(count, i64::MAX);
}

#[test]
fn corrupt_database_remains_unchanged_and_reports_errors() {
    let (temp, fixture) = synthetic();
    let bytes = b"synthetic corrupt sqlite, not user data";
    fs::write(temp.path().join("corrupt.db"), bytes).unwrap();
    let dbs = PlanDatabases {
        message: vec!["corrupt.db".into()],
        resource: Some("corrupt.db".into()),
        media: vec!["corrupt.db".into()],
    };
    let rows = collect_plan(
        temp.path(),
        &dbs,
        &chats(&fixture),
        TimeRange::default(),
        SizeMode::Estimate,
    )
    .unwrap();
    for row in rows {
        assert_eq!(
            row.size_status,
            "partial:media_error,message_error,no_message_table,resource_error"
        );
    }
    assert_eq!(fs::read(temp.path().join("corrupt.db")).unwrap(), bytes);
}

#[test]
fn message_shards_accumulate_once_and_ignore_unselected_tables() {
    let (temp, fixture) = synthetic();
    fs::copy(
        temp.path().join("messages.db"),
        temp.path().join("messages2.db"),
    )
    .unwrap();
    let chat = chats(&fixture).remove(0);
    let rows = collect_plan(
        temp.path(),
        &PlanDatabases {
            message: vec!["messages.db".into(), "messages2.db".into()],
            ..Default::default()
        },
        &[chat],
        TimeRange {
            start: Some(100),
            end: Some(102),
        },
        SizeMode::Estimate,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message_count, 6);
    assert_eq!(rows[0].message_body_bytes, 20);
    assert_eq!(rows[0].first_ts, Some(100));
    assert_eq!(rows[0].last_ts, Some(102));
}
