//! Synthetic encrypted voices through real MCP, daemon workers, CLI and HTTP.
use super::artifacts::{read_artifact, refused};
use super::{call, terminal, Fixture, Mcp, Web};
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs, path::PathBuf};

const HOST: &[&str] = &[
    "--tasks",
    "--task-kind",
    "export_voices",
    "--task-allow-media-write",
    "--task-allow-artifact-read",
];

struct Seed {
    account: PathBuf,
    sources: Vec<(PathBuf, Vec<u8>)>,
    audio: Vec<Vec<u8>>,
}

fn seed(fixture: &mut Fixture, name: &str) -> Seed {
    seed_with_journal(fixture, name, false)
}

fn seed_with_journal(fixture: &mut Fixture, name: &str, wal: bool) -> Seed {
    let account = fixture.account(name, true);
    let contact = rusqlite::Connection::open(account.join("fixture.db")).unwrap();
    contact.execute_batch("INSERT INTO contact VALUES('voice-peer','Same synthetic name','',0),('voice-other','Same synthetic name','',0);").unwrap();
    drop(contact);
    let contact_path = account.join("db_storage/contact/contact.db");
    crate::encrypt_fixture(&account.join("fixture.db"), &contact_path);
    let mut paths = vec![contact_path];
    let mut keys = serde_json::Map::new();
    keys.insert("contact/contact.db".into(), json!("11".repeat(32)));
    let mut audio = Vec::new();
    for shard in 0..2 {
        let media_plain = account.join(format!("voice-media-{shard}.db"));
        let message_plain = account.join(format!("voice-message-{shard}.db"));
        fs::copy(account.join("fixture.db"), &media_plain).unwrap();
        fs::copy(account.join("fixture.db"), &message_plain).unwrap();
        let media = rusqlite::Connection::open(&media_plain).unwrap();
        let messages = rusqlite::Connection::open(&message_plain).unwrap();
        if wal {
            media.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
            messages.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        }
        media.execute_batch("CREATE TABLE Name2Id(user_name TEXT); CREATE TABLE VoiceInfo(chat_name_id INTEGER,local_id INTEGER,create_time INTEGER,svr_id INTEGER,voice_data BLOB);").unwrap();
        for (index, username) in ["voice-peer", "voice-other"].into_iter().enumerate() {
            let bytes = format!("\u{2}#!SILK_V3\0synthetic-{shard}-{index}").into_bytes();
            let server = 1000 + shard * 10 + index as i64;
            media
                .execute(
                    "INSERT INTO Name2Id(rowid,user_name) VALUES(?1,?2)",
                    rusqlite::params![index as i64 + 1, username],
                )
                .unwrap();
            media
                .execute(
                    "INSERT INTO VoiceInfo VALUES(?1,700,1700000000,?2,?3)",
                    rusqlite::params![index as i64 + 1, server, bytes],
                )
                .unwrap();
            let table = format!("Msg_{:x}", md5::compute(username));
            messages.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,server_id INTEGER); INSERT INTO [{table}] VALUES(7,34,1700000000,{server});")).unwrap();
            audio.push(bytes);
        }
        drop(media);
        drop(messages);
        for (kind, plain) in [("media", media_plain), ("message", message_plain)] {
            if wal {
                assert_eq!(&fs::read(&plain).unwrap()[18..20], &[2, 2]);
            }
            let logical = format!("message/{kind}_{shard}.db");
            let encrypted = account.join("db_storage").join(&logical);
            fs::create_dir_all(encrypted.parent().unwrap()).unwrap();
            crate::encrypt_fixture(&plain, &encrypted);
            paths.push(encrypted);
            keys.insert(logical, json!("11".repeat(32)));
        }
    }
    fixture.seed_keys(&account, &Value::Object(keys));
    Seed {
        account,
        sources: paths
            .into_iter()
            .map(|path| {
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect(),
        audio,
    }
}

fn request(id: &str, options: Value) -> Value {
    json!({"idempotency_key":id,"kind":"export_voices","options":{"voice_export":options}})
}

fn assert_no_physical_paths(value: &Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                assert!(
                    !["output_dir", "audio_file", "evidence_file"].contains(&key.as_str()),
                    "{key}"
                );
                assert_no_physical_paths(value);
            }
        }
        Value::Array(values) => values.iter().for_each(assert_no_physical_paths),
        Value::String(value) => {
            assert!(
                !value.contains(":\\") && !value.contains(":/"),
                "physical path leaked"
            );
        }
        _ => {}
    }
}

