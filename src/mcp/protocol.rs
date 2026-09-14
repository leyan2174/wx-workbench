//! 可注入查询器的 MCP 子集；不打开数据库、不启动 daemon、不访问全局标准输出。
use crate::ipc::{Request, Response};
use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const DEFAULT_MAX_FRAME_BYTES: usize = 1024 * 1024;

/// 仅携带公开错误类别，不允许底层 message、路径或密钥进入错误通道。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DispatchError {
    Unavailable,
    Internal,
    Cancelled,
    TimedOut,
    QueryFailed,
    InvalidResponse,
    ResultLimit,
    InvalidArguments,
}

const MAX_CANDIDATES: usize = 10_000;

/// 外部 transport 可以保留克隆并从其他线程取消；不修改任何全局取消状态。
#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);
impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// 同一工具调用中的所有 IPC 共用同一截止时间，不能通过重试重置预算。
#[derive(Clone)]
pub struct CallContext {
    cancellation: CancellationToken,
    deadline: Instant,
    max_response_bytes: usize,
    response_id: Value,
}
/// IPC budget is created before daemon startup, so startup cannot renew a call.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallBudget {
    pub deadline_unix_ms: u64,
    pub max_response_bytes: usize,
    pub response_id: Value,
}

fn unix_ms() -> Result<u64, DispatchError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .ok_or(DispatchError::Unavailable)
}

impl CallContext {
    pub fn budget(&self) -> Result<CallBudget, DispatchError> {
        self.check()?;
        Ok(CallBudget {
            deadline_unix_ms: unix_ms()?.saturating_add(self.remaining().as_millis() as u64),
            max_response_bytes: self.max_response_bytes,
            response_id: self.response_id.clone(),
        })
    }

    pub fn from_budget(budget: CallBudget) -> Result<Self, DispatchError> {
        if !(1024..=16 * 1024 * 1024).contains(&budget.max_response_bytes)
            || !matches!(
                budget.response_id,
                Value::Null | Value::String(_) | Value::Number(_)
            )
        {
            return Err(DispatchError::Unavailable);
        }
        let remaining = budget
            .deadline_unix_ms
            .saturating_sub(unix_ms()?)
            .min(30_000);
        let mut context = Self::new(
            CancellationToken::default(),
            Duration::from_millis(remaining),
        );
        context.max_response_bytes = budget.max_response_bytes;
        context.response_id = budget.response_id;
        context.check()?;
        Ok(context)
    }

    pub fn new(cancellation: CancellationToken, timeout: Duration) -> Self {
        // 溢出按立即超时处理，不能意外退化为无限等待。
        let now = Instant::now();
        Self {
            cancellation,
            deadline: now.checked_add(timeout).unwrap_or(now),
            max_response_bytes: DEFAULT_MAX_FRAME_BYTES,
            response_id: Value::Null,
        }
    }
    pub fn check(&self) -> Result<(), DispatchError> {
        if self.cancellation.is_cancelled() {
            Err(DispatchError::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(DispatchError::TimedOut)
        } else {
            Ok(())
        }
    }
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// 写盘前按真实请求 ID、JSON 转义和外层封装核验成功文本的总响应预算。
    pub fn check_text_result(&self, text: &str) -> Result<(), DispatchError> {
        self.check()?;
        if text.len() > self.max_response_bytes {
            return Err(DispatchError::ResultLimit);
        }
        let mut bounded = LimitedWriter {
            bytes: Vec::new(),
            limit: self.max_response_bytes,
        };
        serde_json::to_writer(
            &mut bounded,
            &result(
                self.response_id.clone(),
                text_result(text.to_owned(), false),
            ),
        )
        .map_err(|_| DispatchError::ResultLimit)
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }
}
impl Default for CallContext {
    fn default() -> Self {
        Self::new(CancellationToken::default(), Duration::from_secs(30))
    }
}

pub trait Dispatcher {
    fn dispatch(
        &mut self,
        request: Request,
        context: &CallContext,
    ) -> Result<Response, DispatchError>;

    /// 默认不开放后台任务；宿主提供固定工具清单，不从模型参数获取授权。
    fn task_tools(&self) -> Vec<Tool> {
        Vec::new()
    }

