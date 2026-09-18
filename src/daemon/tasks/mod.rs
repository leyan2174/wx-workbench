//! Account-scoped task ownership. Frontends submit typed capabilities, never commands.
mod artifact_file;
pub(crate) mod artifacts;
pub(crate) mod history_artifacts;
pub(crate) mod plan_artifacts;
pub(crate) mod process;
mod store;
#[cfg(test)]
mod tests;
mod worker;

use crate::{
    runtime::RuntimeContext,
    service::{
        config_pin::ConfigPin,
        plan,
        protocol::*,
        settings::{self, Settings},
    },
};
use anyhow::Result;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, watch, Notify, Semaphore};

pub const HISTORY_LIMIT: usize = 100;
pub const LOG_LIMIT: usize = 256;
pub const LINE_LIMIT: usize = 512;
pub const QUEUE_LIMIT: usize = 8;
const EVENT_LIMIT: usize = 256;
const EVENT_BYTES: usize = 4 * 1024 * 1024;

pub struct Service {
    pub(super) runtime: RuntimeContext,
    pub(super) query: Arc<super::query_state::QueryState>,
    pub(super) keys: Arc<super::worker_keys::Broker>,
    pub(super) records: Mutex<Records>,
    queue: mpsc::Sender<Work>,
    pub(super) shutdown: watch::Sender<bool>,
    changed: Notify,
    pub(super) redactor: Mutex<store::Redactor>,
    artifact_reads: Arc<Semaphore>,
}

pub(super) struct Binding {
    settings: Settings,
    fingerprint: String,
}

pub(super) struct Records {
    tasks: VecDeque<Task>,
    requests: HashMap<String, String>,
    cancels: HashMap<String, watch::Sender<bool>>,
    binding: Option<Binding>,
    journal_ok: bool,
    events: VecDeque<Event>,
    event_bytes: usize,
    next_event: u64,
}

pub(crate) struct Work {
    id: String,
    request: Submission,
    settings: Settings,
    fingerprint: String,
    cancel: watch::Receiver<bool>,
    plan_selection: Option<String>,
}

fn failure(code: &str, message: &str) -> ServiceError {
    ServiceError {
        code: code.into(),
        message: message.into(),
    }
}

pub(super) fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(super) use crate::service::protocol::valid_task_id as valid_id;

impl Service {
    pub fn new(
        runtime: RuntimeContext,
        query: Arc<super::query_state::QueryState>,
        keys: Arc<super::worker_keys::Broker>,
    ) -> Result<(Arc<Self>, mpsc::Receiver<Work>)> {
        let redactor = store::Redactor::new(&runtime, None)?;
        let (tasks, requests) = store::restore(&runtime, &redactor)?;
        let (queue, receiver) = mpsc::channel(QUEUE_LIMIT);
        let (shutdown, _) = watch::channel(false);
        let state = Arc::new(Self {
            runtime,
            query,
            keys,
            queue,
            shutdown,
            changed: Notify::new(),
            redactor: Mutex::new(redactor),
            artifact_reads: Arc::new(Semaphore::new(2)),
            records: Mutex::new(Records {
                tasks,
                requests,
                cancels: HashMap::new(),
                binding: None,
                journal_ok: true,
                events: VecDeque::new(),
                event_bytes: 0,
                next_event: (now().max(1) as u64).saturating_mul(1000),
            }),
        });
        state.persist(&mut state.records.lock().unwrap())?;
        Ok((state, receiver))
    }

    pub fn subscribe_shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    pub fn request_shutdown(&self) {
        let _ = self.shutdown.send(true);
        self.changed.notify_waiters();
    }

    pub async fn run_worker(self: Arc<Self>, receiver: mpsc::Receiver<Work>) {
        worker::run(self, receiver).await;
    }

    pub(crate) async fn refresh_configuration(&self) {
        self.query.invalidate_keys().await;
        if self.refresh_redactor().await.is_err() {
            self.request_shutdown();
        }
    }

