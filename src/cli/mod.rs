pub(crate) mod asr;
mod asr_batch;
mod asr_database;
pub mod attachments;
pub mod biz_articles;
mod chat_plan;
mod cleanup_native;
pub mod contacts;
pub mod daemon_cmd;
mod database_keys;
mod decode;
pub mod export;
mod export_all;
mod export_chat;
mod export_chats;
mod export_delta;
mod export_emoticons;
mod export_messages;
mod export_sns;
pub mod extract;
pub mod favorites;
pub mod history;
mod image_keys;
mod init;
mod key_provider;
mod mcp;
mod mcp_tasks;
pub mod members;
mod monitor_native;
pub mod new_messages;
pub(crate) mod operation_args;
pub mod output;
pub mod search;
pub mod sessions;
mod setup_native;
pub mod sns_album;
mod sns_archive;
pub mod sns_feed;
pub mod sns_notifications;
pub mod sns_search;
mod sns_timeline;
pub mod stats;
mod tasks;
pub mod toolkit;
pub mod unread;
pub mod voices;
pub(crate) mod web_native;

use self::output::OutputOpts;
use anyhow::Result;
use clap::{Parser, Subcommand};

#[cfg(test)]
mod contract_tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn grouped_arguments_preserve_cli_defaults_and_flags() {
        let Commands::History(history) = Cli::try_parse_from(["wx", "history", "peer"])
            .unwrap()
            .command
        else {
            panic!("expected history");
        };
        assert_eq!((history.limit, history.offset), (50, 0));
        assert!(!history.json && !history.oldest_first);

        let Commands::Voices(voices) = Cli::try_parse_from([
            "wx",
            "voices",
            "peer",
            "-o",
            "voices",
            "-n",
            "2",
            "--offset",
            "3",
            "--overwrite",
            "--json",
        ])
        .unwrap()
        .command
        else {
            panic!("expected voices");
        };
        assert_eq!(voices.chat.as_deref(), Some("peer"));
        assert_eq!(voices.output, "voices");
        assert_eq!((voices.limit, voices.offset), (Some(2), 3));
        assert!(voices.overwrite && voices.json);

        let Commands::Toolkit {
            cmd: toolkit::ToolkitCommands::ExportSnsNative(sns),
        } = Cli::try_parse_from(["wx", "toolkit", "export-sns-native", "sns.db", "preview"])
            .unwrap()
            .command
        else {
            panic!("expected native SNS export");
        };
        assert_eq!(sns.sns_db, std::path::Path::new("sns.db"));
        assert_eq!(sns.output_dir, std::path::Path::new("preview"));
        assert!(!sns.download_media && !sns.update && !sns.adopt_existing);
        assert!(Cli::try_parse_from([
            "wx",
            "toolkit",
            "export-sns-native",
            "sns.db",
            "preview",
            "--adopt-existing",
        ])
        .is_err());
    }

    #[test]
    fn command_tree_and_passthrough_contracts_are_valid() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                Cli::command().debug_assert();
                assert!(Cli::try_parse_from(["wx", "toolkit", "run", "decrypt"]).is_err());
                assert!(Cli::try_parse_from(["wx", "toolkit", "export-chats"]).is_err());
                assert!(Cli::try_parse_from(["wx", "toolkit", "export-all", "--dry-run"]).is_ok());
                let cli = Cli::try_parse_from([
                    "wx",
                    "history",
                    "peer",
                    "--types",
                    "image,text",
                    "--types",
                    "voice",
                    "--oldest-first",
                ])
                .unwrap();
                let Commands::History(args) = cli.command else {
                    panic!("wrong history command")
                };
                assert_eq!(args.msg_type, None);
                assert_eq!(args.msg_types, ["image", "text", "voice"]);
                assert!(args.oldest_first);
                assert!(Cli::try_parse_from([
                    "wx", "history", "peer", "--type", "text", "--types", "image"
                ])
                .is_err());
                assert!(
                    Cli::try_parse_from(["wx", "history", "peer", "--types", "invalid"]).is_err()
                );
                let plan = [
                    "wx",
                    "toolkit",
                    "chat-plan-native",
                    "--decrypted-dir",
                    "plain",
                    "--user",
                    "alice",
                    "--output",
                    "plan.csv",
                ];
                assert!(Cli::try_parse_from(plan).is_ok());
                let mut old_users = plan;
                old_users[5] = "--users";
                assert!(Cli::try_parse_from(old_users).is_err());
                let mut old_output = plan;
                old_output[7] = "--write-plan-csv";
                assert!(Cli::try_parse_from(old_output).is_err());
                assert!(
                    Cli::try_parse_from(["wx", "sns-album", "alice", "--output", "album"]).is_ok()
                );
                assert!(Cli::try_parse_from([
                    "wx",
                    "sns-album",
                    "alice",
                    "--output-root",
                    "album"
                ])
                .is_err());
                assert!(
                    Cli::try_parse_from(["wx", "attachments", "alice", "--kind", "image"]).is_ok()
                );
                assert!(
                    Cli::try_parse_from(["wx", "attachments", "alice", "--kind", "img"]).is_err()
                );
                let Commands::Init { key_provider, .. } =
                    Cli::try_parse_from(["wx", "init"]).unwrap().command
                else {
                    panic!("wrong init command")
                };
                assert_eq!(key_provider, key_provider::KeyProvider::Saved);
                assert!(Cli::try_parse_from(["wx", "init", "--key-provider", "auto"]).is_err());
            })
            .unwrap()
            .join()
            .unwrap();
    }
}