    /// 只适配任务 RPC，返回 MCP 内容；不得在此建立队列或等待任务执行完成。
    fn dispatch_task(
        &mut self,
        _name: &str,
        _arguments: &Value,
        _context: &CallContext,
    ) -> Result<Value, DispatchError> {
        Err(DispatchError::Unavailable)
    }
}
impl<F: FnMut(Request) -> Result<Response, DispatchError>> Dispatcher for F {
    fn dispatch(&mut self, request: Request, _: &CallContext) -> Result<Response, DispatchError> {
        self(request)
    }
}
/// 保留旧单参数 callback；需要实时取消的适配器显式选择这个包装器。
pub struct Controlled<F>(pub F);
impl<F: FnMut(Request, &CallContext) -> Result<Response, DispatchError>> Dispatcher
    for Controlled<F>
{
    fn dispatch(
        &mut self,
        request: Request,
        context: &CallContext,
    ) -> Result<Response, DispatchError> {
        (self.0)(request, context)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    New,
    AwaitingInitialized,
    Ready,
}

#[derive(Debug, Clone)]
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    command: &'static str,
    pub input_schema: Value,
}

impl Tool {
    pub fn task(
        name: &'static str,
        description: &'static str,
        input_schema: Value,
        external: bool,
    ) -> Self {
        Self {
            name,
            description,
            command: if external {
                "submit_task_external"
            } else {
                name
            },
            input_schema,
        }
    }

    fn open_world(&self) -> bool {
        matches!(self.command, "transcribe_voice" | "submit_task_external")
    }

    fn destructive(&self) -> bool {
        matches!(
            self.command,
            "submit_task" | "submit_task_external" | "cancel_task"
        )
    }

