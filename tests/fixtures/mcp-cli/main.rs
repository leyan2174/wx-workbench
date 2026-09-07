use clap::Parser;
use wx_mcp_cli_harness::{cli_mcp as mcp, runtime::RuntimeContext};

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    args: mcp::McpArgs,
    /// 仅用于合成过程测试生成独立管道名，不属于实际 wx 参数。
    #[arg(long, hide = true)]
    fixture_pipe: bool,
}

fn main() {
    // 测试绝不启动实际 daemon；若 mock 没有监听，立即失败而不是递归拉起自己。
    if std::env::var_os("WX_DAEMON_MODE").is_some() {
        std::process::exit(3);
    }
    let cli = Cli::parse();
    if cli.fixture_pipe {
        match RuntimeContext::load() {
            Ok(runtime) => println!("{}", runtime.pipe_name()),
            Err(_) => std::process::exit(2),
        }
        return;
    }
    if let Err(error) = mcp::cmd(cli.args) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