#[test]
fn wal_voice_sources_have_identical_cli_and_task_association_and_selection() {
    let mut fixture = Fixture::new();
    let seeded = seed_with_journal(&mut fixture, "voice-wal", true);
    let mut mcp = Mcp::start(&fixture, &seeded.account, HOST);
    for (number, options, flags, count) in [
        (0, json!({}), vec![], 4),
        (1, json!({"limit":0}), vec!["--limit", "0"], 0),
        (
            2,
            json!({"offset":1,"limit":2}),
            vec!["--offset", "1", "--limit", "2"],
            2,
        ),
    ] {
        let output = fixture.root.join(format!("voice-wal-output-{number}"));
        let mut args = vec!["voices", "--json", "-o", output.to_str().unwrap()];
        args.extend(flags);
        let cli = call(&fixture, &seeded.account, &args);
        assert_eq!(cli["exported"], count, "{cli}");
        assert_eq!(cli["associated"], count, "{cli}");
        assert_eq!(cli["incomplete_items"], 0, "{cli}");
        let mut cli_ids = Vec::new();
        let mut cli_audio = Vec::new();
        for item in cli["manifest"].as_array().unwrap() {
            assert_eq!(item["association"], "exact_message_media_join");
            cli_ids.push(item["message_id"].as_str().unwrap().to_owned());
            cli_audio.push(fs::read(output.join(item["relative_path"].as_str().unwrap())).unwrap());
        }
        args.push("--overwrite");
        let repeated = call(&fixture, &seeded.account, &args);
        assert_eq!(repeated["associated"], count);
        let id = format!("{:02x}", 0xa0 + number).repeat(32);
        mcp.data("submit_task", request(&id, options));
        let task = terminal(&fixture, &seeded.account, &id);
        assert_eq!(task["status"], "succeeded", "{task}");
        assert_eq!(task["result"]["associated"], count, "{task}");
        let page = mcp.data("list_task_artifacts", json!({"id":id,"limit":100}));
        let mut task_ids = Vec::new();
        let mut task_audio = Vec::new();
        for item in page["items"].as_array().unwrap() {
            let bytes = read_artifact(&mut mcp, &id, item);
            if item["media_type"] == "application/json" {
                let evidence: Value = serde_json::from_slice(&bytes).unwrap();
                if let Some(message) = evidence["message_id"].as_str() {
                    assert_eq!(evidence["association"], "exact_message_media_join");
                    task_ids.push(message.to_owned());
                }
            } else {
                task_audio.push(bytes);
            }
        }
        cli_ids.sort();
        task_ids.sort();
        cli_audio.sort();
        task_audio.sort();
        assert_eq!(task_ids, cli_ids);
        assert_eq!(task_audio, cli_audio);
    }
    for (path, before) in seeded.sources {
        assert_eq!(fs::read(path).unwrap(), before);
    }
}