/// 微信本地数据命令行工具
#[derive(Parser)]
#[command(
    name = "wx",
    version = env!("CARGO_PKG_VERSION"),
    about = "wx-workbench · 微信本地数据工作台（CLI / MCP / Web）"
)]
pub struct Cli {
    /// 返回更重的 freshness/source 元数据（如 per-shard latest、cache modes）
    #[arg(long, global = true)]
    with_meta: bool,
    /// 在 meta 里暴露真实 shard 路径（调试用）
    #[arg(long, global = true, hide = true)]
    debug_source: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 原生 MCP stdio 入口：只读查询及受控图片导出，调用需显式 WX_CLI_CONFIG
    Mcp(mcp::McpArgs),
    /// 初始化：检测数据目录并扫描加密密钥
    Init {
        /// 强制重新扫描（覆盖已有配置）
        #[arg(long)]
        force: bool,
        /// 显式指定微信账号的 db_storage 目录，避免多账号时自动选错
        #[arg(long)]
        db_dir: Option<String>,
        /// 密钥来源：已保存账号密钥、显式只读扫描、或账号级捕获
        #[arg(long, value_enum, default_value = "saved")]
        key_provider: key_provider::KeyProvider,
        /// 允许账号级捕获关闭并重新启动微信，需要再次登录
        #[arg(long, requires = "force")]
        restart_wechat: bool,
        /// 账号级捕获使用的 Weixin.exe 路径
        #[arg(long)]
        wechat_exe: Option<std::path::PathBuf>,
        /// 等待微信登录和捕获的秒数
        #[arg(long, default_value = "300", value_parser = clap::value_parser!(u64).range(10..=1800))]
        capture_timeout: u64,
    },
    /// 列出最近会话
    Sessions {
        /// 会话数量
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
        /// 输出 JSON（默认 YAML）
        #[arg(long)]
        json: bool,
    },
    /// 查看聊天记录
    History(history::Args),
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
        /// 消息类型过滤 [text|image|voice|video|sticker|location|link|file|call|system]
        #[arg(long = "type", value_name = "TYPE",
              value_parser = ["text","image","voice","video","sticker","location","link","file","call","system"])]
        msg_type: Option<String>,
        /// 输出 JSON（默认 YAML）
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
        /// 输出 JSON（默认 YAML）
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
        /// 输出格式 [markdown|txt|json|yaml]
        #[arg(short = 'f', long, default_value = "markdown", value_parser = ["markdown", "txt", "json", "yaml"])]
        format: String,
        /// 输出文件（默认 stdout）
        #[arg(short = 'o', long)]
        output: Option<String>,
    },
    /// 显示有未读消息的会话
    Unread {
        /// 显示数量
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
        /// 按会话类型过滤，逗号分隔。示例：--filter private,group 只看真人的未读
        #[arg(long, value_name = "TYPES", value_delimiter = ',',
              value_parser = ["all", "private", "group", "official", "folded"])]
        filter: Vec<String>,
        /// 输出 JSON（默认 YAML）
        #[arg(long)]
        json: bool,
    },
    /// 查看群成员
    Members {
        /// 群聊名称（支持模糊匹配）
        chat: String,
        /// 输出 JSON（默认 YAML）
        #[arg(long)]
        json: bool,
    },
    /// 获取自上次检查以来的新消息
    NewMessages {
        /// 显示数量上限
        #[arg(short = 'n', long, default_value = "200")]
        limit: usize,
        /// 输出 JSON（默认 YAML）
        #[arg(long)]
        json: bool,
    },
    /// 聊天统计分析
    Stats {
        /// 聊天对象名称（支持模糊匹配）
        chat: String,
        /// 起始时间 YYYY-MM-DD
        #[arg(long)]
        since: Option<String>,
        /// 结束时间 YYYY-MM-DD
        #[arg(long)]
        until: Option<String>,
        /// 输出 JSON（默认 YAML）
        #[arg(long)]
        json: bool,
    },
    /// 查看微信收藏内容
    Favorites {
        /// 显示数量
        #[arg(short = 'n', long, default_value = "50")]
        limit: usize,
        /// 类型过滤 [text|image|article|card|video]
        #[arg(long = "type", value_name = "TYPE",
              value_parser = ["text","image","article","card","video"])]
        fav_type: Option<String>,
        /// 内容关键词搜索
        #[arg(short = 'q', long)]
        query: Option<String>,
        /// 输出 JSON（默认 YAML）
        #[arg(long)]
        json: bool,
    },
    /// 朋友圈互动通知：别人对我的朋友圈点赞/评论 + 我评过的帖子下的跟帖
    SnsNotifications {
        /// 显示数量
        #[arg(short = 'n', long, default_value = "50")]
        limit: usize,
        /// 起始时间 YYYY-MM-DD
        #[arg(long)]
        since: Option<String>,
        /// 结束时间 YYYY-MM-DD
        #[arg(long)]
        until: Option<String>,
        /// 包含已读通知（默认仅未读）
        #[arg(long)]
        include_read: bool,
        /// 输出 JSON（默认 YAML）
        #[arg(long)]
        json: bool,
    },
    /// 朋友圈时间线：按时间/作者筛选本地缓存的朋友圈
    SnsFeed {
        /// 显示数量
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
        /// 起始时间 YYYY-MM-DD
        #[arg(long)]
        since: Option<String>,
        /// 结束时间 YYYY-MM-DD
        #[arg(long)]
        until: Option<String>,
        /// 只看指定作者（昵称 / 备注名 / 微信 ID，模糊匹配）
        #[arg(long)]
        user: Option<String>,
        /// 输出 JSON（默认 YAML）
        #[arg(long)]
        json: bool,
    },
    /// 导出指定联系人的朋友圈文字、图片、视频和相册 HTML
    SnsAlbum {
        #[command(flatten)]
        args: sns_album::Args,
    },
    /// 查询公众号文章推送（本地缓存）
    BizArticles {
        /// 显示数量
        #[arg(short = 'n', long, default_value = "50")]
        limit: usize,
        /// 限定公众号（名称模糊匹配）
        #[arg(long)]
        account: Option<String>,
        /// 起始时间 YYYY-MM-DD
        #[arg(long)]
        since: Option<String>,
        /// 结束时间 YYYY-MM-DD
        #[arg(long)]
        until: Option<String>,
        /// 只看有未读的公众号，每个公众号取最新 1 篇
        #[arg(long)]
        unread: bool,
        /// 输出 JSON（默认 YAML）
        #[arg(long)]
        json: bool,
    },
    /// 朋友圈全文搜索：匹配正文关键词
    SnsSearch {
        /// 关键词
        keyword: String,
        /// 结果数量
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
        /// 起始时间 YYYY-MM-DD
        #[arg(long)]
        since: Option<String>,
        /// 结束时间 YYYY-MM-DD
        #[arg(long)]
        until: Option<String>,
        /// 限定作者（昵称 / 备注名 / 微信 ID）
        #[arg(long)]
        user: Option<String>,
        /// 输出 JSON（默认 YAML）
        #[arg(long)]
        json: bool,
    },
    /// 列出某会话的图片附件，返回不透明 attachment_id
    Attachments {
        /// 会话名称（联系人显示名 / wxid / @chatroom username 都可以）
        chat: String,
        /// 类型（当前仅支持 image）
        #[arg(long = "kind", value_name = "KIND", value_parser = ["image"])]
        kinds: Vec<String>,
        /// 显示数量
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
        /// 输出 JSON（默认 YAML）
        #[arg(long)]
        json: bool,
    },
    /// 把单个 attachment_id 对应的资源解密写到指定文件路径
    Extract {
        /// 由 `wx attachments` 输出的不透明 ID（base64url 字符串）
        attachment_id: String,
        /// 输出文件路径（绝对或相对当前工作目录均可；扩展名建议保留为 .jpg 等）
        #[arg(short = 'o', long)]
        output: String,
        /// 目标已存在时覆盖
        #[arg(long)]
        overwrite: bool,
        /// 输出 JSON（默认 YAML）
        #[arg(long)]
        json: bool,
    },
    /// 导出微信语音消息为 .silk，并生成 .voice.json 证据文件
    Voices(voices::Args),
    /// 调用本机原生工具箱能力（解密、图片、朋友圈、Web UI 等）
    Toolkit {
        #[command(subcommand)]
        cmd: toolkit::ToolkitCommands,
    },
    /// Manage persistent account-bound daemon tasks
    Tasks {
        #[command(subcommand)]
        cmd: tasks::Command,
    },
    /// 将指定聊天导出到输出路径
    ExportChat {
        chat: String,
        output: std::path::PathBuf,
    },
    /// 解码位置消息；跨分片 ID 冲突时需提供时间戳
    DecodeLocation {
        chat: String,
        local_id: i64,
        #[arg(default_value_t = 0)]
        create_time: i64,
        #[arg(long)]
        json: bool,
    },
    /// 解码转账消息；跨分片 ID 冲突时需提供时间戳
    DecodeTransfer {
        chat: String,
        local_id: i64,
        #[arg(default_value_t = 0)]
        create_time: i64,
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
    /// 重新加载联系人缓存
    Reload,
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
    finish_dispatch(cli);
}

