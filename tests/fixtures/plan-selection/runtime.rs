//! 实际 CLI -> 实际 daemon -> 合成加密 SQLite；不替代选择器或导出响应。
#![cfg(windows)]
#[path = "../mcp-readonly-runtime/encrypted_sqlite.rs"]
mod encrypted_sqlite;
#[path = "../mcp-readonly-runtime/support.rs"]
#[allow(dead_code)]
mod support;
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn safe_failure(reply: Value, expected: &str) {
    assert_eq!(
        reply["result"],
        json!({"isError":true,"content":[{"type":"text","text":expected}]})
    );
    assert!(reply.get("error").is_none());
}

fn seed(root: &Path) {
    let storage = root.join("db_storage");
    fs::create_dir(storage.join("session")).unwrap();
    for part in ["session", "contact", "message_0", "message_1"] {
        let plain = root.join(format!("selection-{part}.db"));
        let conn = encrypted_sqlite::sqlite(&plain);
        let relative = if part == "session" {
            conn.execute_batch("CREATE TABLE SessionTable(username TEXT); INSERT INTO SessionTable VALUES('peer'),('other')").unwrap();
            "session/session.db".to_owned()
        } else if part == "contact" {
            conn.execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT,verify_flag INTEGER,extra_buffer BLOB); INSERT INTO contact VALUES('peer','Same Name','',0,NULL),('other','Same Name','',0,NULL)").unwrap();
            "contact/contact.db".to_owned()
        } else {
            conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id(rowid,user_name) VALUES(7,'peer'),(8,'other')").unwrap();
            for user in ["peer", "other"] {
                let table = format!("Msg_{:x}", md5::compute(user));
                conn.execute_batch(&format!("CREATE TABLE {table}(local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER)")).unwrap();
                let rows = match (part, user) {
                    ("message_0", "peer") => vec![(1, 100), (2, 200)],
                    ("message_1", "peer") => vec![(3, 201)],
                    ("message_0", "other") => vec![(1, 150)],
                    _ => vec![],
                };
                for (id, timestamp) in rows {
                    conn.execute(
                        &format!("INSERT INTO {table} VALUES(?1,1,?2,7,?3,0)"),
                        rusqlite::params![id, timestamp, format!("{user}-{timestamp}")],
                    )
                    .unwrap();
                }
            }
            format!("message/{part}.db")
        };
        drop(conn);
        encrypted_sqlite::encrypt(&plain, &storage.join(relative));
    }
    let keys_path = root.join("keys.json");
    let mut keys: Value = serde_json::from_slice(&fs::read(&keys_path).unwrap()).unwrap();
    keys["session/session.db"] = json!("11".repeat(32));
    fs::write(keys_path, serde_json::to_vec(&keys).unwrap()).unwrap();
}

fn run(
    account: &support::Account,
    home: &Path,
    output: &Path,
    args: &[&str],
    users: Option<&str>,
) -> Output {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wx"));
    cmd.env_clear()
        .current_dir(account.root())
        .env("PATH", "")
        .env("WX_CLI_CONFIG", account.root().join("config.json"))
        .env("WX_CLI_HOME", home)
        .args(["toolkit", "export-chats-native"])
        .arg(output)
        .args(args)
        .creation_flags(0x0800_0000);
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    for key in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
        cmd.env(key, home);
    }
    if let Some(users) = users {
        cmd.env("WECHAT_EXPORT_USERS", users);
    }
    println!(
        "COMMAND: {cmd:?}; ENV: {:?}",
        cmd.get_envs().collect::<Vec<_>>()
    );
    let output = cmd.output().unwrap();
    println!(
        "STDOUT:\n{}\nSTDERR:\n{}\nEXIT: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        output.status
    );
    output
}

