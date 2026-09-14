use clap::Parser;
#[derive(Parser)]
struct Command {
    #[command(flatten)]
    args: mcp_voice_host_security::cli_mcp::McpArgs,
}
fn main() {
    use mcp_voice_host_security::{
        authenticated_mock::{Mock, Reply},
        runtime::RuntimeContext,
    };
    let args = Command::parse().args;
    let mock = RuntimeContext::load().ok().map(|runtime| {
        Mock::start(runtime, |request| {
            eprintln!("AUDIT_EVENT:ipc:{request}");
            if request["cmd"] == "resolve_chat" {
                assert_eq!(request["chat"], "synthetic-peer");
                return Reply::Json(serde_json::json!({"ok":true,"username":"synthetic-peer"}));
            }
            let bytes = std::fs::read(std::env::var_os("AUDIT_RESPONSE").unwrap()).unwrap();
            Reply::Raw(bytes)
        })
    });
    let result = mcp_voice_host_security::cli_mcp::cmd(args);
    if let Some(mock) = mock {
        mock.finish();
    }
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
