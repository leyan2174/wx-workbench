use clap::Parser;
#[derive(Parser)]
struct HostProbeArgs {
    #[command(flatten)]
    args: mcp_image_security::host::McpArgs,
}
fn main() {
    let args = HostProbeArgs::parse();
    let config = std::path::PathBuf::from(std::env::var_os("WX_CLI_CONFIG").unwrap());
    let home = config.parent().unwrap().join("authenticated-runtime");
    std::env::set_var("WX_CLI_HOME", &home);
    let runtime = wx_mcp_cli_harness::fixture_runtime(&config, &home).unwrap();
    let mock = wx_mcp_cli_harness::authenticated_mock::Mock::start(runtime, |request| {
        if let Some(path) = std::env::var_os("AUDIT_CAPTURE_REQUEST") {
            std::fs::write(path, serde_json::to_vec(request).unwrap()).unwrap();
        }
        let response = if std::env::var_os("AUDIT_SECRET_ERROR").is_some() {
            serde_json::json!({"ok":false,"error":"SYNTHETIC_SECRET_KEY_abcdef012345"})
        } else {
            serde_json::json!({"ok":true,"exit_code":0,"status":"published","image":{"path":"synthetic.jpg"}})
        };
        wx_mcp_cli_harness::authenticated_mock::Reply::Json(response)
    });
    if let Err(error) = mcp_image_security::host::cmd(args.args) {
        eprintln!("{error}");
        std::process::exit(1);
    }
    mock.finish();
}
