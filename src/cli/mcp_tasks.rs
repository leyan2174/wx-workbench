//! MCP 后台任务适配：宿主授权和结果转换留在入口，调度与执行全部使用任务 RPC。
use crate::{
    mcp::protocol::{CallContext, DispatchError, Dispatcher, Tool},
    runtime::RuntimeContext,
    service::{
        client,
        config_pin::ConfigPin,
        plan,
        protocol::{valid_task_id, Call, Kind, Options, ServiceError, Submission},
        settings::{Settings, SettingsInput},
    },
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    cell::{Cell, OnceCell},
    path::PathBuf,
};

#[derive(Debug, Default, Clone, clap::Args)]
#[group(id = "mcp_tasks")]
pub struct Args {
    /// 开放固定账号的后台任务列表、详情、取消及事件；默认关闭。
    #[arg(long)]
    pub tasks: bool,
    /// Allow listing and reading verified CLI/Web/task artifact bytes in the fixed account.
    /// Independent of task submission and media-write permissions; host-only, default off.
    #[arg(long, requires = "tasks")]
    pub task_allow_artifact_read: bool,
    /// 宿主允许提交的任务类型及其固定目录写入；可重复或用逗号分隔。
    #[arg(long, requires = "tasks", value_delimiter = ',', value_parser = crate::service::protocol::parse_task_kind)]
    pub task_kind: Vec<Kind>,
    /// 允许任务在 daemon 管理的固定目录写入媒体；不能改变同步媒体工具的输出根。
    #[arg(long, requires = "tasks")]
    pub task_allow_media_write: bool,
    /// 允许选定任务读取微信进程内存；提交仍须包含本次扫描确认。
    #[arg(long, requires = "tasks")]
    pub task_allow_memory_scan: bool,
    /// 允许朋友圈任务下载媒体；不授权任意 URL。
    #[arg(long, requires_all = ["tasks", "task_allow_media_write"])]
    pub task_allow_media_download: bool,
    /// 与 CLI/Web 任务 Configure 使用同一缓存目录；仅宿主可指定。
    #[arg(long, requires = "tasks")]
    pub task_image_cache_dir: Option<PathBuf>,
}

impl Args {
    pub fn absolute_paths(&mut self) -> anyhow::Result<()> {
        if let Some(path) = &mut self.task_image_cache_dir {
            if path.is_relative() {
                *path = std::env::current_dir()?.join(&*path);
            }
        }
        Ok(())
    }

    fn permits(&self, kind: Kind) -> bool {
        self.tasks
            && self.task_kind.contains(&kind)
            && match kind {
                Kind::WechatKeys | Kind::ImageKey => self.task_allow_memory_scan,
                Kind::ExportAll | Kind::DecodeImages | Kind::SnsDecrypt => {
                    self.task_allow_media_write
                }
                Kind::WechatDecrypt => true,
            }
    }

    fn capabilities(&self) -> Vec<Value> {
        plan::capabilities()
            .into_iter()
            .filter(|entry| {
                entry["enabled"] == true
                    && serde_json::from_value(entry["kind"].clone())
                        .is_ok_and(|kind| self.permits(kind))
            })
            .collect()
    }

    fn permits_option(&self, option: &str) -> bool {
        match option {
            "include_sns" => self.permits(Kind::SnsDecrypt),
            "include_sns_media" => self.task_allow_media_download,
            "authorize_memory_scan" => self.task_allow_memory_scan,
            _ => true,
        }
    }

    fn authorize(&self, call: &Call) -> bool {
        if !self.tasks {
            return false;
        }
        if matches!(
            call,
            Call::TaskArtifacts { .. } | Call::ReadTaskArtifact { .. }
        ) {
            return self.task_allow_artifact_read;
        }
        let Call::Submit { task, .. } = call else {
            return true;
        };
        let o = &task.options;
        self.permits(task.kind)
            && (!o.authorize_memory_scan || self.task_allow_memory_scan)
            && (!o.include_sns_media || self.task_allow_media_download)
            && (!o.include_sns || self.permits(Kind::SnsDecrypt))
    }

