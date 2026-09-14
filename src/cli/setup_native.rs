//! Argument adaptation and authenticated daemon operation forwarding only.
pub use crate::daemon::operations::setup_native::{Args, Backend};
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd(args: Args) -> Result<()> {
    submit(args).map(|_| ())
}

pub(super) fn submit(mut args: Args) -> Result<bool> {
    use std::io::{self, IsTerminal, Write};
    let tty = io::stdin().is_terminal() && io::stderr().is_terminal();
    anyhow::ensure!(
        args.check || !args.interactive || tty,
        "非 TTY 环境不能等待向导输入"
    );
    let explicit = args.db_dir.is_some()
        || args.backend.is_some()
        || args.whisper_binary.is_some()
        || args.whisper_model.is_some()
        || args.local_model.is_some()
        || args.openai_key_env.is_some();
    let interactive = !args.check && (args.interactive || (tty && !explicit && !args.yes));
    fn ask(label: &str) -> Result<Option<String>> {
        eprint!("{label}（留空保留现有设置）: ");
        io::stderr().flush()?;
        let mut text = String::new();
        if io::stdin().read_line(&mut text)? == 0 {
            anyhow::bail!("配置向导已取消");
        }
        let text = text.trim().to_owned();
        Ok((!text.is_empty()).then_some(text))
    }
    if interactive {
        if args.db_dir.is_none() {
            args.db_dir = ask("账号 db_storage 目录")?.map(Into::into);
        }
        if args.backend.is_none() {
            args.backend = match ask("转写后端 local / whisper_cpp / openai")?.as_deref() {
                None => None,
                Some("local") => Some(Backend::Local),
                Some("whisper_cpp") => Some(Backend::WhisperCpp),
                Some("openai") => Some(Backend::Openai),
                Some(_) => anyhow::bail!("转写后端必须为 local、whisper_cpp 或 openai"),
            };
        }
        match args.backend {
            Some(Backend::WhisperCpp) => {
                if args.whisper_binary.is_none() {
                    args.whisper_binary = ask("whisper.cpp 可执行文件路径")?.map(Into::into);
                }
                if args.whisper_model.is_none() {
                    args.whisper_model = ask("本地 ggml 模型文件路径")?.map(Into::into);
                }
            }
            Some(Backend::Local) if args.local_model.is_none() => {
                args.local_model = ask("本地 Whisper 模型名称或路径")?
            }
            Some(Backend::Openai) if args.openai_key_env.is_none() => {
                args.openai_key_env = ask("OpenAI 凭据环境变量名（不要输入 key）")?
            }
            _ => {}
        }
    }
    args.interactive = false;
    if !args.check && args.apply && !args.yes && tty {
        let mut preview = args.clone();
        preview.apply = false;
        let output = crate::service::operation_client::run_capture(Operation::SetupPreview {
            args: preview,
        })?;
        let mut report: serde_json::Value = serde_json::from_slice(&output)?;
        let review: crate::service::operations::SetupReview =
            serde_json::from_value(report["review"].take())?;
        if ask("确认写入请输入 APPLY")?.as_deref() != Some("APPLY") {
            return Ok(false);
        }
        args.yes = true;
        args.config_path = Some(review.config_path.clone());
        crate::service::operation_client::run(Operation::SetupApply { args, review })?;
        return Ok(true);
    }
    let applied = args.apply;
    crate::service::operation_client::run(Operation::Setup { args })?;
    Ok(applied)
}