    pub(crate) async fn refresh_redactor(&self) -> Result<()> {
        // Missing material must not prevent an authorized initialization task.
        let snapshot = self.query.key_snapshot().await.ok();
        let redactor = store::Redactor::new(
            &self.runtime,
            snapshot.as_ref().map(|lease| lease.key_material()),
        )?;
        *self.redactor.lock().unwrap() = redactor;
        Ok(())
    }

    fn info(&self, records: &Records) -> Value {
        json!({"version":VERSION,"runtime_id":self.runtime.id,
            "configured":records.binding.is_some(),
            "settings":records.binding.as_ref().map(|binding| &binding.settings),
            "config_fingerprint":records.binding.as_ref().map(|binding| &binding.fingerprint),
            "history_persisted":records.journal_ok,
            "capabilities":{"task_artifacts_v1":true,"chat_plan_v1":true},
            "task_kinds":plan::capabilities(),
            "running":records.tasks.iter().filter(|task| !task.terminal()).count(),
            "cursor":records.next_event - 1,
            "limits":{"queue":QUEUE_LIMIT,"history":HISTORY_LIMIT,"logs_per_task":LOG_LIMIT}})
    }

    fn effective_settings(records: &Records) -> Option<Settings> {
        Some(records.binding.as_ref()?.settings.clone())
    }