    fn read_only(&self) -> bool {
        matches!(
            self.command,
            "sessions"
                | "contacts"
                | "history"
                | "search"
                | "decode_transfer"
                | "decode_location"
                | "attachments"
                | "contact_tags"
                | "tag_members"
                | "decode_refer"
                | "voice_messages"
                | "decode_file_message"
                | "decode_record_item"
                | "list_tasks"
                | "get_task"
                | "get_task_events"
        )
    }
}

fn string() -> Value {
    json!({"type":"string", "maxLength":4096})
}
fn integer(min: i64, max: i64) -> Value {
    json!({"type":"integer","minimum":min,"maximum":max})
}

/// 固定工具白名单；schema 驱动参数验证和 IPC 映射，读写标记按工具区分。
pub fn tools() -> Vec<Tool> {
    let limit = json!({"type":"integer","minimum":1,"maximum":500});
    let time = integer(i64::MIN, i64::MAX);
    let attachment_time =
        json!({"type":"integer","minimum":i64::MIN,"maximum":i64::MAX,"default":0});
    let mut out = Vec::new();
    for (name, description, command, properties, required) in [
        (
            "get_recent_sessions",
            "List recent sessions via Rust Sessions",
            "sessions",
            json!({"limit":limit}),
            vec![],
        ),
        (
            "get_contacts",
            "Find contacts via Rust Contacts",
            "contacts",
            json!({"query":string(),"limit":limit}),
            vec![],
        ),
        (
            "get_chat_history",
            "读取历史；支持多类型筛选及全分片最早或最新分页，since/until 为 Unix 秒",
            "history",
            json!({"chat_name":string(),"limit":limit,"offset":integer(0,1_000_000),"since":time,"until":time,"msg_type":time}),
            vec!["chat_name"],
        ),
        (
            "search_messages",
            "Search messages; chats is an optional array, dates are Unix seconds",
            "search",
            json!({"keyword":string(),"chats":{"type":"array","items":string(),"maxItems":100},"limit":limit,"since":time,"until":time,"msg_type":time}),
            vec!["keyword"],
        ),
        (
            "decode_transfer",
            "Read structured transfer metadata; does not transfer funds",
            "decode_transfer",
            json!({"chat_name":string(),"local_id":time,"create_time":time}),
            vec!["chat_name", "local_id"],
        ),
        (
            "decode_location",
            "Read structured location metadata",
            "decode_location",
            json!({"chat_name":string(),"local_id":time,"create_time":time}),
            vec!["chat_name", "local_id"],
        ),
    ] {
        out.push(Tool { name, description, command, input_schema: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}) });
    }
    out.push(Tool { name: "get_new_messages", description: "Legacy session-summary polling: first call shows unread sessions, later calls show changed summaries; session-local cursor, at most 10000 sessions", command: "sessions", input_schema: json!({"type":"object","properties":{},"required":[],"additionalProperties":false}) });
    out.push(Tool { name: "get_chat_images", description: "List image messages with exact resource MD5 and encrypted DAT size when unambiguous; metadata only, no decoding or upload", command: "attachments", input_schema: json!({"type":"object","properties":{"chat_name":string(),"limit":limit,"offset":integer(0,1_000_000),"since":time,"until":time},"required":["chat_name"],"additionalProperties":false}) });
    out.extend([
        Tool { name: "get_contact_tags", description: "只读联系人标签与关联计数，不返回全部成员", command: "contact_tags", input_schema: json!({"type":"object","properties":{},"required":[],"additionalProperties":false}) },
        Tool { name: "get_tag_members", description: "查询唯一匹配标签的成员；精确匹配优先，多匹配明确失败", command: "tag_members", input_schema: json!({"type":"object","properties":{"tag_name":string()},"required":["tag_name"],"additionalProperties":false}) },
        Tool { name: "decode_refer", description: "读取唯一消息的结构化引用，不读取被引用附件或下载内容", command: "decode_refer", input_schema: json!({"type":"object","properties":{"chat_name":string(),"local_id":time,"create_time":time},"required":["chat_name","local_id"],"additionalProperties":false}) },
        Tool { name: "get_voice_messages", description: "跨媒体分片查询真实语音长度与时间，保持来源；不读取音频正文或上传", command: "voice_messages", input_schema: json!({"type":"object","properties":{"chat_name":string(),"limit":limit,"offset":integer(0,1_000_000),"since":time,"until":time},"required":["chat_name"],"additionalProperties":false}) },
        Tool { name: "decode_file_message", description: "只读解析唯一文件消息及账号内本地附件信息；不接受任意路径，不下载、上传或写入文件", command: "decode_file_message", input_schema: json!({"type":"object","properties":{"chat_name":string(),"local_id":integer(1,i64::MAX),"create_time":attachment_time},"required":["chat_name","local_id"],"additionalProperties":false}) },
        Tool { name: "decode_record_item", description: "只读解析聊天记录中从零开始索引的条目及本地附件信息；不接受任意路径，不下载、上传或写入文件", command: "decode_record_item", input_schema: json!({"type":"object","properties":{"chat_name":string(),"local_id":integer(1,i64::MAX),"item_index":integer(0,i64::MAX),"create_time":attachment_time},"required":["chat_name","local_id","item_index"],"additionalProperties":false}) },
    ]);
    out.push(Tool {
        name: "decode_image",
        description: "解码唯一图片到宿主显式配置的目录；不覆盖已有文件，不扫描密钥或下载内容",
        command: "decode_image",
        input_schema: json!({"type":"object","properties":{
            "chat_name":string(), "local_id":integer(1,i64::MAX),
            "create_time":{"type":"integer","minimum":0,"maximum":i64::MAX,"default":0}
        },"required":["chat_name","local_id"],"additionalProperties":false}),
    });
    for (name, description) in [
        (
            "decode_voice",
            "按旧媒体记录 ID 解码语音，由宿主发布 WAV；不覆盖已有文件，不上传",
        ),
        (
            "transcribe_voice",
            "按旧媒体记录 ID 转录语音；仅使用宿主显式配置的后端与上传授权，可保存账号隔离缓存",
        ),
    ] {
        out.push(Tool {
            name,
            description,
            command: name,
            input_schema: json!({"type":"object","properties":{
                "chat_name":string(), "local_id":integer(1,i64::MAX)
            },"required":["chat_name","local_id"],"additionalProperties":false}),
        });
    }
    for tool in &mut out {
        let properties = tool.input_schema["properties"].as_object_mut().unwrap();
        if [
            "get_chat_history",
            "search_messages",
            "get_chat_images",
            "get_voice_messages",
        ]
        .contains(&tool.name)
        {
            properties.insert("start_time".into(), string());
            properties.insert("end_time".into(), string());
        }
        if tool.name == "get_chat_history" {
            properties.insert("oldest_first".into(), json!({"type":"boolean","default":false,"description":"在全部分片过滤合并后选择最早页；默认选择最新页"}));
            properties.insert("msg_types".into(), json!({"anyOf":[{"type":"null"},{"type":"array","items":string(),"maxItems":100}],"description":"空列表表示全部；多个类型按并集筛选，与 msg_type 互斥"}));
        }
        if tool.name == "search_messages" {
            properties.insert("chat_name".into(), json!({"anyOf":[{"type":"null"},string(),{"type":"array","items":string(),"maxItems":100}]}));
            properties.insert("offset".into(), integer(0, (MAX_CANDIDATES - 1) as i64));
        }
    }
    out
}