#[test]
fn raw_voice_tasks_share_original_bytes_across_entries_and_restart() {
    let mut fixture = Fixture::new();
    let seeded = seed(&mut fixture, "voice-task-owner");
    let other = fixture.account("voice-task-other", true);
    let mut mcp = Mcp::start(&fixture, &seeded.account, HOST);
    let id = "81".repeat(32);
    let submission = request(&id, json!({}));
    assert_eq!(mcp.data("submit_task", submission.clone())["id"], id);
    mcp.close();
    let task = terminal(&fixture, &seeded.account, &id);
    assert_eq!(task["status"], "succeeded", "{task}");
    assert_eq!(task["result"]["scope"], "raw_voices");
    assert_eq!(task["result"]["selected_rows"], 4);
    assert_eq!(task["result"]["exported"], 4);
    assert_eq!(task["result"]["associated"], 4);
    assert_eq!(task["result"]["unproven"], 0);
    let mut mcp = Mcp::start(&fixture, &seeded.account, HOST);
    let page = mcp.data("list_task_artifacts", json!({"id":id,"limit":100}));
    assert_eq!(page["complete"], true, "{page}");
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 9, "{page}");
    let ids: BTreeSet<_> = items
        .iter()
        .map(|item| item["artifact_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), items.len());
    assert_eq!(
        call(
            &fixture,
            &seeded.account,
            &["tasks", "artifacts", &id, "--limit", "100"]
        ),
        page
    );
    let web = Web::start(&fixture, &seeded.account);
    let mut actual_audio = Vec::new();
    for item in items {
        let bytes = read_artifact(&mut mcp, &id, item);
        let response = web.request(
            reqwest::Method::GET,
            &format!(
                "/api/tasks/{id}/artifacts/{}/download",
                item["artifact_id"].as_str().unwrap()
            ),
            None,
            None,
        );
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(response.bytes().unwrap().as_ref(), bytes.as_slice());
        if item["media_type"] == "application/json" {
            assert_no_physical_paths(&serde_json::from_slice::<Value>(&bytes).unwrap());
        } else {
            actual_audio.push(bytes);
        }
    }
    actual_audio.sort();
    let mut expected = seeded.audio.clone();
    expected.sort();
    assert_eq!(actual_audio, expected);
    drop(web);
    mcp.close();
    crate::success(fixture.run(&seeded.account, &["daemon", "stop"]));
    let mut mcp = Mcp::start(&fixture, &seeded.account, HOST);
    assert_eq!(mcp.data("submit_task", submission.clone()), task);
    assert_eq!(
        mcp.data("list_task_artifacts", json!({"id":id,"limit":100})),
        page
    );
    assert!(!fixture.run(&other, &["tasks", "get", &id]).status.success());
    let mut foreign = Mcp::start(&fixture, &other, HOST);
    refused(&foreign.tool("get_task", json!({"id":id})));
    refused(&foreign.tool("list_task_artifacts", json!({"id":id})));
    refused(&foreign.tool(
        "read_task_artifact",
        json!({"id":id,"artifact_id":items[0]["artifact_id"]}),
    ));
    let config = seeded.account.join("config.json");
    let pinned = fs::read(&config).unwrap();
    let tasks_before_switch = call(&fixture, &seeded.account, &["tasks", "list"]);
    fs::write(&config, fs::read(other.join("config.json")).unwrap()).unwrap();
    let switched = mcp.tool("submit_task", submission);
    fs::write(&config, pinned).unwrap();
    refused(&switched);
    assert_eq!(
        switched["result"]["structuredContent"]["error"]["code"], "configuration_changed",
        "{switched}"
    );
    assert_eq!(
        call(&fixture, &seeded.account, &["tasks", "list"]),
        tasks_before_switch
    );
    for (path, before) in seeded.sources {
        assert_eq!(fs::read(path).unwrap(), before);
    }
}

#[test]
fn raw_voice_tasks_validate_host_authority_and_preserve_zero_and_global_pagination() {
    let mut fixture = Fixture::new();
    let seeded = seed(&mut fixture, "voice-task-selection");
    let mut denied = Mcp::start(
        &fixture,
        &seeded.account,
        &["--tasks", "--task-kind", "export_voices"],
    );
    refused(&denied.tool("submit_task", request(&"82".repeat(32), json!({"limit":0}))));
    denied.close();
    let mut mcp = Mcp::start(&fixture, &seeded.account, HOST);
    for options in [
        json!({"chat":" "}),
        json!({"overwrite":true}),
        json!({"since":"2024-01-02","until":"2024-01-01"}),
        json!({"output":"C:/model-path"}),
        json!({"decode":true}),
        json!({"limit":-1}),
    ] {
        refused(&mcp.tool("submit_task", request(&"83".repeat(32), options)));
    }
    let empty_id = "84".repeat(32);
    mcp.data("submit_task", request(&empty_id, json!({"limit":0})));
    let empty = terminal(&fixture, &seeded.account, &empty_id);
    assert_eq!(empty["status"], "succeeded", "{empty}");
    assert_eq!(empty["result"]["selected_rows"], 0);
    assert_eq!(empty["result"]["exported"], 0);
    let page_id = "85".repeat(32);
    mcp.data(
        "submit_task",
        request(&page_id, json!({"offset":1,"limit":2})),
    );
    let task = terminal(&fixture, &seeded.account, &page_id);
    assert_eq!(task["status"], "succeeded", "{task}");
    assert_eq!(task["result"]["selected_rows"], 2);
    let page = mcp.data("list_task_artifacts", json!({"id":page_id,"limit":100}));
    let mut audio = Vec::new();
    for item in page["items"].as_array().unwrap() {
        if item["media_type"] != "application/json" {
            audio.push(read_artifact(&mut mcp, &page_id, item));
        }
    }
    audio.sort();
    let mut expected = seeded.audio[1..3].to_vec();
    expected.sort();
    assert_eq!(audio, expected);
    let peer_id = "86".repeat(32);
    mcp.data(
        "submit_task",
        request(&peer_id, json!({"chat":"voice-peer"})),
    );
    let peer = terminal(&fixture, &seeded.account, &peer_id);
    assert_eq!(peer["result"]["selected_rows"], 2, "{peer}");
    assert_eq!(peer["result"]["selection"]["target_username"], "voice-peer");
    let cli = call(
        &fixture,
        &seeded.account,
        &["tasks", "submit", "export_voices", "--limit", "0", "--wait"],
    );
    assert_eq!(cli["result"]["selected_rows"], 0, "{cli}");
    assert_eq!(mcp.data("get_task", json!({"id":cli["id"]})), cli);
}

#[test]
fn raw_voice_tasks_preserve_unproven_audio_without_claiming_complete_success() {
    let mut fixture = Fixture::new();
    let seeded = seed(&mut fixture, "voice-task-incomplete");
    fs::remove_file(seeded.account.join("db_storage/message/message_1.db")).unwrap();
    let mut mcp = Mcp::start(&fixture, &seeded.account, HOST);
    let id = "87".repeat(32);
    mcp.data("submit_task", request(&id, json!({})));
    let task = terminal(&fixture, &seeded.account, &id);
    assert_eq!(task["result"]["scope"], "raw_voices", "{task}");
    assert_eq!(task["result"]["finalized"], true);
    assert_ne!(task["result"]["outcome"], "success", "{task}");
    assert_eq!(task["result"]["exported"], 4, "{task}");
    assert!(task["result"]["unproven"].as_u64().unwrap() > 0);
    assert!(task["result"]["incomplete_items"].as_u64().unwrap() > 0);
    let page = mcp.data("list_task_artifacts", json!({"id":id,"limit":100}));
    let mut audio = Vec::new();
    for item in page["items"].as_array().unwrap() {
        let bytes = read_artifact(&mut mcp, &id, item);
        if item["media_type"] == "application/json" {
            assert_no_physical_paths(&serde_json::from_slice::<Value>(&bytes).unwrap());
        } else {
            audio.push(bytes);
        }
    }
    audio.sort();
    let mut expected = seeded.audio;
    expected.sort();
    assert_eq!(audio, expected);
    let mut write_only = Mcp::start(
        &fixture,
        &seeded.account,
        &[
            "--tasks",
            "--task-kind",
            "export_voices",
            "--task-allow-media-write",
        ],
    );
    refused(&write_only.tool("list_task_artifacts", json!({"id":id})));
    let mut read_only = Mcp::start(
        &fixture,
        &seeded.account,
        &[
            "--tasks",
            "--task-kind",
            "export_voices",
            "--task-allow-artifact-read",
        ],
    );
    assert_eq!(
        read_only.data("list_task_artifacts", json!({"id":id,"limit":100})),
        page
    );
    refused(&read_only.tool("submit_task", request(&"88".repeat(32), json!({}))));
}

#[test]
fn raw_voice_http_requires_host_write_authority_but_can_recover_accepted_requests() {
    let mut fixture = Fixture::new();
    let seeded = seed(&mut fixture, "voice-http-authority");
    let id = "89".repeat(32);
    let body = json!({"kind":"export_voices","options":{"voice_export":{"limit":0}}});
    let before = call(&fixture, &seeded.account, &["tasks", "list"]);
    {
        let web = Web::start(&fixture, &seeded.account);
        let denied = web.request(
            reqwest::Method::POST,
            "/api/tasks",
            Some(&id),
            Some(body.clone()),
        );
        assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);
        let forged = web.request(reqwest::Method::POST, "/api/tasks", Some(&id), Some(json!({"kind":"export_voices","options":{"voice_export":{"limit":0},"allow_media_write":true}})));
        assert!(!forged.status().is_success());
        assert_eq!(call(&fixture, &seeded.account, &["tasks", "list"]), before);
    }
    {
        let web = Web::start_with_flags(&fixture, &seeded.account, &["--task-allow-media-write"]);
        let accepted = web.request(
            reqwest::Method::POST,
            "/api/tasks",
            Some(&id),
            Some(body.clone()),
        );
        assert!(
            accepted.status().is_success(),
            "{}",
            accepted.text().unwrap()
        );
    }
    let task = terminal(&fixture, &seeded.account, &id);
    assert_eq!(task["status"], "succeeded", "{task}");
    {
        let web = Web::start(&fixture, &seeded.account);
        let replay = web.request(reqwest::Method::POST, "/api/tasks", Some(&id), Some(body));
        assert!(replay.status().is_success(), "{}", replay.text().unwrap());
        let replay: Value = serde_json::from_slice(&replay.bytes().unwrap()).unwrap();
        assert_eq!(replay["id"], id, "{replay}");
        let conflict = web.request(
            reqwest::Method::POST,
            "/api/tasks",
            Some(&id),
            Some(json!({"kind":"export_voices","options":{"voice_export":{"limit":1}}})),
        );
        assert_eq!(conflict.status(), reqwest::StatusCode::CONFLICT);
        let fresh = web.request(
            reqwest::Method::POST,
            "/api/tasks",
            Some(&"8a".repeat(32)),
            Some(json!({"kind":"export_voices","options":{"voice_export":{}}})),
        );
        assert_eq!(fresh.status(), reqwest::StatusCode::FORBIDDEN);
        let page = web.request(
            reqwest::Method::GET,
            &format!("/api/tasks/{id}/artifacts"),
            None,
            None,
        );
        assert_eq!(page.status(), reqwest::StatusCode::OK);
    }
    assert_eq!(
        call(&fixture, &seeded.account, &["tasks", "get", &id]),
        task
    );
}

