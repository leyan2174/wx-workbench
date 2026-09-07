use clap::Parser;
#[derive(Parser)]
struct Args {
    #[command(flatten)]
    args: mcp_image_security::host::McpArgs,
}
fn main() {
    if let Err(error) = mcp_image_security::host::cmd(Args::parse().args) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