fn data(output: Output) -> Value {
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn selected(value: &Value) -> Vec<&str> {
    value["chats"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["username"].as_str().unwrap())
        .collect()
}

#[test]
fn real_cli_selects_only_csv_usernames_and_preserves_date_incremental_and_dry_run() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let mut account = support::Account::new(home.path(), "A");
    seed(account.root());
    account.migrate_keys();
    let before = account.snapshot();
    account.start();
    let output = work.path().join("export");
    let plan = work.path().join("plan.csv");
    let plan_name = plan.to_str().unwrap();
    let oracle: Value = serde_json::from_str(include_str!("oracle.json")).unwrap();
    let mut writer = csv::Writer::from_writer(vec![0xef, 0xbb, 0xbf]);
    writer
        .write_record(
            oracle["fields"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap()),
        )
        .unwrap();
    for (username, flag) in [("other", ""), ("peer", "1")] {
        writer
            .write_record([
                flag,
                "999",
                username,
                "deliberately wrong display",
                "ignored",
                "0",
                "",
                "",
                "0",
                "",
                "0",
                "ok",
            ])
            .unwrap();
    }
    let bytes = writer.into_inner().unwrap();
    fs::write(&plan, &bytes).unwrap();
    assert_eq!(
        selected(&data(run(
            &account,
            home.path(),
            &output,
            &["--dry-run"],
            None
        ))),
        ["peer", "other"]
    );
    let common = ["--dry-run", "--from-plan-csv", plan_name];
    assert_eq!(
        selected(&data(run(&account, home.path(), &output, &common, None))),
        ["other", "peer"]
    );
    let white = [
        "--dry-run",
        "--from-plan-csv",
        plan_name,
        "--plan-mode",
        "whitelist",
    ];
    let value = data(run(&account, home.path(), &output, &white, None));
    assert_eq!(selected(&value), ["peer"]);
    assert_eq!(value["chats"][0]["chat"], "Same Name");
    assert!(!output.exists());
    assert!(!run(&account, home.path(), &output, &common, Some("peer"))
        .status
        .success());
    assert_eq!(
        selected(&data(run(
            &account,
            home.path(),
            &output,
            &white,
            Some("peer")
        ))),
        ["peer"]
    );
    assert_eq!(
        selected(&data(run(
            &account,
            home.path(),
            &output,
            &["--dry-run", "--users", "other"],
            Some("peer")
        ))),
        ["other"]
    );
    assert!(!run(
        &account,
        home.path(),
        &output,
        &["--plan-mode", "whitelist"],
        None
    )
    .status
    .success());
    assert!(!output.exists());

    let written = data(run(
        &account,
        home.path(),
        &output,
        &[
            "--from-plan-csv",
            plan_name,
            "--plan-mode",
            "whitelist",
            "--start",
            "100",
            "--end",
            "200",
        ],
        None,
    ));
    assert_eq!(written["written"], 1);
    assert_eq!(written["messages"], 2);
    let added = data(run(
        &account,
        home.path(),
        &output,
        &[
            "--from-plan-csv",
            plan_name,
            "--plan-mode",
            "whitelist",
            "--start",
            "201",
            "--end",
            "201",
            "--incremental",
        ],
        None,
    ));
    assert_eq!(added["messages"], 3);
    assert_eq!(added["added_messages"], 1);
    let documents: Vec<Value> = fs::read_dir(&output)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            if path.file_name().unwrap() == "_export_index.json"
                || path.extension().is_none_or(|extension| extension != "json")
            {
                return None;
            }
            Some(serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
        })
        .collect();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0]["username"], "peer");
    assert_eq!(fs::read(&plan).unwrap(), bytes);

    let empty_output = work.path().join("empty");
    fs::write(&plan, b"username,export\nmissing,0\n").unwrap();
    let empty = data(run(
        &account,
        home.path(),
        &empty_output,
        &["--from-plan-csv", plan_name],
        None,
    ));
    assert_eq!(empty["total"], 0);
    assert!(!empty_output.exists());
    for invalid in [
        "username,export\npeer,0\npeer,0\n",
        "username,export\nSame Name,1\n",
        "username,export\n\"peer,1\n",
    ] {
        fs::write(&plan, invalid).unwrap();
        assert!(!run(
            &account,
            home.path(),
            &empty_output,
            &["--from-plan-csv", plan_name],
            None
        )
        .status
        .success());
        assert!(!empty_output.exists());
        assert_eq!(fs::read_to_string(&plan).unwrap(), invalid);
    }
    assert_eq!(account.snapshot(), before);
}
