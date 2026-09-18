//! Narrow HTTP queries, with one explicitly typed Web RPC call per route.
#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use serde_json::json;

    fn pairs(value: &Value) -> Vec<(String, String)> {
        value
            .as_object()
            .unwrap()
            .iter()
            .filter(|(key, value)| key.as_str() != "op" && !value.is_null())
            .map(|(key, value)| {
                (
                    key.clone(),
                    match value {
                        Value::String(value) => value.clone(),
                        Value::Array(values) => values
                            .iter()
                            .map(|value| {
                                value
                                    .as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| value.to_string())
                            })
                            .collect::<Vec<_>>()
                            .join(","),
                        value => value.to_string(),
                    },
                )
            })
            .collect()
    }

    pub(crate) async fn check_http(state: &Shared) -> anyhow::Result<()> {
        use axum::http::StatusCode;
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(3))
            .build()?;
        for (path, expected) in crate::service::web::read_query_cases() {
            let url = format!("{}{path}", state.origin);
            assert_eq!(
                client.get(&url).send().await?.status(),
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
            assert_eq!(
                client
                    .get(&url)
                    .header("x-wx-token", &state.token)
                    .header("origin", "https://untrusted.invalid")
                    .send()
                    .await?
                    .status(),
                StatusCode::FORBIDDEN,
                "{path}"
            );
            assert_eq!(
                client
                    .get(&url)
                    .header("x-wx-token", &state.token)
                    .query(&[("runtime_id", "other-account")])
                    .send()
                    .await?
                    .status(),
                StatusCode::BAD_REQUEST,
                "{path}"
            );
            let response = client
                .get(&url)
                .header("x-wx-token", &state.token)
                .header("origin", &state.origin)
                .query(&pairs(&expected))
                .send()
                .await?;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(response.headers()["cache-control"], "no-store");
            assert_eq!(
                serde_json::from_slice::<Value>(&response.bytes().await?)?["web_call"],
                expected,
                "{path}"
            );
        }
        for query in [
            "until=200",
            "msg_type=text",
            "oldest_first=true",
            "msg_types=text,image",
        ] {
            assert_eq!(
                client
                    .get(format!("{}/api/history?{query}", state.origin))
                    .header("x-wx-token", &state.token)
                    .send()
                    .await?
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
        Ok(())
    }

    #[test]
    fn every_http_query_preserves_all_business_parameters() {
        for (path, expected) in crate::service::web::read_query_cases() {
            let op = expected["op"].as_str().unwrap();
            let call = parse_call(op, pairs(&expected)).unwrap_or_else(|_| panic!("{path}"));
            assert_eq!(serde_json::to_value(call).unwrap(), expected, "{path}");
            for field in [
                "runtime_id",
                "config_path",
                "db_dir",
                "output_root",
                "request",
                "op",
                "unknown",
            ] {
                let mut params = pairs(&expected);
                params.push((field.into(), "untrusted".into()));
                assert!(parse_call(op, params).is_err(), "{path}: {field}");
            }
            let mut duplicate = pairs(&expected);
            duplicate.push(duplicate[0].clone());
            assert!(parse_call(op, duplicate).is_err(), "{path}: duplicate");
        }
    }

    #[test]
    fn named_types_use_shared_business_selectors() {
        for label in [
            "text", "image", "voice", "video", "sticker", "location", "link", "file", "call",
            "system",
        ] {
            for key in ["type", "msg_type"] {
                let call = parse_call(
                    "search",
                    vec![
                        ("keyword".into(), "needle".into()),
                        (key.into(), label.into()),
                    ],
                )
                .unwrap_or_else(|_| panic!("{label}"));
                assert_eq!(
                    serde_json::to_value(call).unwrap()["msg_type"],
                    json!(crate::service::message_filter::cli_type(label).unwrap())
                );
            }
        }
        let call = parse_call(
            "history",
            vec![
                ("chat".into(), "peer".into()),
                ("msg_types".into(), "text, image,voice".into()),
            ],
        )
        .unwrap_or_else(|_| panic!("multi type"));
        assert_eq!(
            serde_json::to_value(call).unwrap()["msg_types"],
            json!([1, 3, 34])
        );
        for params in [
            vec![("keyword", "q"), ("type", "text"), ("msg_type", "image")],
            vec![("keyword", "q"), ("type", "unknown")],
            vec![("keyword", "q"), ("offset", "1")],
            vec![("keyword", "q"), ("oldest_first", "true")],
            vec![("keyword", "q"), ("msg_types", "text,image")],
        ] {
            assert!(parse_call(
                "search",
                params
                    .into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect()
            )
            .is_err());
        }
        assert!(parse_call(
            "history",
            vec![
                ("chat".into(), "peer".into()),
                ("msg_type".into(), "text".into()),
                ("msg_types".into(), "image".into())
            ]
        )
        .is_err());
    }

    #[test]
    fn bounds_time_identity_and_defaults_are_enforced() {
        for (path, value) in crate::service::web::read_query_cases() {
            let op = value["op"].as_str().unwrap();
            for (key, invalid) in [
                ("limit", "0"),
                ("limit", "2001"),
                ("offset", "1000001"),
                ("since", "-1"),
                ("until", "99"),
                ("chat", ""),
                ("keyword", ""),
                ("local_id", "0"),
                ("create_time", "0"),
                ("item_index", "-1"),
                ("filter", "unknown"),
                ("msg_types", ""),
                ("chats", ""),
            ] {
                if value.get(key).is_none() {
                    continue;
                }
                let mut params = pairs(&value);
                params.retain(|(field, _)| field != key);
                params.push((key.into(), invalid.into()));
                assert!(parse_call(op, params).is_err(), "{path}: {key}={invalid}");
            }
        }
        for limit in [0, 501, 2000] {
            assert!(parse_call(
                "voice_messages",
                vec![
                    ("chat".into(), "peer".into()),
                    ("limit".into(), limit.to_string())
                ]
            )
            .is_err());
        }
        let voice = parse_call("voice_messages", vec![("chat".into(), "peer".into())])
            .unwrap_or_else(|_| panic!("voice default"));
        assert_eq!(serde_json::to_value(voice).unwrap()["limit"], 200);
        assert!(parse_call(
            "voice_messages",
            vec![
                ("chat".into(), "peer".into()),
                ("limit".into(), "500".into())
            ]
        )
        .is_ok());
        for op in [
            "decode_transfer",
            "decode_location",
            "decode_refer",
            "decode_file_message",
            "decode_record_item",
        ] {
            assert!(parse_call(
                op,
                vec![
                    ("chat".into(), "peer".into()),
                    ("local_id".into(), "71".into())
                ]
            )
            .is_err());
        }
    }

    #[test]
    fn axum_query_extraction_keeps_duplicate_keys_for_rejection() {
        let uri = "/api/search?keyword=needle&chats=wxid_a%2Cwxid_b&type=text&limit=7"
            .parse()
            .unwrap();
        let Query(input) = Query::<Vec<(String, String)>>::try_from_uri(&uri).unwrap();
        assert!(parse_call("search", input).is_ok());
        let uri = "/api/search?keyword=one&keyword=two".parse().unwrap();
        let Query(input) = Query::<Vec<(String, String)>>::try_from_uri(&uri).unwrap();
        assert!(parse_call("search", input).is_err());
    }
}

use super::{bad, query, query_error, ApiError, ApiResult, Shared};
use crate::service::web::Call;
use axum::{
    extract::{Query, State},
    Json,
};
use serde_json::{Map, Value};
use std::{collections::HashSet, sync::Arc};

type Input = Result<Query<Vec<(String, String)>>, axum::extract::rejection::QueryRejection>;

fn message_type(value: &str) -> Result<i64, ApiError> {
    crate::service::message_filter::cli_type(value)
        .or_else(|| value.parse::<i64>().ok().filter(|kind| *kind >= 0))
        .ok_or_else(bad)
}

fn parse_call(op: &str, pairs: Vec<(String, String)>) -> Result<Call, ApiError> {
    let mut data = Map::new();
    let mut seen = HashSet::new();
    data.insert("op".into(), Value::String(op.into()));
    for (key, value) in pairs {
        let key = if key == "type" {
            "msg_type".to_owned()
        } else {
            key
        };
        if !seen.insert(key.clone()) || value.len() > 32 * 1024 {
            return Err(bad());
        }
        if key == "source" {
            if value != "wechat" {
                return Err(bad());
            }
            continue;
        }
        let value = match key.as_str() {
            "msg_type" => Value::from(message_type(&value)?),
            "msg_types" => Value::Array(
                value
                    .split(',')
                    .map(|value| message_type(value.trim()).map(Value::from))
                    .collect::<Result<_, _>>()?,
            ),
            "chats" | "filter" => Value::Array(
                value
                    .split(',')
                    .map(|value| Value::String(value.trim().into()))
                    .collect(),
            ),
            "limit" | "offset" | "since" | "until" | "local_id" | "create_time" | "item_index"
            | "fav_type" => Value::from(value.parse::<i64>().map_err(|_| bad())?),
            "oldest_first" | "with_meta" | "debug_source" | "unread" | "include_read" => {
                Value::Bool(value.parse::<bool>().map_err(|_| bad())?)
            }
            "chat" | "keyword" | "query" | "account" | "user" => Value::String(value),
            _ => return Err(bad()),
        };
        data.insert(key, value);
    }
    let call: Call = serde_json::from_value(Value::Object(data)).map_err(|_| bad())?;
    call.validate_read().map_err(|_| bad())?;
    Ok(call)
}

async fn run(state: Arc<Shared>, op: &str, input: Input) -> ApiResult {
    let Query(pairs) = input.map_err(|_| bad())?;
    let call = parse_call(op, pairs)?;
    query::web(&state, call)
        .await
        .map(Json)
        .map_err(query_error)
}

macro_rules! handler {
    ($name:ident, $op:literal) => {
        pub(super) async fn $name(State(state): State<Arc<Shared>>, input: Input) -> ApiResult {
            run(state, $op, input).await
        }
    };
}

handler!(search, "search");
handler!(unread, "unread");
handler!(members, "members");
handler!(stats, "stats");
handler!(favorites, "favorites");
handler!(articles, "biz_articles");
handler!(sns_feed, "sns_feed");
handler!(sns_search, "sns_search");
handler!(sns_notifications, "sns_notifications");
handler!(voice_messages, "voice_messages");
handler!(decode_transfer, "decode_transfer");
handler!(decode_location, "decode_location");
handler!(decode_refer, "decode_refer");
handler!(decode_file_message, "decode_file_message");
handler!(decode_record_item, "decode_record_item");

pub(super) async fn history(State(state): State<Arc<Shared>>, input: Input) -> ApiResult {
    let Query(pairs) = input.map_err(|_| bad())?;
    let has_chat = pairs
        .iter()
        .any(|(key, value)| key == "chat" && !value.is_empty());
    if has_chat {
        let call = parse_call("history", pairs)?;
        return query::web(&state, call)
            .await
            .map(Json)
            .map_err(query_error);
    }
    // No conversation selected: keep the existing launch-monitor scope, never
    // silently treat database filters as monitor filters.
    if pairs.iter().any(|(key, _)| {
        !matches!(
            key.as_str(),
            "chat" | "source" | "limit" | "offset" | "since"
        )
    }) {
        return Err(bad());
    }
    let mut pairs = pairs;
    let empty_chats = pairs.iter().filter(|(key, _)| key == "chat").count();
    if empty_chats > 1 {
        return Err(bad());
    }
    pairs.retain(|(key, _)| key != "chat");
    pairs.push(("chat".into(), "launch-monitor".into()));
    let Call::History {
        limit,
        offset,
        since,
        ..
    } = parse_call("history", pairs)?
    else {
        return Err(bad());
    };
    let session = state.records.lock().unwrap().monitor_session.clone();
    let Some(session) = session else {
        return Ok(Json(
            serde_json::json!({"messages":[],"scope":"launch_monitor"}),
        ));
    };
    query::web(
        &state,
        Call::MonitorHistory {
            session,
            limit,
            offset,
            since,
        },
    )
    .await
    .map(Json)
    .map_err(query_error)
}
