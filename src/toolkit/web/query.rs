//! Fixed-account queries and monitoring; daemon startup is detached and shared with CLI.
use super::server_types::Shared;
use crate::ipc::{Request, Response};
use crate::toolkit::enterprise::queries::{Contact, Conversation};
use crate::toolkit::monitor::{
    self as incremental, Cancellation, FixedRuntimeContext, MonitorOptions,
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Debug)]
struct QueryBusy;
impl std::fmt::Display for QueryBusy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("查询繁忙")
    }
}
impl std::error::Error for QueryBusy {}

pub(super) fn is_busy(error: &anyhow::Error) -> bool {
    error.is::<QueryBusy>()
}

pub async fn request(state: &Shared, request: Request) -> Result<Value> {
    let decorate_history = matches!(&request, Request::History { .. });
    let _permit = state
        .queries
        .clone()
        .try_acquire_owned()
        .map_err(|_| QueryBusy)?;
    let timeout = if matches!(&request, Request::Ping) {
        1
    } else {
        20
    };
    let result = tokio::time::timeout(Duration::from_secs(timeout), async {
        use interprocess::local_socket::{tokio::prelude::*, GenericNamespaced};
        let pipe = state.runtime.pipe_name();
        let name = pipe.to_ns_name::<GenericNamespaced>()?;
        let mut stream = interprocess::local_socket::tokio::Stream::connect(name).await?;
        stream
            .write_all((serde_json::to_string(&request)? + "\n").as_bytes())
            .await?;
        let mut reader = BufReader::new(stream).take(8 * 1024 * 1024 + 1);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        ensure!(line.len() <= 8 * 1024 * 1024, "查询响应超过限额");
        let response: Response = serde_json::from_str(&line)?;
        ensure!(response.ok, "后台查询未完成");
        let mut data = response.data;
        if decorate_history {
            if let Some(chat) = data["username"].as_str().map(str::to_owned) {
                if let Some(messages) = data["messages"].as_array_mut() {
                    for message in messages {
                        decorate_image(&chat, message);
                    }
                }
            }
        }
        Ok::<_, anyhow::Error>(data)
    })
    .await?;
    result
}

pub async fn monitor(state: Arc<Shared>) {
    let context = match FixedRuntimeContext::from_runtime(state.runtime.clone()) {
        Ok(context) => context,
        Err(_) => {
            state.event("monitor_status", json!({"status":"error"}));
            return;
        }
    };
    // 为连续监控保留一个查询名额，其余名额仍供交互查询使用。
    let Ok(_permit) = state.queries.clone().acquire_owned().await else {
        return;
    };
    let cancel = Cancellation::new();
    let options = MonitorOptions {
        interval: Duration::from_secs(2),
        with_meta: true,
        ..Default::default()
    };
    let mut sequence = 0_u64;
    let mut sink = |event: incremental::Event| -> Result<()> {
        let status = match event.kind {
            "cursor_held" => Some(if event.data["limit_ceiling_reached"] == true {
                "limit_reached"
            } else {
                "catching_up"
            }),
            "cycle_error" => Some("error"),
            "baseline" | "messages" | "heartbeat" => Some("ready"),
            _ => None,
        };
        // 不把临时批次放入缓冲；扩容重试会重新读取它们，接受后只发布一次。
        for message in committed_messages(&event) {
            let mut message = message.clone();
            if let Some(chat) = message["username"].as_str().map(str::to_owned) {
                decorate_image(&chat, &mut message);
            }
            let size = message.to_string().len();
            let mut records = state.records.lock().unwrap();
            while records.messages.len() >= 2000 || records.message_bytes + size > 8 * 1024 * 1024 {
                if let Some(old) = records.messages.pop_front() {
                    records.message_bytes -= old.to_string().len();
                } else {
                    break;
                }
            }
            records.messages.push_back(message.clone());
            records.message_bytes += size;
            drop(records);
            sequence += 1;
            state.event(
                "message",
                live_message(&message, &state.runtime.id, sequence),
            );
        }
        if let Some(status) = status {
            state.event("monitor_status", json!({"status":status,"coverage_proven":false,"cursor_boundary":incremental::CURSOR_WARNING}));
        }
        Ok(())
    };
    let run = async {
        if incremental::run_monitor(&context, &options, &cancel, &mut sink)
            .await
            .is_err()
        {
            state.event("monitor_status", json!({"status":"error"}));
        }
        cancel.cancel();
    };
    let lifecycle = monitor_lifecycle(&state, &cancel);
    tokio::join!(run, lifecycle);
}

