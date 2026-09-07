//! 原生清理 CLI；默认配置只解析一次，固定账号后不重新发现。

use anyhow::{ensure, Context, Result};
use clap::ValueEnum;
use serde_json::{json, Value};
use std::path::PathBuf;

use crate::toolkit::cleanup::{self, ExecuteOptions, PlanInputs};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Mode {
    Status,
    Plan,
}

#[derive(Debug, Clone, clap::Args)]
pub struct Args {
    /// 默认输出只读 JSON 状态/计划，不启动交互式删除。
    #[arg(value_enum, conflicts_with = "execute")]
    pub mode: Option<Mode>,

    /// 配置路径；省略时使用程序统一的配置查找规则，只选择一次。
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// 覆盖程序默认 runtime 根目录，通常无需指定。
    #[arg(long)]
    pub runtime_root: Option<PathBuf>,

    /// 只生成 JSON 计划；不创建 runtime 目录、锁或缓存。
    #[arg(long, conflicts_with = "execute")]
    pub dry_run: bool,

    /// 已绑定当前账号的 _directory_export.json 或 _cleanup_inventory.json。
    #[arg(long = "native-inventory", conflicts_with = "execute")]
    pub native_inventories: Vec<PathBuf>,

    /// 逐文件旧产物接管 JSON，必须同时明确授权并确认当前账号。
    #[arg(long, requires_all = ["authorize_legacy", "confirm_account"], conflicts_with = "execute")]
    pub adoption_manifest: Option<PathBuf>,

    /// 本次明确授权读取/清理接管清单中的旧文件；不能解除密钥或输入保护。
    #[arg(long, requires = "confirm_account")]
    pub authorize_legacy: bool,

    /// 单独授权当前配置的 keys_file；生成计划和执行时都必须明确提供。
    #[arg(long, requires = "confirm_account", conflicts_with_all = ["authorize_legacy", "native_inventories", "adoption_manifest"])]
    pub authorize_key_removal: bool,

    /// 逐字确认所选 runtime ID；不是微信昵称，也不接受前缀匹配。
    #[arg(long)]
    pub confirm_account: Option<String>,

    /// 将计划写入一个不存在的绝对 JSON 路径；不覆盖，也不创建父目录。
    #[arg(long, conflicts_with = "execute")]
    pub write_plan: Option<PathBuf>,

    /// 显式执行先前计划；必须同时指定 plan、select 和 confirm-account。
    #[arg(long, requires_all = ["plan", "select", "confirm_account"], conflicts_with_all = ["dry_run", "mode", "write_plan", "native_inventories", "adoption_manifest"])]
    pub execute: bool,

    /// 已审阅的 JSON 计划绝对路径；内容、来源、目录身份和文件都会重新核验。
    #[arg(long, requires = "execute")]
    pub plan: Option<PathBuf>,

    /// 精确文件 ID，可重复或逗号分隔；禁止 all、分类、序号、短 ID 和通配符。
    #[arg(long, value_delimiter = ',', requires = "execute")]
    pub select: Vec<String>,
}

/// stdout 恰好输出一份 JSON；错误仍以非零退出码交给主线处理。
pub fn cmd(args: Args) -> Result<()> {
    match outcome(args) {
        Ok((value, complete)) => {
            println!("{}", serde_json::to_string_pretty(&value)?);
            ensure!(complete, "清理状态、计划或执行未完整完成，详见 JSON 中的错误和 partial 标记");
            Ok(())
        }
        Err(error) => {
            println!("{}", serde_json::to_string_pretty(&json!({
                "mode":"refused", "count":0, "bytes":0,
                "errors":[{"error":error.to_string()}],
                "delete_started":false
            }))?);
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
        &std::path::absolute(config_path)?, &std::path::absolute(runtime_root)?,
    )?;
    if let Some(confirm) = args.confirm_account.as_deref() {
        ensure!(confirm == runtime.id, "确认账号标识与所选配置/runtime 根目录不符");
    }
    if args.execute {
        ensure!(args.mode.is_none() && !args.dry_run && args.write_plan.is_none()
            && args.native_inventories.is_empty() && args.adoption_manifest.is_none(),
            "执行只能使用先前审阅的计划，不接受替换扫描输入");
        let plan = cleanup::read_plan_for(&runtime, args.plan.as_ref().context("执行缺少 --plan")?)?;
        let key_authorization = args.authorize_key_removal.then_some(runtime.id.as_str());
        let report = cleanup::execute_with_key_removal_for(&runtime, &plan, &ExecuteOptions {
            select: args.select,
            confirm_account: args.confirm_account.context("执行缺少 --confirm-account")?,
            authorize_legacy: args.authorize_legacy,
        }, key_authorization)?;
        let complete = report.complete;
        return Ok((serde_json::to_value(report)?, complete));
    }
    ensure!(args.plan.is_none() && args.select.is_empty(), "没有 --execute 时不接受计划执行参数");
    let authorization = if args.authorize_legacy {
        Some(args.confirm_account.as_deref().context("接管旧文件需确认账号 ID")?)
    } else { None };
    let key_authorization = if args.authorize_key_removal {
        Some(args.confirm_account.as_deref().context("移除密钥需确认账号 ID")?)
    } else { None };
    let plan = cleanup::plan_with_key_removal_for(&runtime, &PlanInputs {
        native_inventories: args.native_inventories,
        adoption_manifest: args.adoption_manifest,
    }, authorization, key_authorization)?;
    if let Some(path) = args.write_plan {
        // 删除候选必须无错误；独立空间统计是否完整仍在 JSON 中明确报告。
        ensure!(plan.errors.is_empty(), "存在错误的计划不能保存为可执行计划");
        cleanup::write_plan_for(&runtime, &plan, &path)?;
    }
    let complete = plan.errors.is_empty() && !plan.disk_usage.partial;
    Ok((serde_json::to_value(plan)?, complete))
}
