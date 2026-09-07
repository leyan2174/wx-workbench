use clap::Parser;
#[derive(Parser)]
struct Command {
    #[command(flatten)]
    args: mcp_voice_host_security::cli_mcp::McpArgs,
}
fn main() {
    if let Err(error) = mcp_voice_host_security::cli_mcp::cmd(Command::parse().args) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
