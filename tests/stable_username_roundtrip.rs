//! Public CLI -> real daemon over isolated synthetic account databases with DPAPI keys.
#![cfg(windows)]
#[path = "../src/private_file.rs"]
#[allow(dead_code)] // Shared production module; this fixture does not exercise every entry point.
mod private_file;

#[path = "support/mcp_failure.rs"]
mod mcp_failure;
#[path = "fixtures/mcp-readonly-runtime/support.rs"]
#[allow(dead_code)]
mod support;
use mcp_failure::safe_failure;

use serde_json::{json, Value};
use std::{
    fs,
    os::windows::process::CommandExt,
    path::Path,
    process::{Command, Output},
};

const IDS: &[&str] = &[
    "legacy.username",
    "external@openim",
    "enterprise@qy_g",
    "wxid_session_only",
    "synthetic@chatroom",
    "filehelper",
    "@placeholder_foldgroup",
];

fn seed(account: &support::Account) {
    let root = account.root();
    let storage = root.join("db_storage");
    let plain = root.join("identity-build.db");
    let contact = support::encrypted_sqlite::sqlite(&plain);
    contact.execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT,remark TEXT,verify_flag INTEGER,local_type INTEGER,alias TEXT,extra_buffer BLOB); CREATE TABLE contact_label(label_id_,label_name_,sort_order_); INSERT INTO contact VALUES('duplicate_a','Same Name','',0,0,'',NULL),('duplicate_b','Same Name','',0,0,'',NULL),('display_collision','legacy.username','',0,0,'',NULL),('contact_only','Contact Only','',0,0,'',NULL);").unwrap();
    drop(contact);
    support::encrypted_sqlite::encrypt(&plain, &storage.join("contact/contact.db"));

    fs::create_dir_all(storage.join("session")).unwrap();
    let session = support::encrypted_sqlite::sqlite(&plain);
    session.execute_batch("CREATE TABLE SessionTable(username TEXT,unread_count INTEGER,summary TEXT,last_timestamp INTEGER,last_msg_type INTEGER,last_msg_sender TEXT,last_sender_display_name TEXT)").unwrap();
    for username in IDS.iter().copied().chain([
        "brandsessionholder",
        "ordinary_without_table",
        "ordinary_empty_table",
    ]) {
        session
            .execute(
                "INSERT INTO SessionTable VALUES(?1,0,'synthetic',100,1,'','')",
                [username],
            )
            .unwrap();
    }
    drop(session);
    support::encrypted_sqlite::encrypt(&plain, &storage.join("session/session.db"));

    for shard in 0..2 {
        let conn = support::encrypted_sqlite::sqlite(&plain);
        conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id(rowid,user_name) VALUES(1,'table_only@openim')").unwrap();
        if shard == 0 {
            for username in IDS
                .iter()
                .copied()
                .chain(["table_only@openim", "ordinary_empty_table"])
            {
                let table = format!("Msg_{:x}", md5::compute(username));
                conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,server_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER)")).unwrap();
                if username != "ordinary_empty_table" {
                    conn.execute(
                        &format!("INSERT INTO [{table}] VALUES(1,3,100,0,42,?1,0)"),
                        ["<msg><img md5=\"11111111111111111111111111111111\" /></msg>"],
                    )
                    .unwrap();
                }
            }
        }
        drop(conn);
        support::encrypted_sqlite::encrypt(
            &plain,
            &storage.join(format!("message/message_{shard}.db")),
        );
    }
    let keys_path = root.join("keys.json");
    let mut keys: Value = serde_json::from_slice(&fs::read(&keys_path).unwrap()).unwrap();
    keys["session/session.db"] = json!("11".repeat(32));
    fs::write(keys_path, serde_json::to_vec(&keys).unwrap()).unwrap();
    account.seed_keys(&keys);
}

fn cli(account: &support::Account, home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
    command
        .env_clear()
        .current_dir(account.root())
        .env("PATH", "")
        .env("WX_CLI_CONFIG", account.root().join("config.json"))
        .env("WX_CLI_HOME", home)
        .creation_flags(0x08000000)
        .args(args);
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.output().unwrap()
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn public_sessions_usernames_round_trip_to_history_and_attachments() {
    let home = tempfile::tempdir().unwrap();
    let mut account = support::Account::new(home.path(), "IDENTITY");
    seed(&account);
    account.start();
    let sessions = success(cli(
        &account,
        home.path(),
        &["sessions", "--limit", "100", "--json"],
    ));
    let rows = sessions["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), IDS.len() + 3);
    for row in rows {
        let username = row["username"].as_str().unwrap();
        if username == "brandsessionholder" {
            assert_eq!(row["exportable"], false);
            assert_eq!(
                row["skip_reason"],
                "system_placeholder_without_message_table"
            );
            continue;
        }
        assert_eq!(row["exportable"], true, "{username}");
        assert!(row.get("skip_reason").is_none());
        let history = success(cli(
            &account,
            home.path(),
            &["history", username, "--limit", "10", "--json"],
        ));
        assert_eq!(history["username"], username);
        if username.starts_with("ordinary_") {
            assert_eq!(history["count"], 0);
            continue;
        }
        assert_eq!(history["count"], 1);
        let attachments = success(cli(
            &account,
            home.path(),
            &[
                "attachments",
                username,
                "--kind",
                "image",
                "--limit",
                "10",
                "--json",
            ],
        ));
        assert_eq!(attachments["username"], username);
        assert_eq!(attachments["count"], 1);
        assert!(attachments["attachments"][0]["attachment_id"].is_string());
    }
    // A Name2Id + actual table identity is also accepted without contact/session evidence.
    for command in ["history", "attachments"] {
        let value = success(cli(
            &account,
            home.path(),
            &[command, "table_only@openim", "--json"],
        ));
        assert_eq!(value["username"], "table_only@openim");
        assert_eq!(value["count"], 1);
        for invalid in [
            "Same Name",
            "wxid_not_proven",
            "unproven@chatroom",
            "external@OPENIM",
            "external@openim ",
        ] {
            assert!(
                !cli(&account, home.path(), &[command, invalid, "--json"])
                    .status
                    .success(),
                "{command}: {invalid}"
            );
        }
    }
    account.stop();

    let other_home = tempfile::tempdir().unwrap();
    let mut other = support::Account::new(other_home.path(), "OTHER");
    other.start();
    for command in ["history", "attachments"] {
        assert!(!cli(
            &other,
            other_home.path(),
            &[command, "external@openim", "--json"]
        )
        .status
        .success());
    }
    other.stop();
}
