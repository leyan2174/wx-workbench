//! Real CLI -> daemon -> DPAPI/SQLCipher queries over isolated synthetic accounts.
#![cfg(windows)]
#[path = "../src/private_file.rs"]
#[allow(dead_code)] // Shared production module; this fixture does not exercise every entry point.
mod private_file;

#[path = "fixtures/mcp-readonly-runtime/support.rs"]
#[allow(dead_code)]
mod support;

#[path = "support/mcp_failure.rs"]
mod mcp_failure;
use mcp_failure::safe_failure;

use serde_json::Value;
use std::{
    os::windows::process::CommandExt,
    path::Path,
    process::{Command, Output},
};

const RESPONSE_LIMIT_BYTES: usize = 32 * 1024 * 1024;
const LARGE_BODY_BYTES: usize = 256 * 1024;
const LARGE_ROWS: usize = 144;
const CANDIDATE_ROWS: usize = 100_002;
const BODY_MARKER: &str = "synthetic-query-limit-private-body-";

fn body(id: usize, bytes: usize) -> String {
    let mut text = format!("{BODY_MARKER}{id:06}:");
    assert!(text.len() <= bytes);
    text.extend(std::iter::repeat_n('x', bytes - text.len()));
    text
}

fn seed_history(account: &support::Account, rows: usize, body_bytes: usize) {
    assert_eq!(rows % 2, 0);
    assert!(
        body_bytes < 1_048_576,
        "stay below the per-message stored limit"
    );
    assert!(rows * body_bytes < 64 * 1024 * 1024);
    let table = format!("Msg_{:x}", md5::compute(b"peer"));
    // Replace both seeded message shards before starting the daemon. The helper's
    // fixed synthetic key/salt still match its migrated DPAPI key store.
    for shard in 0..2 {
        let plain = account.root().join("limit-build.db");
        let mut conn = support::encrypted_sqlite::sqlite(&plain);
        conn.execute_batch(&format!(
            "CREATE TABLE Name2Id(user_name TEXT);
             INSERT INTO Name2Id(rowid,user_name) VALUES(7,'peer');
             CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,
                 create_time INTEGER,real_sender_id INTEGER,message_content TEXT,
                 WCDB_CT_message_content INTEGER);"
        ))
        .unwrap();
        let transaction = conn.transaction().unwrap();
        {
            let mut insert = transaction
                .prepare(&format!("INSERT INTO [{table}] VALUES(?1,1,?2,7,?3,0)"))
                .unwrap();
            for id in (shard + 1..=rows).step_by(2) {
                insert
                    .execute(rusqlite::params![
                        id as i64,
                        1_700_000_000i64 + id as i64,
                        body(id, body_bytes)
                    ])
                    .unwrap();
            }
        }
        transaction.commit().unwrap();
        let (count, bytes): (usize, usize) = conn
            .query_row(
                &format!(
                    "SELECT count(*),sum(length(CAST(message_content AS BLOB))) FROM [{table}]"
                ),
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((count, bytes), (rows / 2, rows / 2 * body_bytes));
        drop(conn);
        support::encrypted_sqlite::encrypt(
            &plain,
            &account
                .root()
                .join(format!("db_storage/message/message_{shard}.db")),
        );
    }
}

fn history(
    account: &support::Account,
    home: &Path,
    limit: usize,
    offset: usize,
    json: bool,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
    command
        .env_clear()
        .current_dir(account.root())
        .env("PATH", "")
        .env("WX_CLI_CONFIG", account.root().join("config.json"))
        .env("WX_CLI_HOME", home)
        .creation_flags(0x08000000)
        .args(["history", "peer", "--oldest-first", "--limit"])
        .arg(limit.to_string())
        .arg("--offset")
        .arg(offset.to_string());
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    for key in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
        command.env(key, home);
    }
    if json {
        command.arg("--json");
    }
    println!("COMMAND: {command:?}");
    command.output().unwrap()
}

fn assert_limit_error(
    output: &Output,
    account: &support::Account,
    home: &Path,
    code: &str,
    json: bool,
) {
    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "failed history must not emit partial results"
    );
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    assert!(
        stderr.len() < 4096,
        "error must be bounded, not a history dump"
    );
    for forbidden in [
        BODY_MARKER.to_owned(),
        "11".repeat(32),
        account.root().to_string_lossy().into_owned(),
        home.to_string_lossy().into_owned(),
        "db_storage".to_owned(),
        "message_0.db".to_owned(),
        "message_1.db".to_owned(),
        "Business operation failed".to_owned(),
    ] {
        assert!(
            !stderr.contains(&forbidden),
            "error leaked data or lost its classification"
        );
    }
    if json {
        // Parse the whole stream: a JSON object hidden among prose is not enough.
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], code);
        assert_eq!(error["operation"], "history");
        if code == "response_limit_exceeded" {
            assert_eq!(error["response_limit_bytes"], RESPONSE_LIMIT_BYTES);
        } else {
            assert!(error.get("response_limit_bytes").is_none());
        }
    } else {
        assert!(stderr.contains(code), "missing limit classification");
        assert!(stderr.contains("--offset"), "missing offset advice");
        assert!(stderr.contains("--limit"), "missing page-size advice");
    }
    let wrong_code = if code == "response_limit_exceeded" {
        "query_read_limit_exceeded"
    } else {
        "response_limit_exceeded"
    };
    assert!(!stderr.contains(wrong_code), "wrong limit classification");
}

fn assert_small_page(account: &support::Account, home: &Path, body_bytes: usize, json: bool) {
    let output = history(account, home, 2, 3, json);
    assert!(
        output.status.success(),
        "small page failed after oversized history"
    );
    let page: Value = if json {
        serde_json::from_slice(&output.stdout).unwrap()
    } else {
        serde_yaml::from_slice(&output.stdout).unwrap()
    };
    let messages = page["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    for (message, id) in messages.iter().zip([4usize, 5]) {
        assert_eq!(message["local_id"], id);
        assert!(
            message["content"].as_str().unwrap() == body(id, body_bytes),
            "small page body mismatch for synthetic message {id}"
        );
    }
}

fn exercise_limit(rows: usize, body_bytes: usize, code: &str) {
    let home = tempfile::tempdir().unwrap();
    let account = support::Account::new(home.path(), "A");
    seed_history(&account, rows, body_bytes);
    assert!(account.ipc(serde_json::json!({"cmd": "ping"})).is_err());
    // The first JSON request starts the real daemon without contaminating stderr.
    // Account's RuntimeCleanup also owns daemons started through the public CLI.
    for json in [true, false] {
        let output = history(&account, home.path(), rows, 0, json);
        assert_limit_error(&output, &account, home.path(), code, json);
        // No restart, mutation, or substitute response between failure and recovery.
        assert_small_page(&account, home.path(), body_bytes, json);
    }
}

#[test]
fn real_history_response_limit_reports_json_and_text_then_small_pages_succeed() {
    // 36 MiB of ASCII content alone exceeds the wire limit, without relying on
    // JSON escaping or metadata overhead. Each of 144 messages is only 256 KiB.
    const { assert!(LARGE_ROWS * LARGE_BODY_BYTES > RESPONSE_LIMIT_BYTES) };
    exercise_limit(LARGE_ROWS, LARGE_BODY_BYTES, "response_limit_exceeded");
}

#[test]
fn real_history_candidate_budget_is_not_misreported_as_response_limit() {
    // Each shard has only 50,001 rows; merging them exceeds the 100,000-candidate
    // budget. Short bodies keep this distinct from the 32 MiB response case.
    exercise_limit(CANDIDATE_ROWS, 64, "query_read_limit_exceeded");
}
