mod init;
pub mod sessions;
pub mod history;
pub mod search;
pub mod contacts;
pub mod export;
pub mod watch;
pub mod daemon_cmd;
pub mod transport;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// wx — 微信本地数据 CLI
#[derive(Parser)]
#[command(name = "wx", version = "0.1.0", about = "wx — 微信本地数据 CLI")]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 初始化：检测数据目录并扫描加密密钥
    Init {
        /// 强制重新扫描（覆盖已有配置）
        #[arg(long)]
        force: bool,
    },
    /// 列出最近会话
    Sessions {
        /// 会话数量
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
        /// 输出原始 JSON
        #[arg(long)]
        json: bool,
    },
    /// 查看聊天记录
    History {
        /// 聊天对象名称（支持模糊匹配）
        chat: String,
        /// 消息数量
        #[arg(short = 'n', long, default_value = "50")]
        limit: usize,
        /// 分页偏移
        #[arg(long, default_value = "0")]
        offset: usize,
        /// 起始时间 YYYY-MM-DD
        #[arg(long)]
        since: Option<String>,
        /// 结束时间 YYYY-MM-DD
        #[arg(long)]
        until: Option<String>,
        /// 输出原始 JSON
        #[arg(long)]
        json: bool,
    },
    /// 搜索消息
    Search {
        /// 搜索关键词
        keyword: String,
        /// 限定聊天（可多次指定）
        #[arg(long = "in", value_name = "CHAT")]
        chats: Vec<String>,
        /// 结果数量
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
        /// 起始时间 YYYY-MM-DD
        #[arg(long)]
        since: Option<String>,
        /// 结束时间 YYYY-MM-DD
        #[arg(long)]
        until: Option<String>,
        /// 输出原始 JSON
        #[arg(long)]
        json: bool,
    },
    /// 查看联系人
    Contacts {
        /// 按名字过滤
        #[arg(short = 'q', long)]
        query: Option<String>,
        /// 显示数量
        #[arg(short = 'n', long, default_value = "50")]
        limit: usize,
        /// 输出原始 JSON
        #[arg(long)]
        json: bool,
    },
    /// 导出聊天记录到文件
    Export {
        /// 聊天对象名称
        chat: String,
        /// 起始时间 YYYY-MM-DD
        #[arg(long)]
        since: Option<String>,
        /// 结束时间 YYYY-MM-DD
        #[arg(long)]
        until: Option<String>,
        /// 最多导出条数
        #[arg(short = 'n', long, default_value = "500")]
        limit: usize,
        /// 输出格式 [markdown|txt|json]
        #[arg(short = 'f', long, default_value = "markdown", value_parser = ["markdown", "txt", "json"])]
        format: String,
        /// 输出文件（默认 stdout）
        #[arg(short = 'o', long)]
        output: Option<String>,
    },
    /// 实时监听新消息（Ctrl+C 退出）
    Watch {
        /// 只显示指定聊天的消息
        #[arg(long)]
        chat: Option<String>,
        /// 输出 JSON lines
        #[arg(long)]
        json: bool,
    },
    /// 管理 wx-daemon
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCommands,
    },
}

#[derive(Subcommand)]
pub enum DaemonCommands {
    /// 查看 daemon 运行状态
    Status,
    /// 停止 daemon
    Stop,
    /// 查看 daemon 日志
    Logs {
        /// 持续输出（tail -f）
        #[arg(short = 'f', long)]
        follow: bool,
        /// 显示最近 N 行
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,
    },
}

pub fn run() {
    let cli = Cli::parse();
    if let Err(e) = dispatch(cli) {
        eprintln!("错误: {}", e);
        std::process::exit(1);
    }
}

fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Init { force } => init::cmd_init(force),
        Commands::Sessions { limit, json } => sessions::cmd_sessions(limit, json),
        Commands::History { chat, limit, offset, since, until, json } => {
            history::cmd_history(chat, limit, offset, since, until, json)
        }
        Commands::Search { keyword, chats, limit, since, until, json } => {
            search::cmd_search(keyword, chats, limit, since, until, json)
        }
        Commands::Contacts { query, limit, json } => contacts::cmd_contacts(query, limit, json),
        Commands::Export { chat, since, until, limit, format, output } => {
            export::cmd_export(chat, since, until, limit, format, output)
        }
        Commands::Watch { chat, json } => watch::cmd_watch(chat, json),
        Commands::Daemon { cmd } => daemon_cmd::cmd_daemon(cmd),
    }
}