fn finish_dispatch(cli: Cli) {
    let json = match &cli.command {
        Commands::History(args) => args.json,
        Commands::Voices(args) => args.json,
        Commands::Sessions { json, .. }
        | Commands::Search { json, .. }
        | Commands::Contacts { json, .. }
        | Commands::Unread { json, .. }
        | Commands::Members { json, .. }
        | Commands::NewMessages { json, .. }
        | Commands::Stats { json, .. }
        | Commands::Favorites { json, .. }
        | Commands::SnsNotifications { json, .. }
        | Commands::SnsFeed { json, .. }
        | Commands::BizArticles { json, .. }
        | Commands::SnsSearch { json, .. }
        | Commands::Attachments { json, .. }
        | Commands::Extract { json, .. }
        | Commands::DecodeLocation { json, .. }
        | Commands::DecodeTransfer { json, .. } => *json,
        _ => false,
    };
    if let Err(e) = dispatch(cli) {
        if json {
            if let Some(limit) = e.downcast_ref::<crate::ipc::outcome::QueryLimitExceeded>() {
                eprintln!(
                    "{}",
                    serde_json::to_string(limit).expect("query diagnostic JSON")
                );
                std::process::exit(1);
            }
        }
        exit_dispatch_error(e);
    }
}

fn exit_dispatch_error(error: anyhow::Error) -> ! {
    if let Some(exit) = error.downcast_ref::<crate::service::operation_client::OperationExit>() {
        std::process::exit(exit.0);
    }
    if let Some(failure) = error.downcast_ref::<crate::ipc::outcome::BusinessFailure>() {
        eprintln!("{}", failure);
        // Preserve the public query ambiguity code; worker refusal uses reserved 21.
        let code = if failure.worker_exit_code() == Some(2) {
            2
        } else {
            failure.0.worker_exit_code()
        };
        std::process::exit(code);
    }
    eprintln!("错误: {error}");
    std::process::exit(1);
}