#[test]
#[ignore = "browsermanual: requires explicit WX_VOICE_UI_FIXTURE_INFO; synthetic account only"]
fn raw_voice_ui_fixture_browsermanual() {
    use std::{
        io::Write,
        time::{Duration, Instant},
    };
    let info = PathBuf::from(
        std::env::var_os("WX_VOICE_UI_FIXTURE_INFO").expect("explicit fixture rendezvous required"),
    );
    assert!(info.is_absolute());
    let stop = info.with_extension("stop");
    assert!(!stop.exists(), "stale browser fixture stop marker");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&info)
        .unwrap();
    struct Cleanup(PathBuf, PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
            let _ = fs::remove_file(&self.1);
        }
    }
    let _cleanup = Cleanup(info, stop.clone());
    let mut fixture = Fixture::new();
    let seeded = seed(&mut fixture, "voice-browser-owner");
    let mut web = Web::start_with_flags(&fixture, &seeded.account, &["--task-allow-media-write"]);
    file.write_all(&serde_json::to_vec(&json!({"url":format!("{}/#token={}",web.origin,web.token),"stop_file":stop,"synthetic":true})).unwrap()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let deadline = Instant::now() + Duration::from_secs(600);
    while !stop.is_file() && Instant::now() < deadline {
        assert!(
            web.child.try_wait().unwrap().is_none(),
            "synthetic Web process exited early"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(web);
    for (path, bytes) in seeded.sources {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert!(
        stop.is_file(),
        "browser fixture timed out; processes cleaned up"
    );
}

#[test]
fn raw_voice_time_conflicts_remain_unproven_without_reselecting_media() {
    use chrono::TimeZone;
    let mut fixture = Fixture::new();
    let seeded = seed(&mut fixture, "voice-task-time-conflict");
    for shard in 0..2 {
        let plain = seeded.account.join(format!("voice-message-{shard}.db"));
        let db = rusqlite::Connection::open(&plain).unwrap();
        for username in ["voice-peer", "voice-other"] {
            let table = format!("Msg_{:x}", md5::compute(username));
            db.execute_batch(&format!("UPDATE [{table}] SET create_time=1700000060"))
                .unwrap();
        }
        drop(db);
        crate::encrypt_fixture(
            &plain,
            &seeded
                .account
                .join(format!("db_storage/message/message_{shard}.db")),
        );
    }
    let until = chrono::Local
        .timestamp_opt(1700000000, 0)
        .single()
        .unwrap()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let mut mcp = Mcp::start(&fixture, &seeded.account, HOST);
    let id = "8b".repeat(32);
    mcp.data("submit_task", request(&id, json!({"until":until})));
    let task = terminal(&fixture, &seeded.account, &id);
    assert_eq!(task["result"]["selected_rows"], 4, "{task}");
    assert_eq!(task["result"]["exported"], 4);
    assert_eq!(task["result"]["associated"], 0);
    assert_eq!(task["result"]["unproven"], 4);
    assert_ne!(task["result"]["outcome"], "success");
    let page = mcp.data("list_task_artifacts", json!({"id":id,"limit":100}));
    for item in page["items"].as_array().unwrap() {
        if item["media_type"] == "application/json" {
            let bytes = read_artifact(&mut mcp, &id, item);
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            if value.get("manifest").is_some() {
                for entry in value["manifest"].as_array().unwrap() {
                    assert_eq!(entry["timestamp"], 1700000000);
                    assert_eq!(entry["evidence"]["timestamp_source"], "media");
                }
            } else {
                assert_eq!(value["timestamp"], 1700000000, "{value}");
                assert_eq!(value["timestamp_source"], "media");
                assert!(value["message_timestamp"].is_null());
            }
        }
    }
}