    pub fn tools(&self) -> Vec<Tool> {
        if !self.tasks {
            return Vec::new();
        }
        let id = json!({"type":"string","pattern":"^[0-9a-f]{64}$","minLength":64,"maxLength":64});
        let object = |properties: Value, required: &[&str]| {
            json!({
                "type":"object", "properties":properties, "required":required, "additionalProperties":false
            })
        };
        let mut tools = vec![
            Tool::task(
                "list_tasks",
                "列出固定账号 daemon 的任务；包含 CLI/Web 提交的任务。",
                object(json!({}), &[]),
                false,
            ),
            Tool::task(
                "get_task",
                "读取 daemon 任务详情和保留的日志；interrupted 不表示自动续跑。",
                object(json!({"id":id}), &["id"]),
                false,
            ),
            Tool::task(
                "cancel_task",
                "显式请求 daemon 取消该账号任务并回收其子进程；不是取消 MCP 调用。",
                object(json!({"id":id}), &["id"]),
                false,
            ),
            Tool::task(
                "get_task_events",
                "按 daemon 游标读取有限事件；reset 表示需重新列表，不是持久日志重放。",
                object(
                    json!({
                        "after":{"type":"integer","minimum":0,"default":0},
                        "limit":{"type":"integer","minimum":1,"maximum":128,"default":32},
                        "wait_ms":{"type":"integer","minimum":0,"maximum":2000,"default":0}
                    }),
                    &[],
                ),
                false,
            ),
        ];
        if self.task_allow_artifact_read {
            tools.extend([
            Tool::task(
                "list_task_artifacts",
                "List verified artifacts of a terminal task in the fixed account; no paths.",
                object(json!({
                    "id":id,
                    "offset":{"type":"integer","minimum":0,"default":0},
                    "limit":{"type":"integer","minimum":1,"maximum":100,"default":50}
                }), &["id"]),
                false,
            ),
            Tool::task(
                "read_task_artifact",
                "Read one base64 block of a verified task artifact. The frame budget may reduce max_bytes; resume with next_offset. Does not execute or preview the bytes.",
                object(json!({
                    "id":id,
                    "artifact_id":id,
                    "offset":{"type":"integer","minimum":0,"default":0},
                    "max_bytes":{"type":"integer","minimum":1,"maximum":1048576,"default":1048576}
                }), &["id","artifact_id"]),
                false,
            ),
            ]);
        }
        let capabilities = self.capabilities();
        if !capabilities.is_empty() {
            let mut properties = serde_json::Map::new();
            for option in capabilities
                .iter()
                .filter_map(|entry| entry["options"].as_array())
                .flatten()
                .filter_map(Value::as_str)
            {
                if !self.permits_option(option) {
                    continue;
                }
                let schema = match option {
                    "users" => {
                        json!({"type":"array","maxItems":200,"uniqueItems":true,"items":{"type":"string","minLength":1,"maxLength":256}})
                    }
                    "formats" => {
                        json!({"type":"array","maxItems":3,"uniqueItems":true,"items":{"enum":["json","csv","html"]}})
                    }
                    "max_media_bytes" => {
                        json!({"type":"integer","minimum":1,"maximum":524288000,"default":67108864})
                    }
                    "max_total_media_bytes" => {
                        json!({"type":"integer","minimum":1,"maximum":17179869184u64,"default":2147483648u64})
                    }
                    "dry_run" => json!({"type":"boolean","default":false}),
                    _ => json!({"type":"boolean"}),
                };
                properties.insert(option.into(), schema);
            }
            let mut schema = object(
                json!({
                    "idempotency_key":id,
                    "kind":{"type":"string","enum":capabilities.iter().map(|entry| entry["kind"].clone()).collect::<Vec<_>>()},
                    "options":{"type":"object","properties":properties,"additionalProperties":false}
                }),
                &["idempotency_key", "kind"],
            );
            if self.permits(Kind::ExportAll) {
                schema["allOf"] = json!([{
                    "if":{"properties":{"kind":{"const":"export_all"}},"required":["kind"]},
                    "else":{"properties":{"options":{"properties":{
                        "dry_run":{"const":false},"max_media_bytes":false,"max_total_media_bytes":false
                    }}}}
                }]);
                schema["properties"]["options"]["allOf"] = json!([
                    {"if":{"properties":{"include_images":{"const":false}},"required":["include_images"]},
                     "then":{"properties":{"max_media_bytes":false,"max_total_media_bytes":false}}},
                    {"if":{"properties":{"dry_run":{"const":true}},"required":["dry_run"]},
                     "then":{"properties":{"include_sns":{"const":false}}}}
                ]);
            }
            tools.push(Tool::task("submit_task", "异步提交到现有 daemon。必须保存并复用 64 位小写十六进制幂等键；响应丢失、超时或 MCP 断连不自动取消任务。", schema, self.task_allow_media_download));
        }
        tools
    }
}

struct Binding {
    runtime: RuntimeContext,
    fingerprint: Option<String>,
}