fn valid_value(value: &Value, schema: &Value) -> bool {
    if let Some(choices) = schema["anyOf"].as_array() {
        return choices.iter().any(|s| valid_value(value, s));
    }
    if let Some(choices) = schema["enum"].as_array() {
        if !choices.contains(value) {
            return false;
        }
    }
    match schema["type"].as_str() {
        Some("null") => value.is_null(),
        Some("boolean") => value.is_boolean(),
        Some("string") => value.as_str().is_some_and(|s| s.chars().count() <= 4096),
        Some("integer") => value.as_i64().is_some_and(|n| {
            n >= schema["minimum"].as_i64().unwrap() && n <= schema["maximum"].as_i64().unwrap()
        }),
        Some("array") => value
            .as_array()
            .is_some_and(|a| a.len() <= 100 && a.iter().all(|v| valid_value(v, &schema["items"]))),
        _ => false,
    }
}

/// 返回真实 IPC 请求；不接受任意 cmd、导出路径或未注册工具。
pub fn route(name: &str, arguments: &Value) -> Result<Request, &'static str> {
    let tool = tools()
        .into_iter()
        .find(|t| t.name == name)
        .ok_or("Unknown tool")?;
    let args = arguments.as_object().ok_or("Arguments must be an object")?;
    let properties = tool.input_schema["properties"].as_object().unwrap();
    for required in tool.input_schema["required"].as_array().unwrap() {
        if !args.contains_key(required.as_str().unwrap()) {
            return Err("Missing required argument");
        }
    }
    for (key, value) in args {
        if !properties
            .get(key)
            .is_some_and(|schema| valid_value(value, schema))
        {
            return Err("Invalid or unknown argument");
        }
    }
    for key in ["chat_name", "keyword"] {
        if name == "search_messages" && key == "chat_name" {
            continue;
        }
        if args
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|s| s.trim().is_empty())
        {
            return Err("Empty query target");
        }
    }
    let mut mapped = args.clone();
    if name == "get_contacts" {
        mapped.insert("legacy_view".into(), json!(true));
    }
    for (legacy, native, end) in [("start_time", "since", false), ("end_time", "until", true)] {
        if let Some(value) = mapped.remove(legacy) {
            if let Some(timestamp) = parse_legacy_time(value.as_str().unwrap(), end)? {
                if mapped.contains_key(native) {
                    return Err("Conflicting time arguments");
                }
                mapped.insert(native.into(), json!(timestamp));
            }
        }
    }
    if let (Some(since), Some(until)) = (
        mapped.get("since").and_then(Value::as_i64),
        mapped.get("until").and_then(Value::as_i64),
    ) {
        if since > until {
            return Err("since exceeds until");
        }
    }
    if let Some(types) = mapped.remove("msg_types") {
        let mut resolved = Vec::new();
        for item in types.as_array().into_iter().flatten() {
            let kind = match item.as_str().unwrap().trim().to_ascii_lowercase().as_str() {
                "text" => 1,
                "image" => 3,
                "voice" => 34,
                "namecard" => 42,
                "video" => 43,
                "emoji" => 47,
                "location" => 48,
                "app" | "file" => 49,
                "voip" => 50,
                "system" => 10000,
                _ => return Err("Unknown message type"),
            };
            if !resolved.contains(&kind) {
                resolved.push(kind);
            }
        }
        if let Some(kind) = resolved.first() {
            if mapped.contains_key("msg_type") {
                return Err("Conflicting message type arguments");
            }
            if resolved.len() == 1 {
                mapped.insert("msg_type".into(), json!(kind));
            } else {
                mapped.insert("msg_types".into(), json!(resolved));
            }
        }
    }
    if name == "search_messages" {
        if let Some(chats) = mapped.remove("chat_name") {
            if mapped.contains_key("chats") {
                return Err("Conflicting chat arguments");
            }
            let values = if chats.is_string() {
                vec![chats]
            } else {
                chats.as_array().cloned().unwrap_or_default()
            };
            let mut names = Vec::new();
            for value in values {
                let chat = value.as_str().unwrap().trim().to_owned();
                if !chat.is_empty() && !names.contains(&chat) {
                    names.push(chat);
                }
            }
            if !names.is_empty() {
                mapped.insert("chats".into(), json!(names));
            }
        }
        let offset = mapped
            .remove("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let limit = mapped.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
        if offset + limit > MAX_CANDIDATES {
            return Err("Search candidate window exceeds 10000");
        }
        if offset > 0 {
            mapped.insert("limit".into(), json!(offset + limit));
        }
    }
    if name == "get_new_messages" {
        mapped.insert("limit".into(), json!(MAX_CANDIDATES + 1));
    }
    if name == "get_chat_images" {
        mapped.entry("limit").or_insert(json!(20));
        mapped.insert("kinds".into(), json!(["image"]));
        mapped.insert("image_metadata".into(), json!(true));
    }
    if let Some(chat) = mapped.remove("chat_name") {
        mapped.insert("chat".into(), chat);
    }
    mapped.insert("cmd".into(), json!(tool.command));
    serde_json::from_value(Value::Object(mapped)).map_err(|_| "Invalid IPC arguments")
}

