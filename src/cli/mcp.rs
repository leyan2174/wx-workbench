// MCP stdio framing, fixed-account binding and daemon transport; no task execution.
use super::operation_args::mcp_voice;
use super::{mcp_tasks, transport};
use crate::mcp::protocol;
use crate::{
    ipc::Request,
    service::mcp::{Call, HostSettings},
};
use anyhow::{anyhow, Result};
use protocol::{CallContext, Controlled, DispatchError, Dispatcher, Protocol};
use std::{
    cell::Cell,
    io::{self, BufRead, Write},
    path::PathBuf,
};
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
    #[command(flatten)]
    pub task: mcp_tasks::Args,
}

impl Default for McpArgs {
    fn default() -> Self {
        Self {
            max_frame_bytes: protocol::DEFAULT_MAX_FRAME_BYTES as u32,
            media_output_root: None,
            image_key_file: None,
            configured_local_python: false,
            voice: mcp_voice::Args::default(),
            task: mcp_tasks::Args::default(),
        }
    }
}

impl McpArgs {
    fn host_settings(&self) -> HostSettings {
        HostSettings {
            media_output_root: self.media_output_root.clone(),
            image_key_file: self.image_key_file.clone(),
            configured_local_python: self.configured_local_python,
            voice: self.voice.clone().into(),
        }
    }
}

/// Capture relative host paths against the stdio process, not the daemon cwd.
fn absolute_host_settings(mut host: HostSettings) -> Result<HostSettings> {
    let cwd = std::env::current_dir()?;
    for path in [
        &mut host.media_output_root,
        &mut host.image_key_file,
        &mut host.voice.backend.whisper_binary,
        &mut host.voice.backend.whisper_model,
        &mut host.voice.backend.api_key_file,
        &mut host.voice.backend.temp_root,
        &mut host.voice.voice_cache_file,
    ]
    .into_iter()
    .flatten()
    {
        if !path.as_os_str().is_empty() && path.is_relative() {
            *path = cwd.join(&*path);
        }
    }
    Ok(host)
}

pub fn cmd(mut args: McpArgs) -> Result<()> {
    // Keep IOCP cancellation progressing while the stdio thread waits for input.
    let io_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|_| anyhow!("MCP transport initialization failed"))?;
    let host = absolute_host_settings(args.host_settings())
        .map_err(|_| anyhow!("MCP host transport configuration failed"))?;
    args.task
        .absolute_paths()
        .map_err(|_| anyhow!("MCP task host path invalid"))?;
    let account = mcp_tasks::Account::new(args.task.tasks);
    let session = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| anyhow!("MCP session creation failed"))?
            .as_nanos()
    );
    let invalidated = Cell::new(false);
    let mut opened = false;
    let dispatcher = Controlled(|request: Request, context: &CallContext| {
        context.check()?;
        if invalidated.get() {
            return Err(DispatchError::Unavailable);
        }
        // 查询与任务共用第一次业务调用固定的账号；查询会话仍由 daemon 校验。
        let pinned = account.get()?;
        let call = Call {
            session: session.clone(),
            open_session: !opened,
            owner_pid: std::process::id(),
            runtime_id: pinned.id.clone(),
            host: host.clone(),
            budget: context.budget()?,
            request: Some(Box::new(request)),
        };
        let response = (|| {
            transport::ensure_running_quiet(pinned)?;
            context.check().map_err(|_| anyhow!("MCP call expired"))?;
            io_runtime.block_on(async {
                crate::service::client::wait_ready(pinned).await?;
                crate::service::client::request_with_timeout(
                    pinned,
                    crate::service::protocol::Call::Mcp {
                        request: Box::new(call),
                    },
                    context.remaining(),
                )
                .await
            })
        })();
        context.check()?;
        match response {
            Ok(value) => {
                opened = true;
                let response =
                    serde_json::from_value(value).map_err(|_| DispatchError::InvalidResponse)?;
                crate::service::mcp::unpack(response)
            }
            Err(_) => {
                invalidated.set(true);
                Err(DispatchError::Unavailable)
            }
        }
    });
    let result = serve_with(
        args.clone(),
        io::stdin().lock(),
        io::stdout().lock(),
        mcp_tasks::Adapter {
            query: dispatcher,
            account: &account,
            invalidated: &invalidated,
            io: &io_runtime,
            args: &args.task,
        },
    );
    if let Some(runtime) = account.runtime() {
        let context = CallContext::new(
            protocol::CancellationToken::default(),
            std::time::Duration::from_secs(1),
        );
        if let Ok(budget) = context.budget() {
            let _ = io_runtime.block_on(crate::service::client::request_with_timeout(
                runtime,
                crate::service::protocol::Call::Mcp {
                    request: Box::new(Call {
                        session,
                        open_session: false,
                        owner_pid: std::process::id(),
                        runtime_id: runtime.id.clone(),
                        host,
                        budget,
                        request: None,
                    }),
                },
                context.remaining(),
            ));
        }
    }
    result
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
        assert!(
            !TestCli::try_parse_from(["mcp"])
                .unwrap()
                .args
                .configured_local_python
        );
        let args = TestCli::try_parse_from([
            "mcp",
            "--configured-local-python",
            "--language",
            "zh",
            "--threads",
            "2",
            "--voice-cache-file",
            "synthetic-cache.json",
        ])
        .unwrap()
        .args;
        assert!(args.configured_local_python);
        assert_eq!(args.voice.backend.language, "zh");
        assert_eq!(args.voice.backend.threads, Some(2));
    }

    #[test]
    fn configured_local_python_rejects_cloud_and_cpp_host_flags() {
        use clap::Parser;
        for extra in [
            vec!["--allow-upload"],
            vec!["--whisper-binary", "synthetic.exe"],
            vec!["--whisper-model", "synthetic.bin"],
            vec!["--api-key-file", "synthetic.key"],
            vec!["--openai-base-url", "https://example.invalid"],
            vec!["--openai-model", "synthetic"],
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
    fn image_key_argument_requires_output_root() {
        use clap::Parser;
        assert!(TestCli::try_parse_from(["mcp", "--image-key-file", "key.json"]).is_err());
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
            for private in [
                "configured_local_python",
                "local_whisper_model",
                "python_binary",
                "api_key_file",
            ] {
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
