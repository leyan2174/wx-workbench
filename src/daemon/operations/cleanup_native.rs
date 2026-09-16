//! 原生清理 CLI；默认配置只解析一次，固定账号后不重新发现。

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};

use crate::application::cleanup::{self, ExecuteOptions, PlanInputs};

pub use crate::service::operation_requests::cleanup_native::Args;

/// 将清理报告或拒绝原因输出为 JSON；失败或未完整完成时返回错误，由调用方处理。
pub fn cmd(args: Args) -> Result<()> {
    match outcome(args) {
        Ok((value, complete)) => {
            println!("{}", serde_json::to_string_pretty(&value)?);
            ensure!(
                complete,
                "清理状态、计划或执行未完整完成，详见 JSON 中的错误和 partial 标记"
            );
            Ok(())
        }
        Err(error) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "mode":"refused", "count":0, "bytes":0,
                    "errors":[{"error":error.to_string()}],
                    "delete_started":false
                }))?
            );
            Err(error)
        }
    }
}

fn outcome(args: Args) -> Result<(Value, bool)> {
    let config_path = match args.config {
        Some(path) => path,
        None => crate::config::find_config_file()?,
    };
    let runtime_root = args.runtime_root.unwrap_or_else(crate::config::cli_dir);
    let runtime = cleanup::selected_runtime(
        &std::path::absolute(config_path)?,
        &std::path::absolute(runtime_root)?,
    )?;
    if let Some(confirm) = args.confirm_account.as_deref() {
        ensure!(
            confirm == runtime.id,
            "确认账号标识与所选配置/runtime 根目录不符"
        );
    }
    if args.execute {
        ensure!(
            args.mode.is_none()
                && !args.dry_run
                && args.write_plan.is_none()
                && args.native_inventories.is_empty()
                && args.adoption_manifest.is_none(),
            "执行只能使用先前审阅的计划，不接受替换扫描输入"
        );
        let plan =
            cleanup::read_plan_for(&runtime, args.plan.as_ref().context("执行缺少 --plan")?)?;
        let key_authorization = args.authorize_key_removal.then_some(runtime.id.as_str());
        let report = cleanup::execute_with_key_removal_for(
            &runtime,
            &plan,
            &ExecuteOptions {
                select: args.select,
                confirm_account: args.confirm_account.context("执行缺少 --confirm-account")?,
                authorize_legacy: args.authorize_legacy,
            },
            key_authorization,
        )?;
        let complete = report.complete;
        return Ok((serde_json::to_value(report)?, complete));
    }
    ensure!(
        args.plan.is_none() && args.select.is_empty(),
        "没有 --execute 时不接受计划执行参数"
    );
    let authorization = if args.authorize_legacy {
        Some(
            args.confirm_account
                .as_deref()
                .context("接管旧文件需确认账号 ID")?,
        )
    } else {
        None
    };
    let key_authorization = if args.authorize_key_removal {
        Some(
            args.confirm_account
                .as_deref()
                .context("移除密钥需确认账号 ID")?,
        )
    } else {
        None
    };
    let plan = cleanup::plan_with_key_removal_for(
        &runtime,
        &PlanInputs {
            native_inventories: args.native_inventories,
            adoption_manifest: args.adoption_manifest,
        },
        authorization,
        key_authorization,
    )?;
    if let Some(path) = args.write_plan {
        // 删除候选必须无错误；独立空间统计是否完整仍在 JSON 中明确报告。
        ensure!(plan.errors.is_empty(), "存在错误的计划不能保存为可执行计划");
        cleanup::write_plan_for(&runtime, &plan, &path)?;
    }
    let complete = plan.errors.is_empty() && !plan.disk_usage.partial;
    Ok((serde_json::to_value(plan)?, complete))
}
