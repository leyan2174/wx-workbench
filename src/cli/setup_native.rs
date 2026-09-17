pub use super::operation_args::setup_native::*;
// Argument adaptation and authenticated daemon operation forwarding only.
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
    let explicit = args.db_dir.is_some();
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
    if interactive && args.db_dir.is_none() {
        args.db_dir = ask("账号 db_storage 目录")?.map(Into::into);
    }
    args.interactive = false;
    if !args.check && args.apply && !args.yes && tty {
        let mut preview = args.clone();
        preview.apply = false;
        let output = crate::service::operation_client::run_capture(Operation::SetupPreview {
            args: preview.into(),
        })?;
        let mut report: serde_json::Value = serde_json::from_slice(&output)?;
        let review: crate::service::operations::SetupReview =
            serde_json::from_value(report["review"].take())?;
        if ask("确认写入请输入 APPLY")?.as_deref() != Some("APPLY") {
            return Ok(false);
        }
        args.yes = true;
        args.config_path = Some(review.config_path.clone());
        crate::service::operation_client::run(Operation::SetupApply {
            args: args.into(),
            review,
        })?;
        return Ok(true);
    }
    let applied = args.apply;
    crate::service::operation_client::run(Operation::Setup { args: args.into() })?;
    Ok(applied)
}
