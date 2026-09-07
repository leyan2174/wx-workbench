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
    /// 可选企业微信离线快照，浏览器不能更换目录
    #[arg(long)]
    pub enterprise_snapshot: Option<PathBuf>,
    /// 固定企业微信账号 Data 目录；扫描授权仍须由每个任务单独确认
    #[arg(long, conflicts_with = "enterprise_input")]
    pub enterprise_data_dir: Option<PathBuf>,
    /// 企业账号发现的固定根目录；省略时使用原生发现器默认位置
    #[arg(long)]
    pub enterprise_discovery_root: Option<PathBuf>,
    /// 可选企业微信单个离线主库，不能是 WAL
    #[arg(long, requires = "enterprise_key_file")]
    pub enterprise_input: Option<PathBuf>,
    /// 企业微信全局密钥文本文件；逐库密钥优先，不接收命令行明文密钥
    #[arg(long)]
    pub enterprise_key_file: Option<PathBuf>,
    /// 带账号绑定的企业微信逐库密钥 JSON；浏览器不能提交文件路径
    #[arg(long, conflicts_with = "enterprise_input")]
    pub enterprise_keys_file: Option<PathBuf>,
    /// 明确本人企业账号 ID，不从目录猜测
    #[arg(long)]
    pub enterprise_self_id: Option<i64>,
    /// 只允许扫描这些企业微信 PID；授权仍由任务单独提供
    #[arg(long, value_delimiter = ',')]
    pub enterprise_pid: Vec<u32>,
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
    fn enterprise_batch_accepts_global_and_database_keys_together() {
        let parsed = Invocation::try_parse_from([
            "web",
            "--enterprise-data-dir",
            "synthetic-data",
            "--enterprise-key-file",
            "global.key",
            "--enterprise-keys-file",
            "per-database.json",
        ])
        .unwrap()
        .args;
        assert_eq!(parsed.enterprise_key_file, Some("global.key".into()));
        assert_eq!(
            parsed.enterprise_keys_file,
            Some("per-database.json".into())
        );
    }

    #[test]
    fn enterprise_single_database_keeps_its_separate_key_contract() {
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