fn dispatch(cli: Cli) -> Result<()> {
    let base_with_meta = cli.with_meta;
    let base_debug_source = cli.debug_source;
    match cli.command {
        Commands::Mcp(args) => mcp::cmd(args),
        Commands::Init {
            force,
            db_dir,
            key_provider,
            restart_wechat,
            wechat_exe,
            capture_timeout,
        } => init::cmd_init(
            force,
            db_dir,
            key_provider.into(),
            restart_wechat,
            wechat_exe,
            capture_timeout,
        ),
        Commands::Sessions { limit, json } => sessions::cmd_sessions(
            limit,
            OutputOpts {
                json,
                with_meta: base_with_meta,
                debug_source: base_debug_source,
            },
        ),
        Commands::History(args) => {
            let opts = OutputOpts {
                json: args.json,
                with_meta: base_with_meta,
                debug_source: base_debug_source,
            };
            history::cmd_history(args, opts)
        }
        Commands::Search {
            keyword,
            chats,
            limit,
            since,
            until,
            msg_type,
            json,
        } => search::cmd_search(
            keyword,
            chats,
            limit,
            since,
            until,
            msg_type,
            OutputOpts {
                json,
                with_meta: base_with_meta,
                debug_source: base_debug_source,
            },
        ),
        Commands::Contacts { query, limit, json } => contacts::cmd_contacts(query, limit, json),
        Commands::Export {
            chat,
            since,
            until,
            limit,
            format,
            output,
        } => {
            let export_json = format == "json";
            export::cmd_export(
                chat,
                since,
                until,
                limit,
                format,
                output,
                OutputOpts {
                    json: export_json,
                    with_meta: base_with_meta,
                    debug_source: base_debug_source,
                },
            )
        }
        Commands::Unread {
            limit,
            filter,
            json,
        } => unread::cmd_unread(
            limit,
            filter,
            OutputOpts {
                json,
                with_meta: base_with_meta,
                debug_source: base_debug_source,
            },
        ),
        Commands::Members { chat, json } => members::cmd_members(chat, json),
        Commands::NewMessages { limit, json } => new_messages::cmd_new_messages(
            limit,
            OutputOpts {
                json,
                with_meta: base_with_meta,
                debug_source: base_debug_source,
            },
        ),
        Commands::Stats {
            chat,
            since,
            until,
            json,
        } => stats::cmd_stats(
            chat,
            since,
            until,
            OutputOpts {
                json,
                with_meta: base_with_meta,
                debug_source: base_debug_source,
            },
        ),
        Commands::Favorites {
            limit,
            fav_type,
            query,
            json,
        } => favorites::cmd_favorites(limit, fav_type, query, json),
        Commands::SnsNotifications {
            limit,
            since,
            until,
            include_read,
            json,
        } => sns_notifications::cmd_sns_notifications(limit, since, until, include_read, json),
        Commands::SnsFeed {
            limit,
            since,
            until,
            user,
            json,
        } => sns_feed::cmd_sns_feed(limit, since, until, user, json),
        Commands::SnsAlbum { args } => sns_album::cmd_sns_album(args),
        Commands::SnsSearch {
            keyword,
            limit,
            since,
            until,
            user,
            json,
        } => sns_search::cmd_sns_search(keyword, limit, since, until, user, json),
        Commands::BizArticles {
            limit,
            account,
            since,
            until,
            unread,
            json,
        } => biz_articles::cmd_biz_articles(limit, account, since, until, unread, json),
        Commands::Attachments {
            chat,
            kinds,
            limit,
            offset,
            since,
            until,
            json,
        } => attachments::cmd_attachments(
            chat,
            kinds,
            limit,
            offset,
            since,
            until,
            OutputOpts {
                json,
                with_meta: base_with_meta,
                debug_source: base_debug_source,
            },
        ),
        Commands::Extract {
            attachment_id,
            output,
            overwrite,
            json,
        } => extract::cmd_extract(attachment_id, output, overwrite, json),
        Commands::Voices(args) => voices::cmd_voices(args),
        Commands::Toolkit { cmd } => toolkit::cmd_toolkit(cmd),
        Commands::Tasks { cmd } => tasks::cmd(cmd),
        Commands::ExportChat { chat, output } => export_chat::cmd_export(chat, output),
        Commands::DecodeTransfer {
            chat,
            local_id,
            create_time,
            json,
        } => decode::cmd_decode(
            crate::ipc::Request::DecodeTransfer {
                chat,
                local_id,
                create_time,
            },
            json,
        ),
        Commands::DecodeLocation {
            chat,
            local_id,
            create_time,
            json,
        } => decode::cmd_decode(
            crate::ipc::Request::DecodeLocation {
                chat,
                local_id,
                create_time,
            },
            json,
        ),
        Commands::Daemon { cmd } => daemon_cmd::cmd_daemon(cmd),
    }
}

#[cfg(test)]
mod request_conversion_tests;