    pub async fn dispatch(
        self: &Arc<Self>,
        call: Call,
    ) -> std::result::Result<Value, ServiceError> {
        if *self.shutdown.borrow()
            && !matches!(
                &call,
                Call::Info {}
                    | Call::List {}
                    | Call::Get { .. }
                    | Call::Shutdown {}
                    | Call::TaskArtifacts { .. }
                    | Call::ReadTaskArtifact { .. }
                    | Call::ReadChatPlan { .. }
            )
        {
            return Err(failure("stopping", "后台正在关闭，不接受新操作"));
        }
        match call {
            Call::OperationStart { .. }
            | Call::OperationPoll { .. }
            | Call::OperationCancel { .. }
            | Call::WorkerKeys { .. }
            | Call::WorkerKeyRevision { .. }
            | Call::WorkerDatabaseKeys { .. }
            | Call::WorkerImageMaterial { .. }
            | Call::Mcp { .. }
            | Call::Monitor { .. }
            | Call::Web { .. } => Err(failure(
                "invalid_operation",
                "操作请求须由 daemon 操作服务处理",
            )),
            Call::Info {} => Ok(self.info(&self.records.lock().unwrap())),
            Call::Configure { settings: input } => {
                let pin = ConfigPin::new(&self.runtime)
                    .map_err(|_| failure("configuration_changed", "无法固定选中账号配置"))?;
                let fingerprint = pin
                    .fingerprint()
                    .map_err(|_| failure("configuration_changed", "无法核验选中账号配置"))?;
                let settings = settings::load(&self.runtime, &input)
                    .map_err(|_| failure("invalid_settings", "任务服务启动设置无效"))?;
                let mut records = self.records.lock().unwrap();
                if let Some(binding) = &records.binding {
                    if binding.settings != settings || binding.fingerprint != fingerprint {
                        return Err(failure(
                            "settings_conflict",
                            "后台已绑定不同设置；请先停止后台，再使用所需设置启动",
                        ));
                    }
                } else {
                    records.binding = Some(Binding {
                        settings,
                        fingerprint,
                    });
                }
                Ok(self.info(&records))
            }
            Call::Submit {
                idempotency_key,
                task,
            } => self.submit_checked(idempotency_key, *task).await,
            Call::List {} => {
                let records = self.records.lock().unwrap();
                let tasks: Vec<_> = records.tasks.iter().rev().map(summary).collect();
                Ok(json!({"tasks":tasks,"cursor":records.next_event-1,
                    "history_persisted":records.journal_ok}))
            }
            Call::Get { id } => {
                if !valid_id(&id) {
                    return Err(failure("invalid_id", "任务 ID 无效"));
                }
                let records = self.records.lock().unwrap();
                let task = records
                    .tasks
                    .iter()
                    .find(|task| task.id == id)
                    .ok_or_else(|| failure("not_found", "当前账号没有此任务"))?;
                serde_json::to_value(task).map_err(|_| failure("serialization", "任务响应不可用"))
            }
            Call::Cancel { id } => self.cancel(&id),
            call @ (Call::TaskArtifacts { .. }
            | Call::ReadTaskArtifact { .. }
            | Call::ReadChatPlan { .. }) => self.artifact_call(call).await,
            Call::Events {
                after,
                limit,
                wait_ms,
            } => {
                if !(1..=128).contains(&limit) || wait_ms > 2000 {
                    return Err(failure("invalid_events", "事件读取限额无效"));
                }
                let notified = self.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let current = self.events(after, limit);
                if current.events.is_empty()
                    && !current.reset
                    && wait_ms != 0
                    && !*self.shutdown.borrow()
                {
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_millis(wait_ms), notified)
                            .await;
                }
                serde_json::to_value(self.events(after, limit))
                    .map_err(|_| failure("serialization", "事件响应不可用"))
            }
            Call::Shutdown {} => {
                self.request_shutdown();
                Ok(json!({"stopping":true}))
            }
        }
    }

    async fn artifact_call(
        self: &Arc<Self>,
        call: Call,
    ) -> std::result::Result<Value, ServiceError> {
        let id = match &call {
            Call::ReadChatPlan {
                plan_ref, limit, ..
            } => {
                if !(1..=100).contains(limit) {
                    return Err(artifact_file::error("invalid_page"));
                }
                plan_ref
                    .validate()
                    .map_err(|_| artifact_file::error("plan_ref_unavailable"))?;
                &plan_ref.task_id
            }
            Call::TaskArtifacts { id, limit, .. }
                if (1..=crate::service::task_artifacts::MAX_LIST_ITEMS).contains(limit) =>
            {
                id
            }
            Call::ReadTaskArtifact {
                id,
                artifact_id,
                max_bytes,
                ..
            } if valid_id(artifact_id)
                && (1..=crate::service::task_artifacts::CHUNK_BYTES).contains(max_bytes) =>
            {
                id
            }
            _ => return Err(artifact_file::error("invalid_artifact_request")),
        };
        if !valid_id(id) {
            return Err(artifact_file::error("invalid_artifact_request"));
        }
        let (task, fingerprint) = {
            let records = self.records.lock().unwrap();
            let task = records
                .tasks
                .iter()
                .find(|task| task.id == *id)
                .ok_or_else(|| failure("not_found", "Task not found for this account"))?;
            if !task.terminal() {
                return Err(failure("task_not_terminal", "Task has not finished"));
            }
            if task.result.as_ref().is_none_or(|r| !r.validate(task.kind)) {
                return Err(artifact_file::error("result_unavailable"));
            }
            let fingerprint = records
                .binding
                .as_ref()
                .map(|binding| binding.fingerprint.clone());
            (task.clone(), fingerprint)
        };
        let permit = self
            .artifact_reads
            .clone()
            .try_acquire_owned()
            .map_err(|_| failure("artifact_busy", "Artifact read capacity exhausted"))?;
        let state = self.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let pin = ConfigPin::new(&state.runtime)
                .map_err(|_| failure("configuration_changed", "Account configuration changed"))?;
            if let Some(fingerprint) = fingerprint {
                if pin.fingerprint().map_err(|_| {
                    failure("configuration_changed", "Account configuration changed")
                })? != fingerprint
                {
                    return Err(failure(
                        "configuration_changed",
                        "Account configuration changed",
                    ));
                }
            }
            let value = match call {
                Call::ReadChatPlan {
                    plan_ref,
                    plan_mode,
                    offset,
                    limit,
                } => serde_json::to_value(plan_artifacts::read_plan(
                    &state.runtime,
                    &task,
                    plan_ref,
                    plan_mode,
                    offset,
                    limit,
                )?),
                Call::TaskArtifacts { offset, limit, .. } => {
                    serde_json::to_value(artifacts::list(&state.runtime, &task, offset, limit)?)
                }
                Call::ReadTaskArtifact {
                    artifact_id,
                    offset,
                    max_bytes,
                    ..
                } => serde_json::to_value(artifacts::read(
                    &state.runtime,
                    &task,
                    &artifact_id,
                    offset,
                    max_bytes,
                )?),
                _ => unreachable!(),
            }
            .map_err(|_| artifact_file::error("result_unavailable"))?;
            pin.verify(&state.runtime)
                .map_err(|_| failure("configuration_changed", "Account configuration changed"))?;
            Ok(value)
        })
        .await
        .map_err(|_| artifact_file::error("result_unavailable"))?
    }

    async fn submit_checked(
        self: &Arc<Self>,
        id: String,
        request: Submission,
    ) -> std::result::Result<Value, ServiceError> {
        // Existing idempotent requests do not depend on the parent still being retained.
        let exists = self
            .records
            .lock()
            .unwrap()
            .tasks
            .iter()
            .any(|task| task.id == id);
        if exists
            || (plan_artifacts::reference(&request).is_none() && request.kind != Kind::ChatPlan)
        {
            return self.submit(id, request, None);
        }
        plan::validate(&request, &Default::default())
            .map_err(|_| failure("invalid_task", "Invalid task options"))?;
        if request.kind == Kind::ChatPlan {
            let permit = self
                .artifact_reads
                .clone()
                .try_acquire_owned()
                .map_err(|_| artifact_file::error("artifact_busy"))?;
            let runtime = self.runtime.clone();
            let selection = request.options.chat_plan.clone().unwrap();
            tokio::task::spawn_blocking(move || -> std::result::Result<(), ServiceError> {
                let _permit = permit;
                let pin = ConfigPin::new(&runtime)
                    .map_err(|_| artifact_file::error("configuration_changed"))?;
                super::operations::plan_tasks::plan_chats(&runtime, &selection)
                    .map_err(|_| artifact_file::error("plan_selection_invalid"))?;
                pin.verify(&runtime)
                    .map_err(|_| artifact_file::error("configuration_changed"))?;
                Ok(())
            })
            .await
            .map_err(|_| artifact_file::error("plan_selection_invalid"))??;
            return self.submit(id, request, None);
        }
        let reference = plan_artifacts::reference(&request).unwrap().clone();
        let parent = self
            .records
            .lock()
            .unwrap()
            .tasks
            .iter()
            .find(|task| task.id == reference.task_id)
            .cloned()
            .ok_or_else(|| artifact_file::error("plan_ref_unavailable"))?;
        let permit = self
            .artifact_reads
            .clone()
            .try_acquire_owned()
            .map_err(|_| artifact_file::error("artifact_busy"))?;
        let runtime = self.runtime.clone();
        let checked = request.clone();
        let selection = tokio::task::spawn_blocking(
            move || -> std::result::Result<Option<String>, ServiceError> {
                let _permit = permit;
                let pin = ConfigPin::new(&runtime)
                    .map_err(|_| artifact_file::error("configuration_changed"))?;
                let resolved = plan_artifacts::resolve(&runtime, &parent, &reference)?;
                let selection = match checked.kind {
                    crate::service::protocol::Kind::ChatPlanReview => {
                        plan_artifacts::validate_changes(
                            &resolved.rows,
                            checked.options.chat_plan_review.as_ref().unwrap(),
                        )
                        .map_err(|_| artifact_file::error("plan_selection_invalid"))?;
                        None
                    }
                    crate::service::protocol::Kind::ChatPlanApply => {
                        let targets = super::operations::plan_tasks::selected(
                            &runtime,
                            &resolved.csv,
                            checked.options.chat_plan_apply.as_ref().unwrap().plan_mode,
                        )
                        .map_err(|_| artifact_file::error("plan_selection_invalid"))?;
                        Some(
                            super::operations::plan_tasks::selection_hash(&targets)
                                .map_err(|_| artifact_file::error("plan_selection_invalid"))?,
                        )
                    }
                    _ => unreachable!(),
                };
                pin.verify(&runtime)
                    .map_err(|_| artifact_file::error("configuration_changed"))?;
                Ok(selection)
            },
        )
        .await
        .map_err(|_| artifact_file::error("plan_ref_unavailable"))??;
        self.submit(id, request, selection)
    }

    fn submit(
        &self,
        id: String,
        request: Submission,
        plan_selection: Option<String>,
    ) -> std::result::Result<Value, ServiceError> {
        if !valid_id(&id) {
            return Err(failure("invalid_id", "提交 ID 须为完整随机标识"));
        }
        let pin = ConfigPin::new(&self.runtime)
            .map_err(|_| failure("configuration_changed", "选中配置不可用或已改变"))?;
        let fingerprint = pin
            .fingerprint()
            .map_err(|_| failure("configuration_changed", "选中配置不可用或已改变"))?;
        let mut records = self.records.lock().unwrap();
        let binding = records
            .binding
            .as_ref()
            .ok_or_else(|| failure("not_configured", "请先绑定任务服务设置"))?;
        if binding.fingerprint != fingerprint {
            return Err(failure(
                "configuration_changed",
                "配置已改变，请重启后台后重新提交",
            ));
        }
        let settings = Self::effective_settings(&records).unwrap();
        plan::validate(&request, &settings)
            .map_err(|_| failure("invalid_task", "任务参数、操作范围或本次授权无效"))?;
        let signature = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(
                    &json!({"task":request,"settings":binding.settings,"fingerprint":fingerprint})
                )
                .map_err(|_| failure("invalid_task", "任务不能序列化"))?
            )
        );
        if let Some(old) = records.tasks.iter().find(|task| task.id == id) {
            if records.requests.get(&id) != Some(&signature) {
                return Err(failure("submission_conflict", "提交 ID 已被不同任务使用"));
            }
            return serde_json::to_value(old)
                .map_err(|_| failure("serialization", "任务响应不可用"));
        }
        let permit = self
            .queue
            .try_reserve()
            .map_err(|_| failure("queue_full", "任务队列已满"))?;
        let evicted = if records.tasks.len() >= HISTORY_LIMIT {
            let index = records
                .tasks
                .iter()
                .position(Task::terminal)
                .ok_or_else(|| failure("history_full", "任务历史容量已满"))?;
            let old = records.tasks.remove(index).unwrap();
            let signature = records.requests.remove(&old.id);
            Some((index, old, signature))
        } else {
            None
        };
        let task = Task {
            id: id.clone(),
            kind: request.kind,
            options: request.options.clone(),
            status: "queued".into(),
            created_at: now(),
            started_at: None,
            finished_at: None,
            exit_code: None,
            logs: VecDeque::new(),
            log_start_seq: 0,
            next_log_seq: 0,
            output_dir: self
                .runtime
                .root
                .join("web-output")
                .join(&self.runtime.id)
                .join(&id),
            error: None,
            result: None,
        };
        let (cancel, receiver) = watch::channel(false);
        records.tasks.push_back(task.clone());
        records.requests.insert(id.clone(), signature);
        if self.persist(&mut records).is_err() {
            records.tasks.pop_back();
            records.requests.remove(&id);
            if let Some((index, old, signature)) = evicted {
                if let Some(signature) = signature {
                    records.requests.insert(old.id.clone(), signature);
                }
                records.tasks.insert(index, old);
            }
            return Err(failure("journal_unavailable", "任务未能持久化，未开始执行"));
        }
        records.cancels.insert(id.clone(), cancel);
        self.event_locked(&mut records, "task", serde_json::to_value(&task).unwrap());
        drop(records);
        permit.send(Work {
            plan_selection,
            id,
            request,
            settings,
            fingerprint,
            cancel: receiver,
        });
        Ok(serde_json::to_value(task).unwrap())
    }

    fn cancel(&self, id: &str) -> std::result::Result<Value, ServiceError> {
        if !valid_id(id) {
            return Err(failure("invalid_id", "任务 ID 无效"));
        }
        let mut records = self.records.lock().unwrap();
        let index = records
            .tasks
            .iter()
            .position(|task| task.id == id)
            .ok_or_else(|| failure("not_found", "当前账号没有此任务"))?;
        if !records.tasks[index].terminal() {
            if let Some(signal) = records.cancels.get(id) {
                let _ = signal.send(true);
            }
            let task = &mut records.tasks[index];
            if task.status == "queued" {
                task.status = "cancelled".into();
                task.finished_at = Some(now());
            } else {
                task.status = "cancelling".into();
            }
        }
        let task = records.tasks[index].clone();
        let _ = self.persist(&mut records);
        self.event_locked(
            &mut records,
            "task",
            serde_json::to_value(summary(&task)).unwrap(),
        );
        Ok(serde_json::to_value(task).unwrap())
    }

    fn events(&self, after: u64, limit: usize) -> EventsPage {
        let records = self.records.lock().unwrap();
        let first = records
            .events
            .front()
            .map_or(records.next_event, |event| event.seq);
        let reset = after != 0 && (after < first.saturating_sub(1) || after >= records.next_event);
        let events: Vec<_> = if reset {
            Vec::new()
        } else {
            records
                .events
                .iter()
                .filter(|event| event.seq > after)
                .take(limit)
                .cloned()
                .collect()
        };
        let cursor = events
            .last()
            .map_or(records.next_event - 1, |event| event.seq);
        EventsPage {
            events,
            cursor,
            reset,
        }
    }

    fn event_locked(&self, records: &mut Records, name: &str, data: Value) {
        let event = Event {
            seq: records.next_event,
            name: name.into(),
            data,
        };
        records.next_event += 1;
        records.event_bytes += event.data.to_string().len();
        records.events.push_back(event);
        while records.events.len() > EVENT_LIMIT || records.event_bytes > EVENT_BYTES {
            if let Some(old) = records.events.pop_front() {
                records.event_bytes -= old.data.to_string().len();
            }
        }
        self.changed.notify_waiters();
    }

    pub(super) fn persist(&self, records: &mut Records) -> Result<()> {
        let result = store::persist(&self.runtime, &records.tasks, &records.requests);
        records.journal_ok = result.is_ok();
        result
    }

    pub(super) fn update(&self, id: &str, change: impl FnOnce(&mut Task)) -> Option<Task> {
        let mut records = self.records.lock().unwrap();
        let task = records.tasks.iter_mut().find(|task| task.id == id)?;
        // Queued cancellation is already terminal and must never become running.
        if !task.terminal() {
            change(task);
        }
        let task = task.clone();
        if task.terminal() {
            records.cancels.remove(id);
        }
        let _ = self.persist(&mut records);
        self.event_locked(
            &mut records,
            "task",
            serde_json::to_value(summary(&task)).unwrap(),
        );
        Some(task)
    }

    pub(super) fn log(&self, id: &str, stream: &str, text: &str) {
        let text = self.redactor.lock().unwrap().clean(text);
        let mut records = self.records.lock().unwrap();
        let Some(task) = records.tasks.iter_mut().find(|task| task.id == id) else {
            return;
        };
        let log = Log {
            seq: task.next_log_seq,
            stream: stream.into(),
            text,
        };
        task.next_log_seq += 1;
        if task.logs.len() == LOG_LIMIT {
            task.logs.pop_front();
        }
        task.logs.push_back(log.clone());
        task.log_start_seq = task.logs.front().map_or(task.next_log_seq, |log| log.seq);
        self.event_locked(
            &mut records,
            "log",
            json!({"task_id":id,"seq":log.seq,"stream":log.stream,"text":log.text}),
        );
    }
}

fn summary(task: &Task) -> Task {
    let mut summary = task.clone();
    summary.logs.clear();
    summary
}
