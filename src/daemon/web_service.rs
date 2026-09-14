//! 浏览器业务操作由 daemon 持有；HTTP 层仅负责传输与鉴权。
#[path = "web_service/automatic_image.rs"]
mod automatic_image;
#[path = "web_service/preview.rs"]
mod preview;
#[path = "web_service/query.rs"]
mod query;
#[cfg(test)]
#[path = "web_service/tests.rs"]
mod tests;

#[cfg(test)]
use crate::ipc::Response;
use crate::{
    ipc::Request,
    runtime::RuntimeContext,
    service::web::{Call, Image},
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{watch, Semaphore};

const MAX_BUFFER_BYTES: usize = 8 * 1024 * 1024;

#[cfg(test)]
type QueryFixture = tokio::sync::mpsc::Sender<(Request, tokio::sync::oneshot::Sender<Response>)>;

#[derive(Default)]
struct Records {
    cursor: u64,
    next_session: u64,
    sessions: VecDeque<(String, u64)>,
    events: VecDeque<(u64, &'static str, Value, usize)>,
    event_bytes: usize,
    messages: VecDeque<(u64, Value, usize)>,
    message_bytes: usize,
}

pub struct WebService {
    runtime: RuntimeContext,
    epoch: String,
    query: Arc<super::query_state::QueryState>,
    queries: Arc<Semaphore>,
    decodes: Arc<Semaphore>,
    calls: Arc<Semaphore>,
    shutdown: watch::Sender<bool>,
    records: Mutex<Records>,
    monitor: Mutex<Option<tokio::task::JoinHandle<()>>>,
    #[cfg(test)]
    pub(crate) query_fixture: Mutex<Option<QueryFixture>>,
}

impl WebService {
    pub fn new(runtime: RuntimeContext, query: Arc<super::query_state::QueryState>) -> Arc<Self> {
        let (shutdown, _) = watch::channel(false);
        Arc::new(Self {
            runtime,
            query,
            epoch: format!(
                "{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ),
            queries: Arc::new(Semaphore::new(4)),
            decodes: Arc::new(Semaphore::new(2)),
            calls: Arc::new(Semaphore::new(32)),
            shutdown,
            records: Mutex::new(Records::default()),
            monitor: Mutex::new(None),
            #[cfg(test)]
            query_fixture: Mutex::new(None),
        })
    }

    // 仅由 daemon 关闭时调用，此时 Tokio runtime 与查询服务仍存活。
    // Web 断连或关闭不得调用此方法。
    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown.send_replace(true);
        let monitor = self.monitor.lock().unwrap().take();
        if let Some(mut monitor) = monitor {
            if tokio::time::timeout(Duration::from_secs(25), &mut monitor)
                .await
                .is_err()
            {
                monitor.abort();
                let _ = monitor.await;
            }
        }
        let calls_drained =
            tokio::time::timeout(Duration::from_secs(60), self.calls.acquire_many(32))
                .await
                .is_ok_and(|result| result.is_ok());
        // 即使其他业务请求排空超时，仍尝试清理图片任务。
        let images_drained = automatic_image::drain(self).await;
        let reads_drained =
            tokio::time::timeout(Duration::from_secs(30), self.queries.acquire_many(4))
                .await
                .is_ok_and(|result| result.is_ok());
        ensure!(
            calls_drained && images_drained && reads_drained,
            "Web business shutdown drain failed"
        );
        Ok(())
    }

    pub async fn handle(
        self: &Arc<Self>,
        call: Call,
    ) -> std::result::Result<Value, crate::service::protocol::ServiceError> {
        let _permit = self.calls.clone().try_acquire_owned().map_err(|_| {
            crate::service::protocol::ServiceError::new("busy", "Web business service is busy")
        })?;
        if *self.shutdown.borrow() {
            return Err(crate::service::protocol::ServiceError::new(
                "stopping",
                "Web business service is stopping",
            ));
        }
        self.execute(call).await.map_err(|error| {
            if let Some(failure) = error.downcast_ref::<crate::ipc::outcome::BusinessFailure>() {
                crate::service::protocol::ServiceError::new(
                    failure.service_code(),
                    failure.public_message(),
                )
            } else if query::is_busy(&error) {
                crate::service::protocol::ServiceError::new("busy", "Web business service is busy")
            } else {
                crate::service::protocol::ServiceError::new(
                    "unavailable",
                    "Web business operation unavailable",
                )
            }
        })
    }

    async fn execute(self: &Arc<Self>, call: Call) -> Result<Value> {
        match call {
            Call::DecodeImage { encoded, source } => Ok(
                match automatic_image::decode(self.clone(), encoded, source).await {
                    Ok(image) => json!({"image": Image::new(image.bytes, image.content_type)?}),
                    Err(failure) => json!({"failure": failure}),
                },
            ),
            Call::PreviewImage { encoded } => {
                let id = crate::service::web::identity(&encoded)?;
                let image = preview::read(self.clone(), encoded, id).await?;
                let image = image
                    .map(|image| Image::new(image.bytes, image.content_type))
                    .transpose()?;
                Ok(json!({"image": image}))
            }
            Call::Images {
                chat,
                limit,
                offset,
                since,
            } => {
                validate_page(&chat, limit, offset, 1000)?;
                preview::list(self, chat, limit, offset, since).await
            }
            Call::History {
                chat,
                limit,
                offset,
                since,
            } => {
                validate_page(&chat, limit, offset, 2000)?;
                query::request(
                    self,
                    Request::History {
                        chat,
                        limit,
                        offset,
                        since,
                        until: None,
                        msg_type: None,
                        msg_types: None,
                        oldest_first: false,
                        with_meta: false,
                        debug_source: false,
                    },
                )
                .await
            }
            Call::Tags { name } => {
                ensure!(
                    name.as_ref().is_none_or(|name| valid_text(name)),
                    "invalid tag name"
                );
                let mut data = query::request(self, Request::ContactTags).await?;
                if let (Some(name), Some(tags)) = (name, data["tags"].as_array_mut()) {
                    let name = name.to_lowercase();
                    tags.retain(|tag| {
                        tag["name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_lowercase()
                            .contains(&name)
                    });
                }
                Ok(data)
            }
            Call::MonitorOpen {} => {
                let mut monitor = self.monitor.lock().unwrap();
                ensure!(!*self.shutdown.borrow(), "service stopping");
                let (cursor, session) = {
                    let mut records = self.records.lock().unwrap();
                    let cursor = records.cursor;
                    records.next_session = records
                        .next_session
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("monitor session overflow"))?;
                    let session = format!("{}-{}", self.epoch, records.next_session);
                    if records.sessions.len() >= 32 {
                        records.sessions.pop_front();
                    }
                    records.sessions.push_back((session.clone(), cursor));
                    (cursor, session)
                };
                if monitor.is_none() {
                    *monitor = Some(tokio::spawn(query::monitor(self.clone())));
                }
                Ok(
                    json!({"cursor": cursor, "runtime_id": self.runtime.id, "epoch": self.epoch, "session": session}),
                )
            }
            Call::MonitorEvents { epoch, after } => {
                ensure!(epoch == self.epoch, "monitor daemon changed");
                Ok(self.events(after))
            }
            Call::MonitorHistory {
                session,
                limit,
                offset,
                since,
            } => {
                ensure!(
                    (1..=2000).contains(&limit) && offset <= 1_000_000,
                    "invalid pagination"
                );
                let records = self.records.lock().unwrap();
                let after = records
                    .sessions
                    .iter()
                    .find(|(id, _)| id == &session)
                    .map(|(_, cursor)| *cursor)
                    .ok_or_else(|| anyhow::anyhow!("monitor session unavailable"))?;
                let messages: Vec<_> = records
                    .messages
                    .iter()
                    .rev()
                    .filter(|(seq, message, _)| {
                        *seq > after
                            && since.is_none_or(|since| {
                                message["timestamp"].as_i64().is_some_and(|ts| ts > since)
                            })
                    })
                    .skip(offset)
                    .take(limit)
                    .map(|(_, message, _)| message.clone())
                    .collect();
                Ok(json!({"messages": messages, "scope": "launch_monitor"}))
            }
        }
    }

    fn event(&self, name: &'static str, data: Value) {
        let size = data.to_string().len();
        if size > MAX_BUFFER_BYTES {
            // 不保留超大批次，也不据此推进消息历史。
            self.event(
                "monitor_status",
                json!({"status": "error", "reason": "message_too_large"}),
            );
            return;
        }
        let mut records = self.records.lock().unwrap();
        let Some(seq) = records.cursor.checked_add(1) else {
            return;
        };
        records.cursor = seq;
        while records.events.len() >= 256 || records.event_bytes + size > MAX_BUFFER_BYTES {
            if let Some((_, _, _, old_size)) = records.events.pop_front() {
                records.event_bytes -= old_size;
            } else {
                break;
            }
        }
        records.event_bytes += size;
        records.events.push_back((seq, name, data.clone(), size));
        if name == "message" {
            let mut history = data;
            history
                .as_object_mut()
                .map(|row| row.remove("web_delivery"));
            let size = history.to_string().len();
            while records.messages.len() >= 2000 || records.message_bytes + size > MAX_BUFFER_BYTES
            {
                if let Some((_, _, old_size)) = records.messages.pop_front() {
                    records.message_bytes -= old_size;
                } else {
                    break;
                }
            }
            records.message_bytes += size;
            records.messages.push_back((seq, history, size));
        }
    }

    fn events(&self, after: u64) -> Value {
        let records = self.records.lock().unwrap();
        let reset = after > records.cursor
            || records
                .events
                .front()
                .is_some_and(|(seq, _, _, _)| after < seq.saturating_sub(1));
        let events: Vec<_> = if reset {
            Vec::new()
        } else {
            records
                .events
                .iter()
                .filter(|(seq, _, _, _)| *seq > after)
                .take(128)
                .map(|(seq, name, data, _)| json!({"seq":seq,"name":name,"data":data}))
                .collect()
        };
        let cursor = events
            .last()
            .and_then(|event| event["seq"].as_u64())
            .unwrap_or(records.cursor);
        json!({"events": events, "cursor": cursor, "reset": reset, "runtime_id": self.runtime.id})
    }
}

fn valid_text(text: &str) -> bool {
    text.len() <= 256 && !text.chars().any(char::is_control)
}
fn validate_page(chat: &str, limit: usize, offset: usize, maximum: usize) -> Result<()> {
    ensure!(
        !chat.is_empty()
            && valid_text(chat)
            && (1..=maximum).contains(&limit)
            && offset <= 1_000_000,
        "invalid query parameters"
    );
    Ok(())
}
