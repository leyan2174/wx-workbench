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
}

pub fn cmd_web(args: Args) -> Result<()> {
    let runtime = crate::runtime::RuntimeContext::load()
        .map_err(|_| anyhow::anyhow!("无法加载选中账号配置；首次使用请先查看 wx toolkit setup --help，或运行 wx init 完成账号初始化，然后重新启动 Web"))?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("无法创建 Web 运行时")?
        .block_on(crate::toolkit::web::serve(runtime, args))
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
    fn accepts_personal_image_cache() {
        let parsed = Invocation::try_parse_from(["web", "--image-cache-dir", "synthetic-images"])
            .unwrap()
            .args;
        assert_eq!(parsed.image_cache_dir, Some("synthetic-images".into()));
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
