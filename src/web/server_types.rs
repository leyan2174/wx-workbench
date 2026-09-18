//! 后台状态的只读视图，以及当前 Web 实例的事件、队列和限额。
use crate::service::protocol::{Call, Log, Task};
use anyhow::{ensure, Result};
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{broadcast, watch, Semaphore};

pub const HISTORY_LIMIT: usize = 100;
pub const LOG_LIMIT: usize = 256;

#[derive(Clone)]
pub struct Event {
    pub name: &'static str,
    pub data: Value,
    pub seq: Option<u64>,
}
#[derive(Default)]
pub struct Records {
    pub tasks: VecDeque<Task>,
    pub monitor_session: Option<String>,
    pub journal_ok: bool,
}
pub struct Shared {
    pub runtime: crate::runtime::RuntimeContext,
    pub token: String,
    pub authority: String,
    pub origin: String,
    pub allow_plan_scan: bool,
    pub records: Mutex<Records>,
    pub events: broadcast::Sender<Event>,
    pub shutdown: watch::Sender<bool>,
    pub queries: Arc<Semaphore>,
    pub query_waiters: Arc<Semaphore>,
    pub streams: Arc<Semaphore>,
    pub task_requests: Arc<Semaphore>,
}

pub fn random_id() -> Result<String> {
    use windows::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    let mut bytes = [0u8; 32];
    unsafe {
        BCryptGenRandom(
            BCRYPT_ALG_HANDLE::default(),
            &mut bytes,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
        .ok()?;
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

impl Shared {
    pub fn event(&self, name: &'static str, data: Value) {
        let _ = self.events.send(Event {
            name,
            data,
            seq: None,
        });
    }
    pub fn daemon_event(&self, name: &'static str, data: Value, seq: u64) {
        let _ = self.events.send(Event {
            name,
            data,
            seq: Some(seq),
        });
    }
    pub async fn backend(&self, call: Call) -> Result<Value> {
        let _permit = self.task_requests.clone().try_acquire_owned()?;
        tokio::time::timeout(
            Duration::from_secs(25),
            crate::service::client::request(&self.runtime, call),
        )
        .await?
    }
    pub fn project_task(&self, value: Value) -> Result<()> {
        let mut task: Task = serde_json::from_value(value)?;
        ensure!(
            task.logs.len() <= LOG_LIMIT,
            "Task log projection exceeds limit"
        );
        let mut records = self.records.lock().unwrap();
        if let Some(existing) = records.tasks.iter_mut().find(|item| item.id == task.id) {
            if task.logs.is_empty() {
                // Status events are summaries. A prior Get may already be ahead of them.
                ensure!(
                    task.next_log_seq <= existing.next_log_seq,
                    "Task log gap; reconciliation required"
                );
                task.logs = existing.logs.clone();
                task.log_start_seq = existing.log_start_seq;
                task.next_log_seq = existing.next_log_seq;
            }
            *existing = task;
        } else {
            ensure!(
                !task.logs.is_empty() || task.next_log_seq == 0,
                "Missing log tail; reconciliation required"
            );
            if records.tasks.len() >= HISTORY_LIMIT {
                records.tasks.pop_front();
            }
            records.tasks.push_back(task);
        }
        Ok(())
    }
    pub fn project_log(&self, value: &Value) -> Result<()> {
        let id = value["task_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing task id"))?;
        let log: Log = serde_json::from_value(value.clone())?;
        let mut records = self.records.lock().unwrap();
        let task = records
            .tasks
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| anyhow::anyhow!("Unknown log task; reconciliation required"))?;
        // Reconciliation can already include this event's log tail.
        if log.seq < task.next_log_seq {
            return Ok(());
        }
        ensure!(
            log.seq == task.next_log_seq,
            "Log gap; reconciliation required"
        );
        task.next_log_seq = log
            .seq
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Log sequence overflow"))?;
        if task.logs.len() >= LOG_LIMIT {
            task.logs.pop_front();
        }
        task.logs.push_back(log);
        task.log_start_seq = task
            .logs
            .front()
            .map(|log| log.seq)
            .unwrap_or(task.next_log_seq);
        Ok(())
    }
    pub async fn reconcile(&self) -> Result<u64> {
        let list = self.backend(Call::List {}).await?;
        let rows = list["tasks"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Missing tasks"))?;
        ensure!(rows.len() <= HISTORY_LIMIT, "Task projection exceeds limit");
        let cursor = list["cursor"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("Missing cursor"))?;
        let mut tasks = VecDeque::new();
        // List omits logs. Rebuild a bounded snapshot without submitting any work.
        for row in rows.iter().rev() {
            let id = row["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing task id"))?;
            let task: Task =
                serde_json::from_value(self.backend(Call::Get { id: id.into() }).await?)?;
            ensure!(
                task.logs.len() <= LOG_LIMIT,
                "Task log projection exceeds limit"
            );
            tasks.push_back(task);
        }
        let mut records = self.records.lock().unwrap();
        records.tasks = tasks;
        records.journal_ok = list["history_persisted"].as_bool().unwrap_or(false);
        Ok(cursor)
    }
}
