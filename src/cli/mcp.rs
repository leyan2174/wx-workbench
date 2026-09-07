//! MCP stdio 入口：只适配现有账号隔离 transport，不发现账号、不读取数据库或密钥。
//!
//! 使用根 mcp 协议模块；不经过 print_response/emit_warnings。运行时显式设置 WX_CLI_CONFIG。
//!
//! 查询与媒体工具的接线状态见 src/mcp/PROTOCOL.md；媒体输出由宿主显式配置。
//! 同步 serve 不能在 callback 中读取取消通知；30秒 context 只丢弃迟到结果，
//! IPC 使用 context 剩余时限，后台启动仍受现有独立启动时限约束。
//! MCP 输入/输出与 IPC 响应均有字节上限，不复制后台生命周期逻辑。

use super::{mcp_voice, transport};
use crate::{ipc::Request, runtime::RuntimeContext};
use anyhow::{anyhow, Result};
use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead, Read, Write},
    path::PathBuf,
};

use crate::mcp::protocol;

use protocol::{CallContext, Controlled, DispatchError, Dispatcher, Protocol};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

#[derive(clap::Args, Debug, Clone)]
pub struct McpArgs {
    /// 单条 MCP 输入或输出的字节上限，不含结尾 LF
    #[arg(long, default_value_t = protocol::DEFAULT_MAX_FRAME_BYTES as u32,
        value_parser = clap::value_parser!(u32).range(1024..=MAX_FRAME_BYTES as i64))]
    pub max_frame_bytes: u32,
    /// 图片输出根：必须已存在且可信；未配置时图片调用在账号访问前拒绝
    #[arg(long)]
    pub media_output_root: Option<PathBuf>,
    /// 显式图片 AES/XOR 配置文件；不自动扫描或发现密钥
    #[arg(long, requires = "media_output_root")]
    pub image_key_file: Option<PathBuf>,
    /// 允许使用固定账号配置明确选定的 local Python Whisper；不接受工具请求指定后端。
    #[arg(long, conflicts_with_all = ["whisper_binary", "whisper_model", "allow_upload",
        "openai_base_url", "openai_model", "api_key_file", "temp_root"])]
    pub configured_local_python: bool,
    #[command(flatten)]
    pub voice: mcp_voice::Args,
}

impl Default for McpArgs {
    fn default() -> Self {
        Self {
            max_frame_bytes: protocol::DEFAULT_MAX_FRAME_BYTES as u32,
            media_output_root: None,
            image_key_file: None,
            configured_local_python: false,
            voice: mcp_voice::Args::default(),
        }
    }
}

