mod config;
mod ipc;
mod crypto;
mod scanner;
mod daemon;
mod cli;
mod attachment;
#[cfg(windows)]
mod windows_process;

fn main() {
    #[cfg(windows)]
    if let Err(error) = windows_process::isolate_standard_handles() {
        eprintln!("Unable to isolate inherited standard handles: {error}");
        std::process::exit(1);
    }
    if std::env::var("WX_DAEMON_MODE").is_ok() {
        daemon::run();
    } else {
        cli::run();
    }
}
