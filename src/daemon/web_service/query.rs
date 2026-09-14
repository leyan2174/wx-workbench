//! 固定账号的查询与监控；有限等待查询许可，不重放已经发出的请求。
use super::WebService as Shared;
use crate::ipc::Request;
#[cfg(test)]
use crate::ipc::Response;
use crate::toolkit::monitor::{
    self as incremental, Cancellation, FixedRuntimeContext, MonitorOptions,
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
#[cfg(test)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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
    // 调用受 32 个 calls 许可约束；取消后继续的图片任务另受 2 个 decodes 许可约束。
    // 加上单实例监控，等待人数有界；超时或取消只撤销排队，不发送或重放请求。
    let _permit = tokio::time::timeout(
        Duration::from_secs(2),
        state.queries.clone().acquire_owned(),
    )
    .await
    .map_err(|_| QueryBusy)?
    .map_err(|_| QueryBusy)?;
    // 仅内部数据库操作持有代际租约；图片准备、发布与监控轮询不持有该租约。
    #[cfg(test)]
    let fixture = state.query_fixture.lock().unwrap().clone();
    #[cfg(test)]
    let response = if let Some(fixture) = fixture {
        let (reply, receive) = tokio::sync::oneshot::channel();
        fixture
            .send((request, reply))
            .await
            .map_err(|_| anyhow::anyhow!("fixture closed"))?;
        receive.await?
    } else {
        super::super::server::dispatch_state(request, &state.query).await
    };
    #[cfg(not(test))]
    let response = super::super::server::dispatch_state(request, &state.query).await;
    ensure!(response.ok, "background query failed");
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
    Ok(data)
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