fn decorate_image(chat: &str, message: &mut Value) {
    if let Some(descriptor) = super::automatic_image::descriptor(chat, message) {
        message["image"] = descriptor;
    }
}

fn live_message(message: &Value, runtime_id: &str, sequence: u64) -> Value {
    let mut live = message.clone();
    live["web_delivery"] =
        json!({"runtime_id":runtime_id,"sequence":sequence,"committed":true,"replay":false});
    live
}

fn committed_messages(event: &incremental::Event) -> &[Value] {
    if event.kind == "messages" && event.data["cursor_committed"] == true {
        event.data["messages"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
    } else {
        &[]
    }
}

// The permit lives in the blocking closure even when its async caller times out.
static STARTUPS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(1)));

pub(super) async fn ensure_detached(runtime: crate::runtime::RuntimeContext) -> Result<()> {
    let permit = STARTUPS.clone().try_acquire_owned()?;
    tokio::time::timeout(
        Duration::from_secs(25),
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            crate::cli::transport::ensure_running(&runtime)
        }),
    )
    .await??
}

async fn monitor_lifecycle(state: &Arc<Shared>, cancel: &Cancellation) {
    let mut stop = state.shutdown.subscribe();
    if !*stop.borrow() {
        tokio::select! {
            _ = stop.changed() => {},
            _ = cancel.cancelled() => {},
        }
    }
    cancel.cancel();
}

fn enterprise_session(conversation: &Conversation) -> Value {
    json!({"username":conversation.conversation_id,"name":conversation.display_name,"type":conversation.kind,
        "last_ts":conversation.last_time,"message_count":conversation.message_count})
}

fn enterprise_contact(
    contact: &Contact,
    conversations: &[Conversation],
    self_id: Option<i64>,
) -> Value {
    // 只解析快照中真实存在的单聊参与者，不拼接会话 ID，也不按姓名猜测。
    let matches: Vec<_> = conversations
        .iter()
        .filter(|conversation| {
            let Some(tail) = conversation.conversation_id.strip_prefix("S:") else {
                return false;
            };
            let Ok(ids) = tail
                .split('_')
                .map(str::parse::<i64>)
                .collect::<std::result::Result<Vec<_>, _>>()
            else {
                return false;
            };
            !ids.is_empty()
                && ids.contains(&contact.id)
                && self_id.is_none_or(|own| {
                    ids.contains(&own) && (contact.id != own || ids.iter().all(|id| *id == own))
                })
        })
        .map(enterprise_session)
        .collect();
    json!({"username":contact.id.to_string(),"name":contact.display_name,"contact_id":contact.id.to_string(),"conversations":matches})
}

pub fn contacts(mut value: Value) -> Value {
    if let Some(rows) = value.get_mut("contacts").and_then(Value::as_array_mut) {
        for row in rows {
            row["name"] = row["display"].clone();
        }
    }
    value
}

pub fn sessions(mut value: Value) -> Value {
    if let Some(rows) = value.get_mut("sessions").and_then(Value::as_array_mut) {
        for row in rows {
            row["name"] = row["chat"].clone();
            row["last_ts"] = row["timestamp"].clone();
            row["type"] = row["chat_type"].clone();
        }
    }
    value
}

