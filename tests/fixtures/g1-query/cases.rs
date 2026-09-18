use chrono::{Local, TimeZone};
use serde_json::{json, Value};

pub struct Case {
    pub tool: &'static str,
    pub request: Value,
    pub arguments: Value,
    pub cli: Vec<String>,
    pub http: String,
    pub cli_projection: Option<&'static str>,
    pub witnesses: Vec<(&'static str, Value)>,
}

fn case(
    tool: &'static str,
    request: Value,
    arguments: Value,
    cli: &[&str],
    http: &str,
    projection: Option<&'static str>,
    witnesses: Vec<(&'static str, Value)>,
) -> Case {
    Case {
        tool,
        request,
        arguments,
        cli: cli.iter().map(|s| s.to_string()).collect(),
        http: http.into(),
        cli_projection: projection,
        witnesses,
    }
}

pub fn all(marker: &str) -> Vec<Case> {
    let tag = format!("G1-{marker}");
    let date = |ts| {
        Local
            .timestamp_opt(ts, 0)
            .single()
            .unwrap()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    };
    let start = date(10001);
    let end = date(10008);
    let mut cases = vec![
        case("get_contact_tags", json!({"cmd":"contact_tags"}), json!({}),
            &["tags"], "/api/tags", None,
            vec![("/total_tags",json!(3)),("/total_associations",json!(1)),("/tags/0/name",json!(tag))]),
        case("get_tag_members", json!({"cmd":"tag_members","tag_name":tag}), json!({"tag_name":tag}),
            &["tag-members",&tag], &format!("/api/tag-members?name={tag}"), None,
            vec![("/member_count",json!(1)),("/members/0/username",json!("peer")),("/members/0/display_name",json!(format!("Peer{marker}")))]),
        case("get_chat_members", json!({"cmd":"members","chat":"Group"}), json!({"chat_name":"Group"}),
            &["members","Group"], "/api/members?chat=Group", Some("members"),
            vec![("/count",json!(1)),("/membership_complete",json!(true)),("/membership_source",json!("member_directory")),("/members/0/username",json!("peer"))]),
        case("get_chat_history", json!({"cmd":"history","chat":"peer","limit":2,"offset":1,"since":10001,"until":10008,"msg_types":[1,3],"oldest_first":true,"with_meta":true}),
            json!({"chat_name":"peer","limit":2,"offset":1,"since":10001,"until":10008,"msg_types":["text","image"],"oldest_first":true,"with_meta":true}),
            &["history","peer","--limit","2","--offset","1","--since",&start,"--until",&end,"--types","text,image","--oldest-first","--with-meta"],
            "/api/history?chat=peer&limit=2&offset=1&since=10001&until=10008&msg_types=text,image&oldest_first=true&with_meta=true", None,
            vec![("/messages/0/local_id",json!(102)),("/messages/1/local_id",json!(104))]),
        case("search_messages", json!({"cmd":"search","keyword":format!("history-{marker}"),"chats":["peer"],"limit":2,"since":10001,"until":10008,"msg_type":1,"with_meta":true}),
            json!({"keyword":format!("history-{marker}"),"chats":["peer"],"limit":2,"since":10001,"until":10008,"msg_type":1,"with_meta":true}),
            &["search",&format!("history-{marker}"),"--in","peer","--limit","2","--since",&start,"--until",&end,"--type","text","--with-meta"],
            &format!("/api/search?keyword=history-{marker}&chats=peer&limit=2&since=10001&until=10008&msg_type=text&with_meta=true"), None,
            vec![("/count",json!(2)),("/results/0/local_id",json!(108)),("/results/1/local_id",json!(106))]),
        case("get_chat_stats", json!({"cmd":"stats","chat":"peer","since":10001,"until":10008,"with_meta":true}),
            json!({"chat_name":"peer","since":10001,"until":10008,"with_meta":true}),
            &["stats","peer","--since",&start,"--until",&end,"--with-meta"],
            "/api/stats?chat=peer&since=10001&until=10008&with_meta=true", None, vec![("/total",json!(8))]),
        case("get_voice_messages", json!({"cmd":"voice_messages","chat":"peer","limit":1,"offset":0}),
            json!({"chat_name":"peer","limit":1,"offset":0}),
            &["voice-messages","peer","--limit","1","--offset","0"],
            "/api/voice-messages?chat=peer&limit=1&offset=0", None,
            vec![("/count",json!(1)),("/voices/0/local_id",json!(8))]),
        case("get_unread_messages", json!({"cmd":"unread","limit":20,"filter":["private"],"with_meta":true}),
            json!({"limit":20,"filter":["private"],"with_meta":true}),
            &["unread","--limit","20","--filter","private","--with-meta"],
            "/api/unread?limit=20&filter=private&with_meta=true", None,
            vec![("/total",json!(1)),("/sessions/0/username",json!("peer"))]),
        case("get_favorites", json!({"cmd":"favorites","limit":1,"fav_type":1,"query":"needle"}),
            json!({"limit":1,"fav_type":1,"query":"needle"}),
            &["favorites","--limit","1","--type","text","--query","needle"],
            "/api/favorites?limit=1&fav_type=1&query=needle", Some("items"),
            vec![("/count",json!(1)),("/has_more",json!(true)),("/items/0/from",json!(format!("author{marker}")))]),
        case("get_biz_articles", json!({"cmd":"biz_articles","limit":20,"account":"News","unread":true}),
            json!({"limit":20,"account":"News","unread":true}),
            &["biz-articles","--limit","20","--account","News","--unread"],
            "/api/articles?limit=20&account=News&unread=true", Some("articles"),
            vec![("/count",json!(1)),("/articles/0/title",json!(format!("article{marker}"))),("/partial",json!(true)),("/issues",json!(["invalid_content"]))]),
        case("get_sns_feed", json!({"cmd":"sns_feed","limit":20,"user":"peer"}),
            json!({"limit":20,"user":"peer"}), &["sns-feed","--limit","20","--user","peer"],
            "/api/sns-feed?limit=20&user=peer", Some("posts"),
            vec![("/total",json!(1)),("/posts/0/content",json!(format!("needle {marker}"))),("/meta/coverage",json!("local_cache_only"))]),
        case("search_sns", json!({"cmd":"sns_search","keyword":"needle","limit":20,"user":"peer"}),
            json!({"keyword":"needle","limit":20,"user":"peer"}), &["sns-search","needle","--limit","20","--user","peer"],
            "/api/sns-search?keyword=needle&limit=20&user=peer", Some("posts"),
            vec![("/total",json!(1)),("/posts/0/content",json!(format!("needle {marker}")))]),
        case("get_sns_notifications", json!({"cmd":"sns_notifications","limit":20,"include_read":true}),
            json!({"limit":20,"include_read":true}), &["sns-notifications","--limit","20","--include-read"],
            "/api/sns-notifications?limit=20&include_read=true", Some("notifications"),
            vec![("/total",json!(2)),("/notifications/0/content",json!(format!("comment{marker}")))]),
    ];
    for (tool, cli, id, timestamp, index, witness) in [
        (
            "decode_refer",
            "decode-refer",
            7,
            100,
            None,
            ("/refer/reply_text", json!(format!("reply-{marker}-100"))),
        ),
        (
            "decode_file_message",
            "decode-file-message",
            20,
            2000,
            None,
            ("/status", json!("found")),
        ),
        (
            "decode_record_item",
            "decode-record-item",
            40,
            4000,
            Some(1),
            ("/status", json!("found")),
        ),
    ] {
        let mut request = json!({"cmd":tool,"chat":"peer","local_id":id,"create_time":timestamp});
        let mut arguments = json!({"chat_name":"peer","local_id":id,"create_time":timestamp});
        let mut command = vec![cli.to_string(), "peer".into(), id.to_string()];
        let mut http = format!("/api/{cli}?chat=peer&local_id={id}&create_time={timestamp}");
        if let Some(index) = index {
            request["item_index"] = json!(index);
            arguments["item_index"] = json!(index);
            command.push(index.to_string());
            http.push_str(&format!("&item_index={index}"));
        }
        command.push(timestamp.to_string());
        cases.push(Case {
            tool,
            request,
            arguments,
            cli: command,
            http,
            cli_projection: None,
            witnesses: vec![("/exit_code", json!(0)), witness],
        });
    }
    cases
}

pub fn http_projection(tool: &str, mut data: Value) -> Value {
    if tool == "get_chat_history" {
        for row in data["messages"].as_array_mut().unwrap() {
            // Only the documented Web image action is transport-specific.
            if let Some(image) = row.as_object_mut().unwrap().remove("image") {
                assert_eq!(image["status"], "pending");
                assert_eq!(image["binding"], "pending_strict_validation");
                assert_eq!(image["source"], row["source"]);
                assert!(!image["attachment_id"].as_str().unwrap().is_empty());
                assert!(image["decode_url"]
                    .as_str()
                    .unwrap()
                    .starts_with("/api/images/"));
            }
        }
    }
    data
}