/// 与旧 Python 的本机时区规则一致；DST 重叠/空洞不猜测，要求改传 Unix 秒。
fn parse_legacy_time(value: &str, end: bool) -> Result<Option<i64>, &'static str> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let date = if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        if end {
            date.and_hms_opt(23, 59, 59)
        } else {
            date.and_hms_opt(0, 0, 0)
        }
    } else {
        ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"]
            .iter()
            .find_map(|fmt| NaiveDateTime::parse_from_str(value, fmt).ok())
    }
    .ok_or("Invalid legacy date; use YYYY-MM-DD [HH:MM[:SS]]")?;
    Local
        .from_local_datetime(&date)
        .single()
        .map(|d| Some(d.timestamp()))
        .ok_or("Ambiguous or nonexistent local time; use Unix seconds")
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
fn result(id: Value, value: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":value})
}
fn image_metadata(row: &Value) -> Result<Value, DispatchError> {
    let invalid = || DispatchError::InvalidResponse;
    let md5 = row.get("md5").ok_or_else(invalid)?;
    let size = row.get("size").ok_or_else(invalid)?;
    let resource_status = row["resource_status"].as_str().ok_or_else(invalid)?;
    let size_status = row["size_status"].as_str().ok_or_else(invalid)?;
    if !(md5.is_null()
        || md5.as_str().is_some_and(|value| {
            value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        }))
        || !(size.is_null() || size.as_u64().is_some())
        || !matches!(
            resource_status,
            "found" | "missing" | "ambiguous" | "md5_missing" | "unavailable" | "message_ambiguous"
        )
        || !matches!(
            size_status,
            "available" | "missing" | "ambiguous" | "not_requested" | "unavailable"
        )
        || (resource_status == "found") != md5.is_string()
        || (size_status == "available") != size.is_u64()
        || (!size.is_null() && md5.is_null())
        || row["size_kind"] != "encrypted_dat_metadata"
        || row["binding"] != "exact_resource_standard_filename_metadata"
    {
        return Err(invalid());
    }
    Ok(json!({
        "md5": md5,
        "size": size,
        "resource_status": resource_status,
        "size_status": size_status,
        "size_kind": "encrypted_dat_metadata",
        "binding": "exact_resource_standard_filename_metadata"
    }))
}

fn text_result(text: String, is_error: bool) -> Value {
    json!({"content":[{"type":"text","text":text}],"isError":is_error})
}
fn public_failure(failure: &DispatchError) -> &'static str {
    match failure {
        DispatchError::Unavailable => "Query backend unavailable",
        DispatchError::Internal => "Internal error",
        DispatchError::Cancelled => "Query cancelled",
        DispatchError::TimedOut => "Query timed out",
        DispatchError::QueryFailed => "Query failed",
        DispatchError::InvalidResponse => "Invalid query response",
        DispatchError::ResultLimit => "Query result exceeds safe limit",
        DispatchError::InvalidArguments => "Invalid tool arguments",
    }
}
fn valid_id(id: &Value) -> bool {
    id.is_string() || id.as_i64().is_some() || id.as_u64().is_some()
}
fn params_object(value: &Value) -> Option<Map<String, Value>> {
    match value.get("params") {
        None => Some(Map::new()),
        Some(p) => p.as_object().cloned(),
    }
}

pub struct Protocol<D> {
    dispatcher: D,
    phase: Phase,
    session_state: Option<HashMap<String, i64>>,
}

