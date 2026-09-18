//! tools/call -> MCP account pin -> production dispatch_state -> encrypted SQLite.
use crate::{
    daemon::mcp_service::{dispatch, Call, HostSettings},
    ipc::Request,
    mcp::protocol::CallContext,
    runtime::RuntimeContext,
};
use crate::{
    daemon::{
        query::encrypted_cache::encrypted_sqlite, query_state::QueryState, server::dispatch_state,
    },
    mcp::protocol::{Controlled, Protocol, PROTOCOL_VERSION},
};
use serde_json::{json, Value};

struct Account {
    _root: tempfile::TempDir,
    runtime: RuntimeContext,
    query: QueryState,
    call: Call,
}

impl Account {
    fn new(marker: &str, missing_shard: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let config = crate::config::Config {
            key_store: Some(root.path().join("keys.dpapi")),
            db_dir: root.path().join("db"),
            keys_file: root.path().join("keys.json"),
            decrypted_dir: root.path().join("decrypted"),
            wechat_process: "SyntheticNeverLaunched.exe".into(),
        };
        std::fs::create_dir_all(&config.db_dir).unwrap();
        let path = root.path().join("config.json");
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let runtime =
            RuntimeContext::from_config(path, config, root.path().join("runtime")).unwrap();
        let message_table = format!("Msg_{:x}", md5::compute("peer"));
        let article_table = format!("Msg_{:x}", md5::compute("gh_news"));
        let sources = [
            ("contact/contact.db", format!(
                "CREATE TABLE contact(id INTEGER,username TEXT,nick_name TEXT,remark TEXT,verify_flag INTEGER,local_type INTEGER);
                 INSERT INTO contact VALUES(1,'peer','Peer{marker}','',0,1),(2,'g@chatroom','Group','',0,1),(3,'gh_news','News','',8,3),(4,'other','Same','',0,1),(5,'another','Same','',0,1);
                 CREATE TABLE chat_room(id INTEGER,username TEXT,owner TEXT);
                 CREATE TABLE chatroom_member(room_id INTEGER,member_id INTEGER);
                 INSERT INTO chat_room VALUES(7,'g@chatroom','peer');
                 INSERT INTO chatroom_member VALUES(7,1);")),
            ("session/session.db", format!(
                "CREATE TABLE SessionTable(username TEXT,unread_count INTEGER,summary TEXT,last_timestamp INTEGER,last_msg_type INTEGER,last_msg_sender TEXT,last_sender_display_name TEXT);
                 INSERT INTO SessionTable VALUES('peer',2,'summary{marker}',20,1,'peer',''),('g@chatroom',1,'group summary',15,1,'peer',''),('gh_news',1,'news summary',20,49,'gh_news',''),('other',0,'read summary',30,1,'other','');")),
            ("message/message_0.db", format!(
                "CREATE TABLE Name2Id(user_name TEXT);
                 INSERT INTO Name2Id VALUES('peer');
                 CREATE TABLE [{message_table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER);
                 INSERT INTO [{message_table}] VALUES(1,1,10,1,'needle first {marker}',0),(2,3,15,1,'image',0),(3,1,20,1,'needle last {marker}',0);")),
            ("favorite/favorite.db", format!(
                "CREATE TABLE fav_db_item(local_id INTEGER,type INTEGER,update_time INTEGER,content TEXT,fromusr TEXT,realchatname TEXT);
                 INSERT INTO fav_db_item VALUES(7,1,20000,'needle {marker}','author{marker}','peer'),(8,2,10000,'image','author{marker}','peer');")),
            ("message/biz_message_0.db", format!(
                "CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES('gh_news');
                 CREATE TABLE [{article_table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,WCDB_CT_message_content INTEGER,message_content TEXT);
                 INSERT INTO [{article_table}] VALUES(1,49,10,0,'<msg><item><title>old</title><url>https://example.test/old</url><pub_time>100</pub_time></item></msg>'),(2,49,20,0,'<msg><item><title>article{marker}</title><url>https://example.test/new</url><pub_time>200</pub_time></item></msg>'),(3,49,15,0,'broken XML');")),
            ("sns/sns.db", format!(
                "CREATE TABLE SnsTimeLine(tid INTEGER,user_name TEXT,content TEXT);
                 INSERT INTO SnsTimeLine VALUES(1,'peer','<TimelineObject><username>peer</username><createTime>10</createTime><contentDesc>needle {marker}</contentDesc></TimelineObject>'),(2,'other','<TimelineObject><username>peer</username><createTime>20</createTime><contentDesc>other</contentDesc></TimelineObject>');
                 CREATE TABLE SnsMessage_tmp3(local_id INTEGER,create_time INTEGER,feed_id INTEGER,from_username TEXT,from_nickname TEXT,content TEXT,is_unread INTEGER);
                 INSERT INTO SnsMessage_tmp3 VALUES(1,10,99,'peer','','',1),(2,20,99,'peer','','comment{marker}',0);")),
        ];
        let mut keys = serde_json::Map::new();
        for (index, (key, sql)) in sources.iter().enumerate() {
            let plain = root.path().join(format!("source{index}.db"));
            let connection = encrypted_sqlite::sqlite(&plain);
            connection.execute_batch(sql).unwrap();
            drop(connection);
            let target = runtime.config.db_dir.join(key);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            encrypted_sqlite::encrypt(&plain, &target);
            keys.insert((*key).into(), json!("11".repeat(32)));
        }
        if missing_shard {
            keys.insert("message/message_1.db".into(), json!("11".repeat(32)));
        }
        crate::key_store::seed_databases(&runtime, Value::Object(keys));
        let stored = crate::key_store::Store::for_runtime(&runtime)
            .unwrap()
            .load()
            .unwrap();
        assert_eq!(
            stored.database_keys().len(),
            sources.len() + usize::from(missing_shard)
        );
        assert!(!runtime.config.keys_file.exists());
        let call = Call {
            session: runtime.id.clone(),
            open_session: true,
            owner_pid: std::process::id(),
            runtime_id: runtime.id.clone(),
            host: HostSettings::default(),
            budget: CallContext::default().budget().unwrap(),
            request: None,
        };
        let query = QueryState::new(runtime.clone());
        Self {
            _root: root,
            runtime,
            query,
            call,
        }
    }
}

fn send<D: crate::mcp::protocol::Dispatcher>(protocol: &mut Protocol<D>, value: Value) -> Value {
    protocol
        .handle(&serde_json::to_vec(&value).unwrap())
        .unwrap_or(Value::Null)
}

fn ready<D: crate::mcp::protocol::Dispatcher>(protocol: &mut Protocol<D>) {
    let reply = send(
        protocol,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"g1-fixture","version":"1"}}}),
    );
    assert_eq!(reply["result"]["protocolVersion"], PROTOCOL_VERSION);
    send(
        protocol,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
}

fn invoke<D: crate::mcp::protocol::Dispatcher>(
    protocol: &mut Protocol<D>,
    name: &str,
    args: Value,
) -> Value {
    send(
        protocol,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":args}}),
    )
}

fn data<D: crate::mcp::protocol::Dispatcher>(
    protocol: &mut Protocol<D>,
    name: &str,
    args: Value,
) -> Value {
    let reply = invoke(protocol, name, args);
    assert_eq!(reply["result"]["isError"], false, "{name}: {reply}");
    serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn readonly_tools_dispatch_real_encrypted_sources_for_two_accounts() {
    let executor = tokio::runtime::Runtime::new().unwrap();
    let accounts = [Account::new("A", false), Account::new("B", false)];
    for (account, marker) in [
        (&accounts[0], "A"),
        (&accounts[1], "B"),
        (&accounts[0], "A"),
    ] {
        let mut protocol = Protocol::new(Controlled(|request: Request, context: &CallContext| {
            let mut call = account.call.clone();
            call.budget = context.budget()?;
            call.request = Some(Box::new(request));
            crate::service::mcp::unpack(dispatch(call, &account.runtime, |request, _, _| {
                Ok(executor.block_on(dispatch_state(request, &account.query)))
            }))
        }));
        ready(&mut protocol);
        let unread = data(
            &mut protocol,
            "get_unread_messages",
            json!({"filter":["private"],"with_meta":true}),
        );
        assert_eq!(unread["total"], 1);
        assert_eq!(unread["sessions"][0]["username"], "peer");
        assert_eq!(
            data(
                &mut protocol,
                "get_unread_messages",
                json!({"filter":["group"]})
            )["sessions"][0]["username"],
            "g@chatroom"
        );
        let members = data(
            &mut protocol,
            "get_chat_members",
            json!({"chat_name":"Group"}),
        );
        assert_eq!(members["count"], 1);
        assert_eq!(
            members["members"][0]["contact_display"],
            format!("Peer{marker}")
        );
        assert_eq!(members["membership_complete"], true);
        assert_eq!(members["membership_source"], "member_directory");
        let stats = data(
            &mut protocol,
            "get_chat_stats",
            json!({"chat_name":"peer","since":10,"until":15,"with_meta":true}),
        );
        assert_eq!(stats["total"], 2);
        assert!(stats["meta"].is_object());
        assert!(stats["meta"].get("shard_paths").is_none());
        let favorites = data(
            &mut protocol,
            "get_favorites",
            json!({"fav_type":1,"query":"needle","limit":1}),
        );
        assert_eq!(favorites["count"], 1);
        assert_eq!(favorites["items"][0]["from"], format!("author{marker}"));
        assert_eq!(favorites["has_more"], false);
        assert_eq!(
            data(&mut protocol, "get_favorites", json!({"fav_type":5}))["count"],
            0
        );
        let articles = data(
            &mut protocol,
            "get_biz_articles",
            json!({"account":"News","since":10,"until":20,"unread":true}),
        );
        assert_eq!(articles["count"], 1);
        assert_eq!(articles["articles"][0]["title"], format!("article{marker}"));
        assert_eq!(articles["articles"][0]["recv_time"], 20);
        assert_eq!(articles["articles"][0]["timestamp"], 200);
        assert_eq!(articles["partial"], true);
        assert_eq!(articles["issues"], json!(["invalid_content"]));
        let feed = data(
            &mut protocol,
            "get_sns_feed",
            json!({"user":"peer","since":10,"until":10}),
        );
        assert_eq!(feed["total"], 1);
        assert_eq!(feed["posts"][0]["content"], format!("needle {marker}"));
        assert_eq!(feed["meta"]["coverage"], "local_cache_only");
        let all_feed = data(&mut protocol, "get_sns_feed", json!({}));
        assert_eq!(all_feed["meta"]["author_conflicts"], 1);
        let search = data(
            &mut protocol,
            "search_sns",
            json!({"keyword":"needle","user":"peer","since":10,"until":20}),
        );
        assert_eq!(search["posts"], feed["posts"]);
        assert_eq!(search["meta"]["coverage"], "local_cache_only");
        assert_eq!(
            data(&mut protocol, "search_sns", json!({"keyword":"absent"}))["total"],
            0
        );
        let notifications = data(
            &mut protocol,
            "get_sns_notifications",
            json!({"since":10,"until":20}),
        );
        assert_eq!(notifications["total"], 1);
        assert_eq!(notifications["notifications"][0]["timestamp"], 10);
        assert_eq!(
            data(
                &mut protocol,
                "get_sns_notifications",
                json!({"include_read":true})
            )["total"],
            2
        );
        // Reading with include_read does not mutate the unread state.
        assert_eq!(
            data(&mut protocol, "get_sns_notifications", json!({}))["total"],
            1
        );
        let history = data(
            &mut protocol,
            "get_chat_history",
            json!({"chat_name":"peer","msg_types":["text"],"since":10,"until":20,"oldest_first":true,"limit":1,"with_meta":true}),
        );
        assert_eq!(history["messages"][0]["local_id"], 1);
        let search = data(
            &mut protocol,
            "search_messages",
            json!({"keyword":"needle","chats":["peer"],"msg_type":1,"since":10,"until":20,"offset":1,"limit":1,"with_meta":true}),
        );
        assert_eq!(search["count"], 1);
        assert_eq!(search["results"][0]["local_id"], 1);
        assert!(search["meta"].is_object());
        assert!(search["meta"].get("shard_paths").is_none());
        assert!(history["meta"].get("shard_paths").is_none());
        for (name, args) in [
            ("get_chat_members", json!({"chat_name":"missing"})),
            ("get_chat_stats", json!({"chat_name":"Same"})),
            ("get_sns_feed", json!({"user":"Same"})),
        ] {
            let reply = invoke(&mut protocol, name, args);
            assert_eq!(reply["result"]["isError"], true, "{reply}");
            assert!(!reply
                .to_string()
                .contains(account._root.path().to_str().unwrap()));
        }
    }
}

#[test]
fn missing_shard_and_runtime_switch_are_not_successful_empty_results() {
    let executor = tokio::runtime::Runtime::new().unwrap();
    let account = Account::new("A", true);
    let other = Account::new("B", false);
    let mut protocol = Protocol::new(Controlled(|request: Request, context: &CallContext| {
        let mut call = account.call.clone();
        call.budget = context.budget()?;
        call.request = Some(Box::new(request));
        crate::service::mcp::unpack(dispatch(call, &account.runtime, |request, _, _| {
            Ok(executor.block_on(dispatch_state(request, &account.query)))
        }))
    }));
    ready(&mut protocol);
    let reply = invoke(&mut protocol, "get_chat_stats", json!({"chat_name":"peer"}));
    assert_eq!(reply["result"]["isError"], true);
    assert_eq!(reply["result"]["content"][0]["text"], "Query failed");
    assert_eq!(data(&mut protocol, "get_favorites", json!({}))["count"], 2);
    let mut switched = account.call.clone();
    switched.runtime_id = other.runtime.id.clone();
    switched.request = Some(Box::new(Request::Favorites {
        limit: 1,
        fav_type: None,
        query: None,
    }));
    let response = dispatch(switched, &other.runtime, |_, _, _| {
        panic!("a pinned MCP session must not switch accounts")
    });
    assert!(crate::service::mcp::unpack(response).is_err());
}