/// 不打印欢迎语、成功行或底层错误链；返回给主 CLI 的错误也只含安全固定文本。
pub fn cmd(args: McpArgs) -> Result<()> {
    let max_response_bytes = args.max_frame_bytes as usize;
    let policy = args.clone();
    let mut account = None;
    let mut invalidated = false;
    let dispatcher = Controlled(move |mut request: Request, context: &CallContext| {
        context.check()?;
        if invalidated {
            return Err(DispatchError::Unavailable);
        }
        policy.prepare_request(&mut request)?;
        // 宿主授权先于账号访问；daemon 只负责准备音频，不执行后端或发布 WAV。
        let mut voice = match &request {
            Request::DecodeVoice { local_id, .. } => Some(policy.voice.prepare(
                mcp_voice::Operation::Decode,
                *local_id,
                policy.media_output_root.as_deref(),
                context,
            )?),
            Request::TranscribeVoice { local_id, .. } => Some(if policy.configured_local_python {
                policy.voice.prepare_configured_local(*local_id, context)?
            } else {
                policy.voice.prepare(
                    mcp_voice::Operation::Transcribe,
                    *local_id,
                    policy.media_output_root.as_deref(),
                    context,
                )?
            }),
            _ => None,
        };
        if account.is_none() {
            account = Some(PinnedAccount::open().map_err(|_| DispatchError::Unavailable)?);
        }
        let pinned = account.as_ref().unwrap();
        if !pinned.is_current() {
            invalidated = true;
            return Err(DispatchError::Unavailable);
        }
        if let Some(pending) = voice.take() {
            voice = Some(pending.bind(&pinned.context)?);
        }
        context.check()?;
        if let Request::TranscribeVoice { chat, .. } = &mut request {
            if policy.voice.voice_cache_file.is_some() {
                let current = || {
                    context.check()?;
                    if pinned.is_current() {
                        Ok(())
                    } else {
                        Err(DispatchError::Unavailable)
                    }
                };
                let pending = voice.as_mut().ok_or(DispatchError::Unavailable)?;
                // 精确 username 可在联系人或源音频删除后直接命中；显示名不查历史别名。
                if let Some(response) = pending.try_cached(chat, context, current)? {
                    return Ok(response);
                }
                let resolved = transport::send_with_limits(
                    &pinned.context,
                    Request::ResolveChat { chat: chat.clone() },
                    context.remaining(),
                    8192,
                )
                .map_err(|_| DispatchError::Unavailable)?;
                current()?;
                if !resolved.ok
                    || resolved.error.is_some()
                    || resolved
                        .data
                        .get("exit_code")
                        .is_some_and(|value| value.as_i64() != Some(0))
                    || resolved
                        .data
                        .get("error")
                        .is_some_and(|value| !value.is_null())
                {
                    return Err(DispatchError::QueryFailed);
                }
                let username = resolved.data["username"]
                    .as_str()
                    .filter(|name| !name.trim().is_empty() && name.len() <= 4096)
                    .ok_or(DispatchError::InvalidResponse)?
                    .to_owned();
                pending.bind_username(username.clone())?;
                if username != *chat {
                    if let Some(response) = pending.try_cached(&username, context, current)? {
                        return Ok(response);
                    }
                }
                *chat = username;
            }
        }
        // 复用后台生命周期和命名管道发送；不记录 error 的路径、message 或密钥。
        let response = transport::send_with_limits(
            &pinned.context,
            request,
            context.remaining(),
            if voice.is_some() {
                crate::ipc::MAX_PREPARED_VOICE_RESPONSE_BYTES
            } else {
                max_response_bytes
            },
        );
        if !pinned.is_current() {
            invalidated = true;
            return Err(DispatchError::Unavailable);
        }
        context.check()?;
        let response = response.map_err(|_| DispatchError::Unavailable)?;
        match voice {
            Some(voice) => voice.finish(response, &pinned.context, context, || {
                context.check()?;
                if pinned.is_current() {
                    Ok(())
                } else {
                    Err(DispatchError::Unavailable)
                }
            }),
            None => Ok(response),
        }
    });
    serve_with(args, io::stdin().lock(), io::stdout().lock(), dispatcher)
}

impl McpArgs {
    fn prepare_request(&self, request: &mut Request) -> std::result::Result<(), DispatchError> {
        if let Request::DecodeImage {
            output_root,
            image_key_file,
            ..
        } = request
        {
            let root = self
                .media_output_root
                .as_deref()
                .ok_or(DispatchError::Unavailable)?;
            let root = host_path(root)?;
            if !root.is_dir() {
                return Err(DispatchError::Unavailable);
            }
            *output_root = root.to_str().ok_or(DispatchError::Unavailable)?.to_owned();
            *image_key_file = self
                .image_key_file
                .as_deref()
                .map(|path| {
                    host_path(path)?
                        .to_str()
                        .map(str::to_owned)
                        .ok_or(DispatchError::Unavailable)
                })
                .transpose()?;
        }
        Ok(())
    }
}

fn host_path(path: &std::path::Path) -> std::result::Result<PathBuf, DispatchError> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| part == std::path::Component::ParentDir)
    {
        return Err(DispatchError::Unavailable);
    }
    std::path::absolute(path).map_err(|_| DispatchError::Unavailable)
}

/// 可注入 I/O 与查询器的同一条适配路径，专属合成测试不需要真实数据库。
pub fn serve_with<R: BufRead, W: Write, D: Dispatcher>(
    args: McpArgs,
    reader: R,
    writer: W,
    dispatcher: D,
) -> Result<()> {
    if !(1024..=MAX_FRAME_BYTES).contains(&args.max_frame_bytes) {
        return Err(anyhow!(
            "MCP frame limit must be between 1024 and 16777216 bytes"
        ));
    }
    Protocol::new(dispatcher)
        .serve(reader, writer, args.max_frame_bytes as usize)
        .map_err(|_| anyhow!("MCP stdio transport failed"))
}

