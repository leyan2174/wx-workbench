mod attachment;
mod cli;
mod config;
mod crypto;
mod daemon;
mod ipc;
mod mcp;
mod message;
mod runtime;
mod scanner;
mod service;
mod toolkit;
#[cfg(windows)]
mod windows_process;

fn main() {
    #[cfg(windows)]
    if let Err(error) = windows_process::isolate_standard_handles() {
        eprintln!("Unable to isolate inherited standard handles: {error}");
        std::process::exit(1);
    }
    if std::env::var("WX_DAEMON_TASK_WORKER").as_deref() == Ok("1") {
        if let Err(error) = cli::task_worker::run() {
            eprintln!("任务执行失败：{error:#}");
            std::process::exit(1);
        }
    } else if std::env::var("WX_DAEMON_MODE").is_ok() {
        daemon::run();
    } else if env!("CARGO_BIN_NAME") == "wx-toolbox" {
        cli::run_toolbox();
    } else {
        cli::run();
    }
}
