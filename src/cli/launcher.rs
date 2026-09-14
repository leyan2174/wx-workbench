//! 双模式启动器只转换白名单入口，不执行任意脚本，也不改变工作目录。
use std::ffi::OsString;

pub(super) fn prepare_first_run() -> anyhow::Result<bool> {
    use std::io::IsTerminal;
    match crate::service::operation_client::run(
        crate::service::operations::Operation::FirstRunCheck,
    ) {
        Ok(()) => return Ok(true),
        Err(error)
            if error
                .downcast_ref::<crate::service::operation_client::OperationExit>()
                .is_some_and(|exit| exit.0 == 10) => {}
        Err(error) => return Err(error),
    }
    anyhow::ensure!(std::io::stdin().is_terminal() && std::io::stderr().is_terminal(),
        "尚未配置账号；请先运行 wx toolkit setup --interactive --apply，或使用显式参数 --apply --yes");
    super::setup_native::submit(super::setup_native::Args {
        interactive: true,
        apply: true,
        ..Default::default()
    })
}

pub(super) fn arguments(mut args: Vec<OsString>) -> Vec<OsString> {
    let program = if args.is_empty() {
        OsString::from("wx-toolbox")
    } else {
        args.remove(0)
    };
    if args.first().and_then(|value| value.to_str()) == Some("main.py") {
        args.remove(0);
    }
    let command = args.first().and_then(|value| value.to_str());
    let prefix: &[&str] = match command {
        None => &["toolkit", "gui"],
        Some("web" | "gui" | "monitor_web.py" | "app_gui.py") => &["toolkit", "gui"],
        Some("status" | "-s") => &["toolkit", "run", "status", "--"],
        Some("decrypt") => &["toolkit", "run", "decrypt", "--"],
        Some("export") => &["toolkit", "run", "export", "--"],
        Some("all") => &["toolkit", "run", "all", "--"],
        Some("export-all" | "export_all_chats.py") => &["toolkit", "run", "export-all", "--"],
        Some("decode-images") => &["toolkit", "decode-images"],
        Some("decrypt_db.py") => &["toolkit", "decrypt"],
        Some("export_messages.py") => &["toolkit", "export-messages"],
        Some("find_image_key.py") => &["toolkit", "find-image-key"],
        Some("find_image_key_monitor.py") => &["toolkit", "find-image-key-monitor"],
        Some("decrypt_sns.py") => &["toolkit", "decrypt-sns"],
        Some("export_sns.py") => &["toolkit", "export-sns"],
        Some("voice_to_mp3.py") => &["toolkit", "voice-to-mp3"],
        Some("batch_decrypt_images.py") => &["toolkit", "batch-decrypt-images"],
        Some("monitor.py") => &["toolkit", "monitor"],
        Some("latency_test.py") => &["toolkit", "latency"],
        Some("setup.py" | "setup") => &["toolkit", "setup"],
        Some("cleanup.py" | "cleanup") => &["toolkit", "cleanup"],
        _ => &[],
    };
    if !prefix.is_empty() && !args.is_empty() {
        args.remove(0);
    }
    std::iter::once(program)
        .chain(prefix.iter().map(OsString::from))
        .chain(args)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapped(args: &[&str]) -> Vec<OsString> {
        arguments(args.iter().map(OsString::from).collect())
    }

    #[test]
    fn no_arguments_launches_gui() {
        assert_eq!(
            mapped(&["wx-toolbox"]),
            mapped(&["wx-toolbox", "toolkit", "gui"])
        );
    }

    #[test]
    fn native_cli_arguments_pass_through() {
        let args = [
            "wx-toolbox",
            "history",
            "example",
            "--limit",
            "20",
            "--json",
        ];
        assert_eq!(mapped(&args), args.map(OsString::from));
    }

    #[test]
    fn legacy_alias_preserves_argument_boundaries() {
        assert_eq!(
            mapped(&[
                "wx-toolbox",
                "main.py",
                "export_messages.py",
                "--output",
                "folder with spaces"
            ]),
            [
                "wx-toolbox",
                "toolkit",
                "export-messages",
                "--output",
                "folder with spaces"
            ]
            .map(OsString::from),
        );
    }

    #[test]
    fn unknown_script_is_not_dispatched_as_python() {
        let args = ["wx-toolbox", "unknown.py", "--flag"];
        assert_eq!(mapped(&args), args.map(OsString::from));
    }

    #[test]
    fn export_alias_inserts_native_argument_separator() {
        assert_eq!(
            mapped(&["wx-toolbox", "export", "--json"]),
            ["wx-toolbox", "toolkit", "run", "export", "--", "--json"].map(OsString::from),
        );
    }
}