/// 查询与任务只保留这一份账号选择。只保存配置指纹，不长期锁住图片密钥更新所需的文件。
pub struct Account {
    binding: OnceCell<Binding>,
    protect_tasks: bool,
}
impl Account {
    pub fn new(protect_tasks: bool) -> Self {
        Self {
            binding: OnceCell::new(),
            protect_tasks,
        }
    }

    pub fn get(&self) -> Result<&RuntimeContext, DispatchError> {
        if self.binding.get().is_none() {
            if std::env::var_os("WX_CLI_CONFIG").is_none_or(|path| path.is_empty()) {
                return Err(DispatchError::Unavailable);
            }
            let runtime = RuntimeContext::load().map_err(|_| DispatchError::Unavailable)?;
            let fingerprint = if self.protect_tasks {
                Some(
                    ConfigPin::new(&runtime)
                        .and_then(|pin| pin.fingerprint())
                        .map_err(|_| DispatchError::Unavailable)?,
                )
            } else {
                None
            };
            self.binding
                .set(Binding {
                    runtime,
                    fingerprint,
                })
                .map_err(|_| DispatchError::Unavailable)?;
        }
        self.runtime().ok_or(DispatchError::Unavailable)
    }

    pub fn runtime(&self) -> Option<&RuntimeContext> {
        self.binding.get().map(|binding| &binding.runtime)
    }

    fn matches_service(&self, info: &Value) -> bool {
        self.binding.get().is_some_and(|binding| {
            info["runtime_id"].as_str() == Some(binding.runtime.id.as_str())
                && binding.fingerprint.is_some()
                && info["config_fingerprint"].as_str() == binding.fingerprint.as_deref()
        })
    }