impl<D: Dispatcher> Protocol<D> {
    pub fn new(dispatcher: D) -> Self {
        Self {
            dispatcher,
            phase: Phase::New,
            session_state: None,
        }
    }
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "协议库公开状态观察接口；CLI 内部不读取状态")
    )]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// 一条完整 JSON 消息。所有合法通知静默丢弃，唯 initialized 改变状态。
    /// 未解析输入及无效 envelope 不是通知，返回 null id 的 JSON-RPC 错误。
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "独立协议宿主使用默认上下文入口；CLI 显式绑定帧预算"
        )
    )]
    pub fn handle(&mut self, bytes: &[u8]) -> Option<Value> {
        self.handle_with_context(bytes, &CallContext::default())
    }

    /// 外部 transport 将当前请求 id 与 context 的取消 token 关联；不存历史 id。
    pub fn handle_with_context(&mut self, bytes: &[u8], context: &CallContext) -> Option<Value> {
        let value: Value = match serde_json::from_slice(bytes) {
            Ok(v) => v,
            Err(_) => return Some(error(Value::Null, -32700, "Parse error")),
        };
        let method = value.get("method").and_then(Value::as_str);
        let id = value.get("id");
        if !value.is_object()
            || value["jsonrpc"] != "2.0"
            || method.is_none()
            || id.is_some_and(|id| !valid_id(id))
            || value.get("result").is_some()
            || value.get("error").is_some()
        {
            return Some(error(Value::Null, -32600, "Invalid Request"));
        }
        let method = method.unwrap();
        let params = params_object(&value);
        if id.is_none() {
            if method == "notifications/initialized"
                && self.phase == Phase::AwaitingInitialized
                && params.is_some()
            {
                self.phase = Phase::Ready;
            }
            return None;
        }
        let id = id.unwrap().clone();
        let Some(params) = params else {
            return Some(error(id, -32602, "Invalid params"));
        };
        let reply = match method {
            "initialize" => {
                if self.phase != Phase::New {
                    error(id, -32600, "Already initialized")
                } else if !params.get("protocolVersion").is_some_and(Value::is_string)
                    || !params.get("capabilities").is_some_and(Value::is_object)
                    || !params
                        .get("clientInfo")
                        .is_some_and(|v| v["name"].is_string() && v["version"].is_string())
                {
                    error(id, -32602, "Invalid initialize params")
                } else {
                    // 不支持的客户端版本回报服务器支持版本，由客户端决定是否断开。
                    self.phase = Phase::AwaitingInitialized;
                    result(
                        id,
                        json!({"protocolVersion":PROTOCOL_VERSION,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"wx-cli-native-mcp","version":"0.1.0"}}),
                    )
                }
            }
            "ping" => result(id, json!({})),
            "tools/list" | "tools/call" if self.phase != Phase::Ready => {
                error(id, -32002, "Server not initialized")
            }
            "tools/list" => {
                if params.keys().any(|k| k != "_meta") {
                    error(id, -32602, "Pagination is not supported")
                } else {
                    result(
                        id,
                        json!({"tools": tools().into_iter().chain(self.dispatcher.task_tools()).map(|t| json!({"name":t.name,"description":t.description,"inputSchema":t.input_schema,"annotations":{"readOnlyHint":t.read_only(),"destructiveHint":t.destructive(),"openWorldHint":t.open_world()}})).collect::<Vec<_>>()}),
                    )
                }
            }
            "tools/call" => {
                if params
                    .keys()
                    .any(|k| !["name", "arguments", "_meta"].contains(&k.as_str()))
                {
                    error(id, -32602, "Invalid tool call params")
                } else {
                    let empty = json!({});
                    if let Some(name) = params.get("name").and_then(Value::as_str) {
                        if self
                            .dispatcher
                            .task_tools()
                            .iter()
                            .any(|tool| tool.name == name)
                        {
                            let mut context = context.clone();
                            context.response_id = id.clone();
                            let reply = context
                                .check()
                                .and_then(|()| {
                                    self.dispatcher.dispatch_task(
                                        name,
                                        params.get("arguments").unwrap_or(&empty),
                                        &context,
                                    )
                                })
                                .and_then(|value| {
                                    context.check()?;
                                    Ok(value)
                                });
                            return Some(match reply {
                                Ok(value) => result(id, value),
                                Err(DispatchError::InvalidArguments) => {
                                    error(id, -32602, "Invalid task arguments")
                                }
                                Err(DispatchError::Internal) => error(id, -32603, "Internal error"),
                                Err(DispatchError::TimedOut | DispatchError::Cancelled) => result(
                                    id,
                                    text_result(
                                        "Task tool call ended; an accepted background task is not cancelled. Submission outcome may be unknown: get_task by the original idempotency_key or retry submit_task with the same key and arguments; use cancel_task for explicit cancellation.".into(),
                                        true,
                                    ),
                                ),
                                Err(failure) => {
                                    result(id, text_result(public_failure(&failure).into(), true))
                                }
                            });
                        }
                    }
                    match params
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or("Missing tool name")
                        .and_then(|name| route(name, params.get("arguments").unwrap_or(&empty)))
                    {
                        Err(message) => error(id, -32602, message),
                        Ok(request) => {
                            let name = params["name"].as_str().unwrap();
                            let mut context = context.clone();
                            context.response_id = id.clone();
                            match self.execute_tool(
                                name,
                                params.get("arguments").unwrap_or(&empty),
                                request,
                                &context,
                            ) {
                                Ok(content) => result(id, content),
                                Err(DispatchError::Internal) => error(id, -32603, "Internal error"),
                                Err(failure) => {
                                    result(id, text_result(public_failure(&failure).into(), true))
                                }
                            }
                        }
                    }
                }
            }
            _ => error(id, -32601, "Method not found"),
        };
        Some(reply)
    }

    fn execute_tool(
        &mut self,
        name: &str,
        args: &Value,
        request: Request,
        context: &CallContext,
    ) -> Result<Value, DispatchError> {
        context.check()?;
        let response = self.dispatcher.dispatch(request, context)?;
        // 不合作 callback 的迟到结果也不能伪装成按时成功。
        context.check()?;
        if !response.ok || response.error.is_some() {
            return Err(DispatchError::QueryFailed);
        }
        let mut data = response.data;
        // 解码查询可能将业务失败包在 ok 响应的 exit_code/text 中，统一隐藏底层错误。
        if data.get("exit_code").is_some_and(|v| v.as_i64() != Some(0))
            || data.get("error").is_some_and(|v| !v.is_null())
        {
            return Err(DispatchError::QueryFailed);
        }
        if matches!(name, "decode_voice" | "transcribe_voice") {
            // 仅交付宿主完成解码或识别后的文本；准备音频绝不能成为公开成功。
            let text = data
                .get("mcp_text")
                .and_then(Value::as_str)
                .ok_or(DispatchError::InvalidResponse)?;
            return Ok(text_result(text.to_owned(), false));
        }
        if name == "get_new_messages" {
            return self.poll_sessions(data, context);
        }
        if name == "get_chat_images" {
            let attachments = data["attachments"]
                .as_array()
                .ok_or(DispatchError::InvalidResponse)?;
            let mut images = Vec::new();
            for row in attachments {
                let local_id = row["local_id"]
                    .as_i64()
                    .ok_or(DispatchError::InvalidResponse)?;
                let ts = row["timestamp"]
                    .as_i64()
                    .ok_or(DispatchError::InvalidResponse)?;
                // 白名单只包含已验证元数据；不回传路径、附件句柄或 packed_info。
                let mut image = image_metadata(row)?;
                image["local_id"] = json!(local_id);
                image["create_time"] = json!(ts);
                images.push(image);
            }
            return Ok(text_result(json!({"images":images,"count":images.len(),"partial_legacy_compatibility":true,"note":"Exact resource matching; ambiguous DAT candidates have no selected size. No image decoding or upload."}).to_string(), false));
        }
        if name == "search_messages" {
            let offset = args["offset"].as_u64().unwrap_or(0) as usize;
            if offset > 0 {
                let rows = data["results"]
                    .as_array()
                    .ok_or(DispatchError::InvalidResponse)?;
                let limit = args["limit"].as_u64().unwrap_or(20) as usize;
                let paged: Vec<Value> = rows.iter().skip(offset).take(limit).cloned().collect();
                data["count"] = json!(paged.len());
                data["results"] = json!(paged);
            }
        }
        context.check()?;
        Ok(text_result(data.to_string(), false))
    }

    fn poll_sessions(
        &mut self,
        data: Value,
        context: &CallContext,
    ) -> Result<Value, DispatchError> {
        let rows = data["sessions"]
            .as_array()
            .ok_or(DispatchError::InvalidResponse)?;
        if rows.len() > MAX_CANDIDATES {
            return Err(DispatchError::ResultLimit);
        }
        let first = self.session_state.as_ref().is_none_or(HashMap::is_empty);
        let mut next = HashMap::new();
        let mut entries = Vec::new();
        for row in rows {
            let username = row["username"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 4096)
                .ok_or(DispatchError::InvalidResponse)?;
            let ts = row["timestamp"]
                .as_i64()
                .ok_or(DispatchError::InvalidResponse)?;
            let unread = row["unread"]
                .as_i64()
                .ok_or(DispatchError::InvalidResponse)?;
            let chat = row["chat"].as_str().ok_or(DispatchError::InvalidResponse)?;
            let summary = row["summary"]
                .as_str()
                .ok_or(DispatchError::InvalidResponse)?;
            let kind = row["last_msg_type"]
                .as_str()
                .ok_or(DispatchError::InvalidResponse)?;
            let sender = row["last_sender"]
                .as_str()
                .ok_or(DispatchError::InvalidResponse)?;
            let group = row["is_group"]
                .as_bool()
                .ok_or(DispatchError::InvalidResponse)?;
            if next.insert(username.to_owned(), ts).is_some() {
                return Err(DispatchError::InvalidResponse);
            }
            let changed = if first {
                unread > 0
            } else {
                ts > self
                    .session_state
                    .as_ref()
                    .and_then(|s| s.get(username))
                    .copied()
                    .unwrap_or(0)
            };
            if !changed {
                continue;
            }
            let date = Local
                .timestamp_opt(ts, 0)
                .single()
                .ok_or(DispatchError::InvalidResponse)?;
            let entry = if first {
                format!(
                    "[{}] {}{} ({}条未读): {}",
                    date.format("%H:%M"),
                    chat,
                    if group { "[群]" } else { "" },
                    unread,
                    summary
                )
            } else {
                let sender = if group && !sender.is_empty() {
                    format!(" ({sender})")
                } else {
                    String::new()
                };
                format!(
                    "[{}] {}{}: {}{} - {}",
                    date.format("%H:%M:%S"),
                    chat,
                    if group { " [群]" } else { "" },
                    kind,
                    sender,
                    summary
                )
            };
            entries.push((ts, entry));
        }
        if !first {
            entries.sort_by_key(|(ts, _)| *ts);
        }
        let text = if entries.is_empty() {
            if first {
                "当前无未读消息（已记录状态，下次调用将返回新消息）".into()
            } else {
                "无新消息".into()
            }
        } else {
            let heading = if first {
                format!("当前 {} 个未读会话", entries.len())
            } else {
                format!("{} 条新消息", entries.len())
            };
            format!(
                "{heading}:\n\n{}",
                entries
                    .into_iter()
                    .map(|(_, s)| s)
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        context.check()?;
        self.session_state = Some(next);
        Ok(text_result(text, false))
    }

    /// stdio 是 UTF-8 NDJSON，不是 LSP Content-Length。上限不含 LF、包含 CR。
    /// 超长帧立即返回 I/O 错误并关闭会话；不继续消费攻击者的无限输入。
    /// 输出先限长序列化，避免长度错误时已向客户端写入半条 JSON。
    pub fn serve<R: BufRead, W: Write>(
        &mut self,
        mut reader: R,
        mut writer: W,
        max_bytes: usize,
    ) -> io::Result<()> {
        if max_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "zero frame limit",
            ));
        }
        loop {
            let Some(frame) = read_frame(&mut reader, max_bytes)? else {
                return Ok(());
            };
            let previous_state = self.session_state.clone();
            let context = CallContext {
                max_response_bytes: max_bytes,
                ..CallContext::default()
            };
            if let Some(reply) = self.handle_with_context(&frame, &context) {
                let written = (|| -> io::Result<()> {
                    let mut bounded = LimitedWriter {
                        bytes: Vec::new(),
                        limit: max_bytes,
                    };
                    serde_json::to_writer(&mut bounded, &reply).map_err(io::Error::other)?;
                    writer.write_all(&bounded.bytes)?;
                    writer.write_all(b"\n")?;
                    writer.flush()
                })();
                if let Err(error) = written {
                    // 输出失败不能让未交付的新消息悄悄变成已读游标。
                    self.session_state = previous_state;
                    return Err(error);
                }
            }
        }
    }
}

fn read_frame<R: BufRead>(reader: &mut R, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "unterminated MCP frame",
                ))
            };
        }
        let newline = chunk.iter().position(|b| *b == b'\n');
        let count = newline.unwrap_or(chunk.len());
        if count > limit.saturating_sub(frame.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MCP frame exceeds limit",
            ));
        }
        frame.extend_from_slice(&chunk[..count]);
        reader.consume(count + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(Some(frame));
        }
    }
}

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MCP response exceeds limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