/// 持有配置读锁直到 stdio 会话结束，阻止普通写入/替换将轮询游标带到另一账号。
/// 文件锁不替代操作系统账户权限；不保证抵御特权进程更换祖先目录联接。
struct PinnedAccount {
    _config_lock: File,
    context: RuntimeContext,
}

impl PinnedAccount {
    fn open() -> Result<Self> {
        let selected = std::env::var_os("WX_CLI_CONFIG")
            .filter(|p| !p.is_empty())
            .ok_or_else(|| anyhow!("Explicit MCP account configuration required"))?;
        let path = std::path::absolute(PathBuf::from(selected))?;
        let mut file = open_config_read_lock(&path)?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(anyhow!("MCP account configuration exceeds limit"));
        }
        let config: serde_json::Value = serde_json::from_slice(&bytes)?;
        if !config
            .get("db_dir")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|p| !p.trim().is_empty())
        {
            return Err(anyhow!("MCP account requires an explicit db_dir"));
        }
        // 验证 db_dir 已明确给出；不调用 Config 的默认目录探测或解密函数。
        let context = RuntimeContext::load()?;
        if context.config_path != path.canonicalize()? {
            return Err(anyhow!("MCP account configuration changed"));
        }
        Ok(Self {
            _config_lock: file,
            context,
        })
    }

    fn is_current(&self) -> bool {
        RuntimeContext::load().is_ok_and(|current| same_account(&self.context, &current))
    }
}

fn same_account(a: &RuntimeContext, b: &RuntimeContext) -> bool {
    a.id == b.id
        && a.config_path == b.config_path
        && a.root == b.root
        && a.config.db_dir == b.config.db_dir
        && a.config.keys_file == b.config.keys_file
        && a.config.decrypted_dir == b.config.decrypted_dir
        && a.config.wechat_process == b.config.wechat_process
}

