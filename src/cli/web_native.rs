//! 原生本地 Web 入口；GUI 仅自动打开浏览器，不模拟原桌面窗口。
use anyhow::{Context, Result};
use std::path::PathBuf;

#[derive(Debug, Default, Clone, clap::Args)]
pub struct Args {
    /// 仅绑定 127.0.0.1；0 由操作系统分配空闲端口
    #[arg(long, default_value_t = 0)]
    pub port: u16,
    /// 自动在系统默认浏览器打开本地页面
    #[arg(long)]
    pub open: bool,
    /// 当前账号已解码图片缓存；预览仅只读此目录和本服务生成的图片目录
    #[arg(long)]
    pub image_cache_dir: Option<PathBuf>,
    /// 允许本次 Web 启动提交计划媒体扫描；不授予下载或进程内存读取权限
    #[arg(long)]
    pub task_allow_plan_scan: bool,
    /// 仅允许本次 Web 启动提交 export_voices 并写入固定任务目录；不授予扫描或下载权限
    #[arg(long)]
    pub task_allow_media_write: bool,
}

impl From<Args> for crate::service::web::HostSettings {
    fn from(args: Args) -> Self {
        Self {
            port: args.port,
            open: args.open,
            image_cache_dir: args.image_cache_dir,
            allow_plan_scan: args.task_allow_plan_scan,
            allow_media_write: args.task_allow_media_write,
        }
    }
}

pub fn cmd_web(args: Args) -> Result<()> {
    let runtime = crate::runtime::RuntimeContext::load()
        .map_err(|_| anyhow::anyhow!("无法加载选中账号配置；首次使用请先查看 wx setup --help，或运行 wx init 完成账号初始化，然后重新启动 Web"))?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("无法创建 Web 运行时")?
        .block_on(crate::web::serve(runtime, args.into()))
}

pub fn cmd_gui(mut args: Args) -> Result<()> {
    args.open = true;
    cmd_web(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Invocation {
        #[command(flatten)]
        args: Args,
    }

    #[test]
    fn host_settings_preserve_defaults_and_explicit_values() {
        let default = Invocation::try_parse_from(["web"]).unwrap().args;
        assert_eq!(
            crate::service::web::HostSettings::from(default),
            Default::default()
        );
        let parsed = Invocation::try_parse_from([
            "web",
            "--port",
            "12345",
            "--open",
            "--image-cache-dir",
            "synthetic-images",
        ])
        .unwrap()
        .args;
        assert_eq!(
            crate::service::web::HostSettings::from(parsed),
            crate::service::web::HostSettings {
                port: 12345,
                open: true,
                image_cache_dir: Some("synthetic-images".into()),
                allow_plan_scan: false,
                allow_media_write: false,
            }
        );
    }

    #[test]
    fn accepts_personal_image_cache() {
        let parsed = Invocation::try_parse_from(["web", "--image-cache-dir", "synthetic-images"])
            .unwrap()
            .args;
        assert_eq!(parsed.image_cache_dir, Some("synthetic-images".into()));
    }

    #[test]
    fn plan_scan_is_opt_in_for_this_web_startup() {
        let disabled = Invocation::try_parse_from(["web"]).unwrap().args;
        assert!(!crate::service::web::HostSettings::from(disabled).allow_plan_scan);
        let enabled = Invocation::try_parse_from(["web", "--task-allow-plan-scan"])
            .unwrap()
            .args;
        assert!(crate::service::web::HostSettings::from(enabled).allow_plan_scan);
    }

    #[test]
    fn voice_media_write_is_host_opt_in_and_independent_of_plan_scan() {
        assert!(!Args::default().task_allow_media_write);
        for (flags, media_write, plan_scan) in [
            (vec![], false, false),
            (vec!["--task-allow-media-write"], true, false),
            (vec!["--task-allow-plan-scan"], false, true),
            (
                vec!["--task-allow-media-write", "--task-allow-plan-scan"],
                true,
                true,
            ),
        ] {
            let mut argv = vec!["web"];
            argv.extend(flags);
            let args = Invocation::try_parse_from(&argv).unwrap().args;
            assert_eq!(args.task_allow_media_write, media_write);
            let settings = crate::service::web::HostSettings::from(args);
            assert_eq!(settings.allow_media_write, media_write);
            assert_eq!(settings.allow_plan_scan, plan_scan);
        }
        assert!(Invocation::try_parse_from(["web", "--allow-media-write"]).is_err());
        assert!(Invocation::try_parse_from(["web", "--task-allow-media-write=false"]).is_err());
    }

    #[test]
    fn removed_enterprise_flags_are_rejected() {
        assert!(
            Invocation::try_parse_from(["web", "--enterprise-input", "synthetic.db",]).is_err()
        );
        assert!(Invocation::try_parse_from([
            "web",
            "--enterprise-input",
            "synthetic.db",
            "--enterprise-key-file",
            "global.key",
            "--enterprise-keys-file",
            "per-database.json",
        ])
        .is_err());
        assert!(Invocation::try_parse_from([
            "web",
            "--enterprise-input",
            "synthetic.db",
            "--enterprise-key-file",
            "global.key",
            "--enterprise-data-dir",
            "synthetic-data",
        ])
        .is_err());
    }
}