pub async fn enterprise(
    state: Arc<Shared>,
    operation: &'static str,
    chat: Option<String>,
    limit: usize,
    offset: usize,
    since: Option<i64>,
) -> Result<Value> {
    let settings = state.effective_settings();
    let snapshot = settings
        .enterprise_snapshot
        .clone()
        .ok_or_else(|| anyhow::anyhow!("未配置企业微信快照"))?;
    let permit = state
        .queries
        .clone()
        .try_acquire_owned()
        .map_err(|_| anyhow::anyhow!("查询繁忙"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        use crate::toolkit::enterprise::queries::{OfflineStore, MessageFilter};
        let store = OfflineStore::open(&snapshot, settings.enterprise_self_id)?;
        Ok(match operation {
            "contacts" => {
                let conversations = store.conversations()?;
                json!({"contacts":store.contacts()?.iter().skip(offset).take(limit)
                    .map(|c| enterprise_contact(c, &conversations, settings.enterprise_self_id)).collect::<Vec<_>>()})
            },
            "sessions" => json!({"sessions":store.conversations()?.into_iter().skip(offset).take(limit).map(|c|
                enterprise_session(&c)).collect::<Vec<_>>()}),
            "history" => json!({"messages":store.messages(&MessageFilter {conversation_ids:chat.into_iter().collect(),
                start_time:since,offset,limit:Some(limit),..Default::default()})?}),
            _ => anyhow::bail!("不支持此企业微信查询"),
        })
    }).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_error_classification_preserves_nonbusy_errors() {
        assert!(is_busy(&QueryBusy.into()));
        assert!(is_busy(&anyhow::Error::new(QueryBusy).context("query")));
        assert!(!is_busy(&anyhow::anyhow!("查询繁忙")));
        assert!(!is_busy(&anyhow::anyhow!("synthetic disconnected pipe")));
    }

    fn conversation(id: &str) -> Conversation {
        Conversation {
            conversation_id: id.into(),
            display_name: "同名联系人".into(),
            kind: "单聊".into(),
            message_count: 1,
            last_time: 10,
            last_message_id: 1,
        }
    }

    #[test]
    fn contact_only_resolves_existing_exact_participants() {
        let contact = Contact {
            id: 12,
            display_name: "同名联系人".into(),
        };
        let conversations = [
            "S:1_12",
            "S:12_1",
            "S:1_112",
            "R:12",
            "12",
            "S:2_12",
            "S:1_12_bad",
        ]
        .map(conversation);
        let row = enterprise_contact(&contact, &conversations, Some(1));
        assert_eq!(row["username"], "12");
        assert_eq!(row["conversations"].as_array().unwrap().len(), 2);
        assert_eq!(row["conversations"][0]["username"], "S:1_12");
        assert_eq!(row["conversations"][1]["username"], "S:12_1");
    }

    #[test]
    fn missing_contact_and_own_contact_do_not_invent_chats() {
        let conversations = [conversation("S:1_12")];
        for id in [1, 99] {
            assert_eq!(
                enterprise_contact(
                    &Contact {
                        id,
                        display_name: "测试".into()
                    },
                    &conversations,
                    Some(1)
                )["conversations"],
                json!([])
            );
        }
        assert_eq!(
            enterprise_contact(
                &Contact {
                    id: 12,
                    display_name: "测试".into()
                },
                &conversations,
                None
            )["conversations"][0]["username"],
            "S:1_12"
        );
    }

    #[test]
    fn provisional_and_failed_monitor_batches_never_reach_web_history() {
        let mut event = incremental::Event {
            kind: "cursor_held",
            runtime_id: "synthetic".into(),
            cycle: 1,
            observed_at: String::new(),
            data: json!({"messages":[{"local_id":1}],"cursor_committed":false}),
        };
        assert!(committed_messages(&event).is_empty());
        event.kind = "messages";
        assert!(committed_messages(&event).is_empty());
        event.data["cursor_committed"] = json!(true);
        assert_eq!(committed_messages(&event).len(), 1);
        for kind in ["cycle_error", "baseline", "cursor_held"] {
            event.kind = kind;
            assert!(committed_messages(&event).is_empty());
        }
    }

    #[test]
    fn delivery_marker_keeps_message_identity_and_does_not_mark_history_live() {
        let message =
            json!({"username":"synthetic","local_id":7,"timestamp":123,"source":"message_0.db"});
        let live = live_message(&message, "account", 42);
        for field in ["username", "local_id", "timestamp", "source"] {
            assert_eq!(live[field], message[field]);
        }
        assert_eq!(live["web_delivery"]["sequence"], 42);
        assert_eq!(live["web_delivery"]["replay"], false);
        assert!(message.get("web_delivery").is_none());
    }

    #[tokio::test]
    async fn synthetic_monitor_reuses_baseline_hold_expand_and_ceiling() {
        use crate::{config::Config, runtime::RuntimeContext};
        use interprocess::local_socket::{tokio::prelude::*, GenericNamespaced, ListenerOptions};
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            db_dir: root.path().join("db"),
            keys_file: root.path().join("keys.json"),
            decrypted_dir: root.path().join("plain"),
            wechat_process: String::new(),
        };
        let runtime = RuntimeContext::from_config(
            root.path().join("config.json"),
            config,
            root.path().to_owned(),
        )
        .unwrap();
        let name = runtime.pipe_name();
        let listener = ListenerOptions::new()
            .name(name.to_ns_name::<GenericNamespaced>().unwrap())
            .create_tokio()
            .unwrap();
        let context = FixedRuntimeContext::from_runtime(runtime).unwrap();
        let now = chrono::Utc::now().timestamp();
        let message = |id| json!({"username":"alice","timestamp":now+1,"local_id":id});
        let base = json!({"alice":now});
        let next = json!({"alice":now+1,"new_empty":now+2});
        let good = json!({"status":"ok","unknown_shards":[]});
        let bad = json!({"status":"partial","unknown_shards":["synthetic"]});
        // IPC 响应为扁平 JSON；用真实类型生成，避免夹具多包一层 data。
        let batch = |messages: Value, cursor: Value, meta: Value| {
            serde_json::to_value(Response::ok(json!({
            "count":messages.as_array().unwrap().len(),"messages":messages,"new_state":cursor,"meta":meta
        }))).unwrap()
        };
        let replies = vec![
            batch(json!([]), base.clone(), good.clone()),
            batch(json!([message(1), message(2)]), next.clone(), good.clone()),
            batch(json!([message(1), message(2)]), next.clone(), bad),
            batch(
                json!([message(1), message(2), message(3)]),
                next.clone(),
                good.clone(),
            ),
            batch(
                json!([message(4), message(5), message(6), message(7)]),
                next.clone(),
                good.clone(),
            ),
            batch(
                json!([message(4), message(5), message(6), message(7)]),
                next,
                good.clone(),
            ),
        ];
        let serve = async {
            let mut seen = Vec::new();
            for reply in replies {
                let stream = listener.accept().await.unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                seen.push(serde_json::from_str::<Value>(&line).unwrap());
                reader
                    .get_mut()
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            }
            seen
        };
        let options = MonitorOptions {
            interval: Duration::from_millis(100),
            limit: 2,
            max_limit: 4,
            max_cycles: Some(6),
            max_duration: Some(Duration::from_secs(5)),
            ..Default::default()
        };
        let cancel = Cancellation::new();
        let mut events = Vec::new();
        let mut sink = |event| {
            events.push(event);
            Ok(())
        };
        let (seen, summary) = tokio::time::timeout(Duration::from_secs(8), async {
            tokio::join!(
                serve,
                incremental::run_monitor(&context, &options, &cancel, &mut sink)
            )
        })
        .await
        .unwrap();
        let summary = summary.unwrap();
        assert_eq!(
            summary.error_cycles,
            0,
            "合成协议错误：{}",
            serde_json::to_string(&events).unwrap()
        );
        assert_eq!(summary.held_cycles, 4);
        assert_eq!(seen[0]["limit"], 0);
        assert!(seen[0]["state"].is_null());
        assert!(seen.iter().all(|request| request["with_meta"] == true));
        assert_eq!(seen[1]["limit"], 2);
        for request in &seen[2..4] {
            assert_eq!(request["limit"], 4);
            assert_eq!(request["state"], base);
        }
        assert_eq!(seen[4]["state"], seen[5]["state"]);
        assert!(seen[4]["state"]["new_empty"].as_i64().unwrap() <= now - 86400 + 5);
        let published: Vec<_> = events
            .iter()
            .flat_map(committed_messages)
            .map(|m| m["local_id"].clone())
            .collect();
        assert_eq!(published, vec![json!(1), json!(2), json!(3)]);
        assert_eq!(
            events
                .iter()
                .filter(|e| e.kind == "cursor_held" && e.data["limit_ceiling_reached"] == true)
                .count(),
            2
        );
    }
}
