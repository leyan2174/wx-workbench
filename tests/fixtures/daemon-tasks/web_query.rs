//! Real binary -> HTTP -> authenticated query pipe -> synthetic SQLCipher sources.
use super::*;

fn account(fixture: &mut Fixture, name: &str) -> PathBuf {
    let account = fixture.account(name, true);
    let plain = account.join("fixture.db");
    let db = rusqlite::Connection::open(&plain).unwrap();
    db.execute_batch(
        "ALTER TABLE contact ADD COLUMN extra_buffer BLOB;
         CREATE TABLE contact_label(label_id_,label_name_,sort_order_);
         INSERT INTO contact_label VALUES(7,'Friends',1);
         UPDATE contact SET extra_buffer=x'f2010137';",
    )
    .unwrap();
    let second = format!("{name}-second");
    db.execute(
        "INSERT INTO contact(username,nick_name,remark,verify_flag) VALUES(?1,?1,'',0)",
        [&second],
    )
    .unwrap();
    drop(db);
    super::super::encrypt_fixture(&plain, &account.join("db_storage/contact/contact.db"));

    let sessions = account.join("sessions-plain.db");
    fs::copy(&plain, &sessions).unwrap();
    let db = rusqlite::Connection::open(&sessions).unwrap();
    db.execute_batch(
        "CREATE TABLE SessionTable(username,unread_count,summary,last_timestamp,
         last_msg_type,last_msg_sender,last_sender_display_name);",
    )
    .unwrap();
    for (user, timestamp) in [(name, 300), (second.as_str(), 200)] {
        db.execute(
            "INSERT INTO SessionTable VALUES(?1,2,?2,?3,1,NULL,NULL)",
            rusqlite::params![user, format!("summary-{user}"), timestamp],
        )
        .unwrap();
    }
    drop(db);
    let encrypted = account.join("db_storage/session/session.db");
    fs::create_dir_all(encrypted.parent().unwrap()).unwrap();
    super::super::encrypt_fixture(&sessions, &encrypted);
    let keys = json!({
        "contact/contact.db":"11".repeat(32),
        "session/session.db":"11".repeat(32),
    });
    fs::write(account.join("all_keys.json"), keys.to_string()).unwrap();
    fixture.seed_keys(&account, &keys);
    account
}

fn get(web: &Web, path: &str) -> Value {
    let response = web.request(reqwest::Method::GET, path, None, None);
    let status = response.status();
    let body = response.text().unwrap();
    assert_eq!(status, 200, "{path}: {body}");
    serde_json::from_str(&body).unwrap()
}

fn assert_account(web: &Web, name: &str) {
    let contacts = get(web, "/api/contacts?limit=1");
    assert_eq!(contacts["contacts"].as_array().unwrap().len(), 1);
    assert_eq!(contacts["total"], 2);
    let all = get(web, "/api/contacts");
    let mut users: Vec<_> = all["contacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            assert_eq!(row["name"], row["display"]);
            row["username"].as_str().unwrap().to_owned()
        })
        .collect();
    users.sort();
    assert_eq!(users, [name.to_owned(), format!("{name}-second")]);

    let sessions = get(web, "/api/sessions?limit=1");
    let rows = sessions["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["username"], name);
    assert_eq!(rows[0]["name"], rows[0]["chat"]);
    assert_eq!(rows[0]["last_ts"], 300);
    assert_eq!(rows[0]["type"], rows[0]["chat_type"]);
    assert_eq!(rows[0]["summary"], format!("summary-{name}"));

    let tag = get(web, "/api/tag-members?name=Friends");
    assert_eq!(tag["name"], "Friends");
    assert_eq!(tag["member_count"], 1);
    assert_eq!(tag["members"][0]["username"], name);
    assert_eq!(tag["members"][0]["display_name"], name);
}

#[test]
fn three_web_query_routes_use_real_daemon_v3_and_stay_account_bound() {
    let mut fixture = Fixture::new();
    let a = account(&mut fixture, "web-query-a");
    let b = account(&mut fixture, "web-query-b");
    let first = Web::start(&fixture, &a);
    let second = Web::start(&fixture, &b);
    for (web, name) in [
        (&first, "web-query-a"),
        (&second, "web-query-b"),
        (&first, "web-query-a"),
    ] {
        assert_account(web, name);
    }

    for path in [
        "/api/contacts",
        "/api/sessions",
        "/api/tag-members?name=Friends",
    ] {
        let response = first
            .http
            .get(format!("{}{path}", first.origin))
            .header("x-wx-token", &second.token)
            .send()
            .unwrap();
        assert_eq!(response.status(), 401);
    }
    for path in [
        "/api/contacts?limit=0",
        "/api/sessions?limit=2001",
        "/api/tag-members?name=Friends&source=other",
    ] {
        assert_eq!(
            first
                .request(reqwest::Method::GET, path, None, None)
                .status(),
            400
        );
    }

    // Web queries remain connect-only after an explicit daemon stop.
    let info = call(&fixture, &a, &["tasks", "info"]);
    let directory = fixture
        .root
        .join("shared-runtime/accounts")
        .join(info["runtime_id"].as_str().unwrap());
    success(fixture.run(&a, &["daemon", "stop"]));
    for path in [
        "/api/contacts",
        "/api/sessions",
        "/api/tag-members?name=Friends",
    ] {
        assert_eq!(
            first
                .request(reqwest::Method::GET, path, None, None)
                .status(),
            503
        );
    }
    assert!(!directory.join("daemon.pid").exists());
    assert!(!directory.join("service-token.key").exists());
    assert_account(&second, "web-query-b");
}