    fn verify(&self) -> anyhow::Result<()> {
        let binding = self
            .binding
            .get()
            .ok_or_else(|| anyhow::anyhow!("账号未固定"))?;
        let current = RuntimeContext::load()?;
        anyhow::ensure!(
            current.id == binding.runtime.id && current.directory == binding.runtime.directory,
            "账号配置改变"
        );
        let current = ConfigPin::new(&binding.runtime)?.fingerprint()?;
        anyhow::ensure!(
            binding.fingerprint.as_deref() == Some(&current),
            "账号配置改变"
        );
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitInput {
    idempotency_key: String,
    kind: Kind,
    #[serde(default)]
    options: Options,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdInput {
    id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyInput {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactsInput {
    id: String,
    #[serde(default)]
    offset: u64,
    #[serde(default = "artifact_limit")]
    limit: u32,
}
fn artifact_limit() -> u32 {
    50
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArtifactInput {
    id: String,
    artifact_id: String,
    #[serde(default)]
    offset: u64,
    #[serde(default = "artifact_bytes")]
    max_bytes: u32,
}
fn artifact_bytes() -> u32 {
    1048576
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventsInput {
    #[serde(default)]
    after: u64,
    #[serde(default = "event_limit")]
    limit: usize,
    #[serde(default)]
    wait_ms: u64,
}
fn event_limit() -> usize {
    32
}

fn parse(name: &str, arguments: &Value) -> Result<Call, DispatchError> {
    let invalid = || DispatchError::InvalidArguments;
    match name {
        "submit_task" => {
            let input: SubmitInput =
                serde_json::from_value(arguments.clone()).map_err(|_| invalid())?;
            if !valid_task_id(&input.idempotency_key) {
                return Err(invalid());
            }
            let task = Submission {
                kind: input.kind,
                options: input.options,
            };
            super::tasks::validate_export_options(&task).map_err(|_| invalid())?;
            Ok(Call::Submit {
                idempotency_key: input.idempotency_key,
                task,
            })
        }
        "list_tasks" => {
            let _: EmptyInput = serde_json::from_value(arguments.clone()).map_err(|_| invalid())?;
            Ok(Call::List {})
        }
        "get_task" | "cancel_task" => {
            let input: IdInput =
                serde_json::from_value(arguments.clone()).map_err(|_| invalid())?;
            if !valid_task_id(&input.id) {
                return Err(invalid());
            }
            Ok(if name == "get_task" {
                Call::Get { id: input.id }
            } else {
                Call::Cancel { id: input.id }
            })
        }
        "get_task_events" => {
            let input: EventsInput =
                serde_json::from_value(arguments.clone()).map_err(|_| invalid())?;
            if !(1..=128).contains(&input.limit) || input.wait_ms > 2000 {
                return Err(invalid());
            }
            Ok(Call::Events {
                after: input.after,
                limit: input.limit,
                wait_ms: input.wait_ms,
            })
        }
        "list_task_artifacts" => {
            let input: ArtifactsInput =
                serde_json::from_value(arguments.clone()).map_err(|_| invalid())?;
            if !valid_task_id(&input.id) || !(1..=100).contains(&input.limit) {
                return Err(invalid());
            }
            Ok(Call::TaskArtifacts {
                id: input.id,
                offset: input.offset,
                limit: input.limit,
            })
        }
        "read_task_artifact" => {
            let input: ReadArtifactInput =
                serde_json::from_value(arguments.clone()).map_err(|_| invalid())?;
            if !valid_task_id(&input.id)
                || !valid_task_id(&input.artifact_id)
                || !(1..=1048576).contains(&input.max_bytes)
                || input
                    .offset
                    .checked_add(u64::from(input.max_bytes))
                    .is_none()
            {
                return Err(invalid());
            }
            Ok(Call::ReadTaskArtifact {
                id: input.id,
                artifact_id: input.artifact_id,
                offset: input.offset,
                max_bytes: input.max_bytes,
            })
        }
        _ => Err(invalid()),
    }
}

fn content(data: Value, failed: bool) -> Value {
    json!({"content":[{"type":"text","text":data.to_string()}],"structuredContent":data,"isError":failed})
}

fn frame_size(data: &Value, response_id: &Value) -> Result<usize, DispatchError> {
    serde_json::to_vec(&json!({"jsonrpc":"2.0","id":response_id,"result":data}))
        .map(|bytes| bytes.len() + 1)
        .map_err(|_| DispatchError::Internal)
}

fn fit_artifact_read(call: &mut Call, context: &CallContext) -> Result<(), DispatchError> {
    let Call::ReadTaskArtifact {
        id,
        artifact_id,
        max_bytes,
        ..
    } = call
    else {
        return Ok(());
    };
    let budget = context.budget()?;
    // Both MCP content and structuredContent carry base64. Reserve worst-case
    // numeric widths and the full JSON-RPC envelope before requesting bytes.
    let sample = content(
        json!({
            "version":1,"task_id":id,"artifact_id":artifact_id,"offset":u64::MAX,
            "bytes_read":u64::MAX,"next_offset":u64::MAX,"size":u64::MAX,
            "sha256":"0".repeat(64),"encoding":"base64","data_base64":"","eof":false
        }),
        false,
    );
    let overhead = frame_size(&sample, &budget.response_id)?;
    let bytes = budget.max_response_bytes.saturating_sub(overhead) / 8 * 3;
    if bytes == 0 {
        return Err(DispatchError::ResultLimit);
    }
    *max_bytes = (*max_bytes).min(bytes.min(1048576) as u32);
    Ok(())
}

fn bounded_content(data: Value, context: &CallContext) -> Result<Value, DispatchError> {
    let result = content(data, false);
    let budget = context.budget()?;
    if frame_size(&result, &budget.response_id)? > budget.max_response_bytes {
        return Err(DispatchError::ResultLimit);
    }
    Ok(result)
}

fn failure(code: &str) -> Value {
    // 保留共享服务的稳定错误码，不把文件路径、配置内容或底层异常文本交给模型。
    let (code, message) = match code {
        "host_forbidden" => (code, "宿主未授权此任务能力或选项"),
        "configuration_changed" => (code, "固定账号配置已改变或不可用；请重启 MCP，不会切换账号"),
        "settings_conflict" | "invalid_settings" => {
            (code, "后台设置与宿主配置不匹配；未覆盖已有设置")
        }
        "invalid_id" | "invalid_events" | "invalid_task" => (code, "任务参数不符合共享服务约束"),
        "submission_conflict" => (
            code,
            "幂等键已用于不同任务；请勿用新键重试结果不确定的原提交",
        ),
        "not_found" => (code, "当前账号的保留任务记录中没有此 ID"),
        "unauthorized" => (code, "Task service authorization failed"),
        "invalid_artifact_request" => (code, "Invalid artifact ID, offset, or byte/page limit"),
        "task_not_terminal" => (code, "Artifacts are available only after the task ends"),
        "result_unavailable" => (
            code,
            "No verified artifact result is available for this task",
        ),
        "artifact_unavailable" => (code, "The registered artifact is currently unavailable"),
        "artifact_changed" => (
            code,
            "The registered artifact has changed; bytes were not returned",
        ),
        "artifact_unsafe" => (code, "The registered artifact cannot be opened safely"),
        "artifact_busy" => (code, "Artifact readers are busy; retry this read later"),
        "queue_full" | "history_full" | "stopping" | "not_configured" => {
            (code, "后台任务服务暂不接受此操作；可保留原幂等键重试")
        }
        "outcome_unknown" => (
            code,
            "任务 RPC 结果不确定；提交请复用原幂等键，取消请查询任务详情。未发送额外取消",
        ),
        _ => (
            "backend_unavailable",
            "任务服务不可用；未启动替代后台，也未发送额外取消",
        ),
    };
    content(json!({"error":{"code":code,"message":message}}), true)
}

/// 只包裹既有查询分派器；不接管其同步语音逻辑或 daemon MCP 会话生命周期。
pub struct Adapter<'a, D> {
    pub query: D,
    pub account: &'a Account,
    pub invalidated: &'a Cell<bool>,
    pub io: &'a tokio::runtime::Runtime,
    pub args: &'a Args,
}
impl<D: Dispatcher> Dispatcher for Adapter<'_, D> {
    fn dispatch(
        &mut self,
        request: crate::ipc::Request,
        context: &CallContext,
    ) -> Result<crate::ipc::Response, DispatchError> {
        self.query.dispatch(request, context)
    }

    fn task_tools(&self) -> Vec<Tool> {
        self.args.tools()
    }

    fn dispatch_task(
        &mut self,
        name: &str,
        arguments: &Value,
        context: &CallContext,
    ) -> Result<Value, DispatchError> {
        context.check()?;
        let mut call = parse(name, arguments)?;
        if !self.args.authorize(&call) {
            return Ok(failure("host_forbidden"));
        }
        fit_artifact_read(&mut call, context)?;
        let artifact_call = matches!(
            call,
            Call::TaskArtifacts { .. } | Call::ReadTaskArtifact { .. }
        );
        if self.invalidated.get() {
            return Ok(failure("configuration_changed"));
        }
        let runtime = match self.account.get() {
            Ok(runtime) => runtime,
            Err(_) => {
                self.invalidated.set(true);
                return Ok(failure("configuration_changed"));
            }
        };
        if self.account.verify().is_err() {
            self.invalidated.set(true);
            return Ok(failure("configuration_changed"));
        }
        let mut mutation_sent = false;
        let result = (|| -> anyhow::Result<Value> {
            crate::service::query_client::ensure_running_quiet(runtime)?;
            context
                .check()
                .map_err(|_| anyhow::anyhow!("任务调用已结束"))?;
            self.io.block_on(async {
                tokio::time::timeout(context.remaining(), client::wait_ready(runtime)).await??;
                let info = client::request_with_timeout(
                    runtime,
                    Call::Configure {
                        settings: SettingsInput {
                            image_cache_dir: self.args.task_image_cache_dir.clone(),
                        },
                    },
                    context.remaining(),
                )
                .await?;
                if !self.account.matches_service(&info) {
                    return Err(ServiceError::new(
                        "configuration_changed",
                        "后台绑定与固定账号不一致",
                    )
                    .into());
                }
                if artifact_call && !super::tasks::supports_artifacts(&info) {
                    return Err(ServiceError::new(
                        "result_unavailable",
                        "This task service does not support task_artifacts_v1",
                    )
                    .into());
                }
                if let Call::Submit { task, .. } = &call {
                    let settings: Settings = serde_json::from_value(info["settings"].clone())?;
                    plan::validate(task, &settings)
                        .map_err(|_| ServiceError::new("invalid_task", "任务参数无效"))?;
                }
                self.account
                    .verify()
                    .map_err(|_| ServiceError::new("configuration_changed", "固定账号配置改变"))?;
                // 配置绑定和等待共用本次工具预算；只发送一次变更 RPC，不重放不确定结果。
                context
                    .check()
                    .map_err(|_| anyhow::anyhow!("任务调用已结束"))?;
                mutation_sent = matches!(call, Call::Submit { .. } | Call::Cancel { .. });
                client::request_with_timeout(runtime, call, context.remaining()).await
            })
        })();
        if self.account.verify().is_err() {
            self.invalidated.set(true);
            return Ok(failure("configuration_changed"));
        }
        Ok(match result {
            Ok(data) if artifact_call => return bounded_content(data, context),
            Ok(data) => content(data, false),
            Err(error) => {
                if let Some(error) = error.downcast_ref::<ServiceError>() {
                    if matches!(
                        error.code.as_str(),
                        "configuration_changed" | "settings_conflict"
                    ) {
                        self.invalidated.set(true);
                    }
                    failure(&error.code)
                } else {
                    failure(if mutation_sent {
                        "outcome_unknown"
                    } else {
                        "backend_unavailable"
                    })
                }
            }
        })
    }
}

#[cfg(test)]
#[path = "mcp_tasks_tests.rs"]
mod tests;