fn open_config_read_lock(path: &std::path::Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ：其他读取不受影响，编辑配置须先退出 MCP 会话。
    OpenOptions::new().read(true).share_mode(1).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::Response;
    use serde_json::{json, Value};
    use std::io::Cursor;

    #[derive(clap::Parser)]
    struct TestCli {
        #[command(flatten)]
        args: McpArgs,
    }

    #[test]
    fn configured_local_python_is_an_explicit_host_flag() {
        use clap::Parser;
        assert!(!TestCli::try_parse_from(["mcp"]).unwrap().args.configured_local_python);
        let args = TestCli::try_parse_from([
            "mcp", "--configured-local-python", "--language", "zh", "--threads", "2",
            "--voice-cache-file", "synthetic-cache.json",
        ]).unwrap().args;
        assert!(args.configured_local_python);
        assert_eq!(args.voice.backend.language, "zh");
        assert_eq!(args.voice.backend.threads, Some(2));
    }

    #[test]
    fn configured_local_python_rejects_cloud_and_cpp_host_flags() {
        use clap::Parser;
        for extra in [
            vec!["--allow-upload"], vec!["--whisper-binary", "synthetic.exe"],
            vec!["--whisper-model", "synthetic.bin"], vec!["--api-key-file", "synthetic.key"],
            vec!["--openai-base-url", "https://example.invalid"], vec!["--openai-model", "synthetic"],
            vec!["--temp-root", "synthetic-work"],
        ] {
            let mut argv = vec!["mcp", "--configured-local-python"];
            argv.extend(extra);
            assert!(TestCli::try_parse_from(argv).is_err());
        }
    }

    #[test]
    fn args_accept_default_and_reject_unbounded_limits() {
        use clap::Parser;
        assert_eq!(
            TestCli::try_parse_from(["mcp"])
                .unwrap()
                .args
                .max_frame_bytes,
            1024 * 1024
        );
        for value in ["0", "1023", "16777217", "-1"] {
            assert!(TestCli::try_parse_from(["mcp", "--max-frame-bytes", value]).is_err());
        }
    }

    #[test]
    fn image_host_policy_is_explicit_and_does_not_create_paths() {
        use clap::Parser;
        let root = tempfile::tempdir().unwrap();
        let image = || Request::DecodeImage {
            chat: "peer".into(),
            local_id: 7,
            create_time: 0,
            output_root: "untrusted".into(),
            image_key_file: Some("untrusted-key".into()),
        };
        assert_eq!(
            McpArgs::default().prepare_request(&mut image()),
            Err(DispatchError::Unavailable)
        );
        assert!(McpArgs::default()
            .prepare_request(&mut Request::Ping)
            .is_ok());
        assert!(TestCli::try_parse_from(["mcp", "--image-key-file", "key.json"]).is_err());
        let args = McpArgs {
            media_output_root: Some(root.path().into()),
            image_key_file: Some(root.path().join("key.json")),
            ..McpArgs::default()
        };
        let mut request = image();
        args.prepare_request(&mut request).unwrap();
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["output_root"], root.path().to_str().unwrap());
        assert_eq!(
            value["image_key_file"],
            root.path().join("key.json").to_str().unwrap()
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        for invalid in [
            root.path().join("missing"),
            root.path().join(".."),
            PathBuf::new(),
        ] {
            let args = McpArgs {
                media_output_root: Some(invalid),
                ..McpArgs::default()
            };
            assert_eq!(
                args.prepare_request(&mut image()),
                Err(DispatchError::Unavailable)
            );
        }
    }

    #[test]
    fn handshake_list_and_notifications_do_not_call_ipc() {
        let frames = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"synthetic","version":"1"},"capabilities":{}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"get_contacts"}}),
        ];
        let input = frames.iter().map(|v| format!("{v}\n")).collect::<String>();
        let mut output = Vec::new();
        serve_with(
            McpArgs::default(),
            Cursor::new(input),
            &mut output,
            |_: Request| -> std::result::Result<Response, DispatchError> {
                panic!("unexpected IPC")
            },
        )
        .unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(replies.len(), 2);
        let tools = replies[1]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 17);
        for added in ["decode_file_message", "decode_record_item"] {
            let tool = tools.iter().find(|t| t["name"] == added).unwrap();
            assert_eq!(tool["annotations"]["readOnlyHint"], true);
            assert_eq!(tool["annotations"]["destructiveHint"], false);
        }
        let image = tools
            .iter()
            .find(|tool| tool["name"] == "decode_image")
            .unwrap();
        assert_eq!(image["annotations"]["readOnlyHint"], false);
        assert_eq!(image["annotations"]["destructiveHint"], false);
        for name in ["decode_voice", "transcribe_voice"] {
            let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
            let properties = tool["inputSchema"]["properties"].as_object().unwrap();
            for private in ["configured_local_python", "local_whisper_model", "python_binary", "api_key_file"] {
                assert!(!properties.contains_key(private));
            }
            assert_eq!(tool["annotations"]["readOnlyHint"], false);
            assert_eq!(tool["annotations"]["destructiveHint"], false);
            assert_eq!(
                tool["annotations"]["openWorldHint"],
                name == "transcribe_voice"
            );
        }
    }

    #[test]
    fn framing_errors_are_safe_and_eof_is_preserved() {
        let mut output = Vec::new();
        assert!(serve_with(
            McpArgs::default(),
            Cursor::new(b""),
            &mut output,
            |_: Request| Ok(Response::ok(json!({})))
        )
        .is_ok());
        assert!(output.is_empty());
        let error = serve_with(
            McpArgs::default(),
            Cursor::new(b"SECRET-no-newline"),
            &mut output,
            |_: Request| Ok(Response::ok(json!({}))),
        )
        .unwrap_err();
        assert_eq!(format!("{error:#}"), "MCP stdio transport failed");
        assert!(output.is_empty());
        let oversized = vec![b'x'; 1025];
        let error = serve_with(
            McpArgs {
                max_frame_bytes: 1024,
                ..McpArgs::default()
            },
            Cursor::new(oversized),
            &mut output,
            |_: Request| Ok(Response::ok(json!({}))),
        )
        .unwrap_err();
        assert_eq!(format!("{error:#}"), "MCP stdio transport failed");
        assert!(output.is_empty());
    }

    #[test]
    fn cancelled_callback_boundary_is_not_bypassed() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"},\"capabilities\":{}}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_contacts\"}}\n"
        );
        let mut output = Vec::new();
        serve_with(
            McpArgs::default(),
            Cursor::new(input),
            &mut output,
            Controlled(|_: Request, ctx: &CallContext| {
                ctx.cancellation().cancel();
                Ok(Response::ok(json!({"secret":"not-delivered"})))
            }),
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Query cancelled"));
        assert!(!text.contains("not-delivered"));
    }
}
