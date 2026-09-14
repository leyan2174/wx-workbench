use crate::service::operations::{Operation, ToolkitOperation};
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[derive(Subcommand)]
pub enum ToolkitCommands {
    /// 原生配置向导与只读环境检查；默认预览，不扫描进程或下载模型
    Setup(super::setup_native::Args),
    /// 只读统计及清理计划；执行要求逐文件选择与账号确认
    Cleanup(super::cleanup_native::Args),
    /// 显式静态快照的单条语音转录；按完整消息分片及服务端 ID 关联媒体
    TranscribeDatabaseNative(super::asr_database::TranscribeDatabaseNativeArgs),
    /// 原生增量文件及 manifest；可显式追加全新批次，不改已有完整导出
    ExportDeltaNative(super::export_delta::Args),
    /// 显式离线数据库与媒体目录的完整计划 CSV
    ChatPlanNative(super::chat_plan::Args),
    /// 原生 SILK/WAV 转录；本地模型或显式授权的云端后端
    TranscribeAudioNative(super::asr::TranscribeAudioNativeArgs),
    /// 按显式媒体清单转录聊天语音并原子回写，不自动关联数据库
    TranscribeChatNative(super::asr::TranscribeChatNativeArgs),
    /// 离线解码朋友圈视频（Rust WASM）；只验证 MP4 文件头，不校验可播放性
    DecodeSnsVideo {
        input: PathBuf,
        /// 新输出文件；拒绝覆盖已有文件
        output: PathBuf,
        /// UTF-8 文本密钥文件；明文 MP4 无需密钥
        #[arg(long)]
        key_file: Option<PathBuf>,
        /// 可选模块路径；必须与内嵌模块的已审计哈希一致
        #[arg(long)]
        wasm: Option<PathBuf>,
    },
    /// 原生朋友圈预览：JSON / HTML，默认离线，可显式恢复缓存或下载媒体
    ExportSnsNative(super::export_sns::Args),
    /// 原生批量导出：日期、增量 JSON 与计划 CSV 选择；不串联语音转录
    ExportChatsNative(super::export_chats::Args),
    /// 使用已保存的账号密钥导出表情（Rust；不要求微信运行）
    ExportEmoticons(super::export_emoticons::Args),
    /// 显示本机 wechat-decrypt 源码、Python 环境和可用能力
    Status {
        /// 输出 JSON（默认 YAML）
        #[arg(long)]
        json: bool,
    },
    /// 运行 wechat-decrypt 一键入口：status / decrypt / decode-images / export / all / emoticons / web
    Run {
        /// 原生工作流命令，省略时启动 Web UI
        #[arg(default_value = "web", allow_hyphen_values = true)]
        command: String,
        /// 工作流参数，放在 -- 后；未知命令直接报错，不执行脚本
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// 使用当前账号已保存的密钥解密数据库主文件（Rust；不合并 WAL）
    Decrypt {
        /// 增量模式
        #[arg(short = 'i', long)]
        incremental: bool,
        /// 只预览，不写出
        #[arg(long)]
        dry_run: bool,
        /// 保留兼容参数位置；不接受额外参数
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// 批量导出全部聊天记录
    ExportChats {
        /// 输出目录；默认使用选中配置旁的 exported_chats
        output_dir: Option<String>,
        /// 附带语音转录
        #[arg(short = 't', long)]
        with_transcriptions: bool,
        /// 额外透传参数
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// 原生导出选中配置的朋友圈；同来源更新，旧目录须显式认领
    ExportSns(super::sns_timeline::Args),
    /// 个人聊天 CSV/HTML/JSON 与媒体目录导出
    ExportMessages(super::export_messages::Args),
    /// 归档选中账号的全部朋友圈缓存图片，不要求存在对应帖子
    #[command(name = "decrypt-sns")]
    SnsArchive(super::sns_archive::Args),
    /// 提取并保存选中账号的图片密钥，需要显式内存扫描授权
    FindImageKey(super::image_keys::Args),
    /// 明确授权后提取固定账号的数据库密钥，不改变配置或重启微信
    FindDatabaseKeys(super::database_keys::Args),
    /// 持续捕获并验证当前账号的图片密钥，找到后退出
    FindImageKeyMonitor(super::image_keys::MonitorArgs),
    /// 持续读取新消息，保留账号绑定游标与完整性提示
    Monitor(super::monitor_native::Args),
    /// 观测数据库/WAL 变化与 IPC 查询延迟
    Latency(super::monitor_native::LatencyArgs),
    /// 批量解密微信 .dat 图片（Rust；输出须在源目录外，不允许 .. 路径）
    DecodeImages {
        /// 微信 attach 根目录
        #[arg(long)]
        attach_dir: Option<String>,
        /// 明文图片输出目录
        #[arg(long)]
        decoded_dir: Option<String>,
        /// V2 AES key
        #[arg(long)]
        aes_key: Option<String>,
        /// V2 XOR key，例如 0x88
        #[arg(long)]
        xor_key: Option<String>,
        /// 忽略已存在目标，重新解密
        #[arg(long)]
        force: bool,
    },
    /// 解密单个 .dat 图片
    DecodeImage {
        /// 输入 .dat 文件
        dat_file: String,
        /// 输出图片文件
        output_file: Option<String>,
    },
    /// 批量解密目录中的 .dat 图片（Rust；输出须在源目录外，跳过已有结果）
    BatchDecryptImages {
        /// 输入目录
        input_dir: String,
        /// 输出目录
        output_dir: Option<String>,
    },
    /// 从已解密 media_0.db 批量导出 MP3（Rust，需要 ffmpeg）
    VoiceBatch {
        /// 显式配置文件，读取 decrypted_dir 和 output_base_dir
        #[arg(long)]
        config: PathBuf,
        /// 覆盖输出目录
        #[arg(long)]
        output_dir: Option<PathBuf>,
        /// username 列表，逗号分隔；默认 WECHAT_EXPORT_CONTACTS
        #[arg(long)]
        contacts: Option<String>,
    },
    /// 把 SILK 语音转换成 MP3（原生解码，需要 ffmpeg）
    VoiceToMp3 {
        /// 输入 SILK 文件
        input: String,
        /// 输出 MP3 文件
        output: Option<String>,
    },
    /// 对导出的聊天 JSON 做语音转录
    TranscribeChat(super::asr_batch::Args),
    /// 启动固定账号的本地原生 Web 服务
    Web(super::web_native::Args),
    /// 启动本地工作台并在浏览器中打开
    Gui(super::web_native::Args),
}

pub fn cmd_toolkit(cmd: ToolkitCommands) -> Result<()> {
    let operation = match cmd {
        ToolkitCommands::Setup(args) => return super::setup_native::cmd(args),
        ToolkitCommands::Cleanup(args) => return super::cleanup_native::cmd(args),
        ToolkitCommands::TranscribeDatabaseNative(args) => {
            return super::asr_database::cmd_transcribe_database_native(args)
        }
        ToolkitCommands::ExportDeltaNative(args) => return super::export_delta::cmd(args),
        ToolkitCommands::ChatPlanNative(args) => return super::chat_plan::cmd(args),
        ToolkitCommands::TranscribeAudioNative(args) => {
            return super::asr::cmd_transcribe_audio_native(args)
        }
        ToolkitCommands::TranscribeChatNative(args) => {
            return super::asr::cmd_transcribe_chat_native(args)
        }
        ToolkitCommands::DecodeSnsVideo {
            input,
            output,
            key_file,
            wasm,
        } => ToolkitOperation::DecodeSnsVideo {
            input,
            output,
            key_file,
            wasm,
        },
        ToolkitCommands::ExportSnsNative(super::export_sns::Args {
            sns_db,
            output_dir,
            contact_db,
            contacts,
            utc_offset,
            download_media,
            update,
            adopt_existing,
            local_cache,
        }) => ToolkitOperation::ExportSnsNative {
            sns_db,
            output_dir,
            contact_db,
            contacts,
            utc_offset,
            download_media,
            update,
            adopt_existing,
            local_cache,
        },
        ToolkitCommands::ExportChatsNative(args) => return super::export_chats::cmd_export(args),
        ToolkitCommands::ExportEmoticons(args) => ToolkitOperation::ExportEmoticons(args),
        ToolkitCommands::Status { json } => ToolkitOperation::Status { json },
        ToolkitCommands::Run { command, args } => return cmd_run(command, args),
        ToolkitCommands::Decrypt {
            incremental,
            dry_run,
            args,
        } => {
            anyhow::ensure!(args.is_empty(), "decrypt 不支持额外参数，请使用 --help");
            ToolkitOperation::Decrypt {
                incremental,
                dry_run,
            }
        }
        ToolkitCommands::ExportChats {
            output_dir,
            with_transcriptions,
            args,
        } => return export_chats(output_dir, with_transcriptions, args),
        ToolkitCommands::ExportSns(args) => return super::sns_timeline::cmd(args),
        ToolkitCommands::ExportMessages(args) => return super::export_messages::cmd(args),
        ToolkitCommands::SnsArchive(args) => return super::sns_archive::cmd(args),
        ToolkitCommands::FindImageKey(args) => return super::image_keys::cmd(args),
        ToolkitCommands::FindDatabaseKeys(args) => return super::database_keys::cmd(args),
        ToolkitCommands::FindImageKeyMonitor(args) => return super::image_keys::cmd_monitor(args),
        ToolkitCommands::Monitor(args) => return super::monitor_native::cmd_monitor(args),
        ToolkitCommands::Latency(args) => return super::monitor_native::cmd_latency(args),
        ToolkitCommands::DecodeImages {
            attach_dir,
            decoded_dir,
            aes_key,
            xor_key,
            force,
        } => ToolkitOperation::DecodeImages {
            attach_dir,
            decoded_dir,
            aes_key,
            xor_key,
            force,
        },
        ToolkitCommands::DecodeImage {
            dat_file,
            output_file,
        } => ToolkitOperation::DecodeImage {
            dat_file,
            output_file,
        },
        ToolkitCommands::BatchDecryptImages {
            input_dir,
            output_dir,
        } => ToolkitOperation::BatchDecryptImages {
            input_dir,
            output_dir,
        },
        ToolkitCommands::VoiceBatch {
            config,
            output_dir,
            contacts,
        } => ToolkitOperation::VoiceBatch {
            config,
            output_dir,
            contacts,
        },
        ToolkitCommands::VoiceToMp3 { input, output } => {
            ToolkitOperation::VoiceToMp3 { input, output }
        }
        ToolkitCommands::TranscribeChat(args) => return super::asr_batch::cmd(args),
        ToolkitCommands::Web(args) => return super::web_native::cmd_web(args),
        ToolkitCommands::Gui(args) => return super::web_native::cmd_gui(args),
    };
    crate::service::operation_client::run(Operation::Toolkit { operation })
}

#[derive(Parser)]
struct NativeInvocation {
    #[command(subcommand)]
    command: ToolkitCommands,
}

fn cmd_run(command: String, args: Vec<String>) -> Result<()> {
    if matches!(command.as_str(), "export-all" | "export" | "all") {
        let parsed = super::export_all::Args::try_parse_from(
            [format!("wx toolkit run {command}")]
                .into_iter()
                .chain(args),
        )
        .unwrap_or_else(|error| error.exit());
        return crate::service::operation_client::run(Operation::ExportAll {
            args: parsed,
            prepare: command != "export-all",
            announce: command == "all",
        });
    }
    if matches!(command.as_str(), "emoticons" | "decrypt") {
        let invocation = NativeInvocation::try_parse_from(
            [
                "wx toolkit".to_owned(),
                if command == "emoticons" {
                    "export-emoticons"
                } else {
                    "decrypt"
                }
                .to_owned(),
            ]
            .into_iter()
            .chain(args),
        )
        .unwrap_or_else(|error| error.exit());
        if let ToolkitCommands::Decrypt { args, .. } = &invocation.command {
            anyhow::ensure!(args.is_empty(), "decrypt 不支持额外参数，请使用 --help");
        }
        return match invocation.command {
            ToolkitCommands::ExportEmoticons(args) => {
                crate::service::operation_client::run(Operation::PreparedEmoticons { args })
            }
            ToolkitCommands::Decrypt {
                incremental,
                dry_run,
                ..
            } => crate::service::operation_client::run(Operation::PreparedDecrypt {
                incremental,
                dry_run,
            }),
            _ => unreachable!(),
        };
    }
    if matches!(command.as_str(), "status" | "-s") {
        let parsed = RunStatusArgs::try_parse_from(
            ["wx toolkit run status".to_owned()].into_iter().chain(args),
        )
        .unwrap_or_else(|error| error.exit());
        return crate::service::operation_client::run(Operation::RunStatus {
            exported_dir: parsed.exported_dir,
            json: parsed.json,
        });
    }
    if matches!(
        command.as_str(),
        "decode-images"
            | "web"
            | "gui"
            | "decrypt-sns"
            | "find-image-key"
            | "find-image-key-monitor"
            | "find-database-keys"
            | "setup"
            | "cleanup"
            | "export-messages"
            | "export-sns"
            | "transcribe-chat"
            | "export-emoticons"
            | "monitor"
            | "latency"
    ) {
        let invocation = NativeInvocation::try_parse_from(
            ["wx toolkit".to_string(), command].into_iter().chain(args),
        )
        .unwrap_or_else(|error| error.exit());
        return cmd_toolkit(invocation.command);
    }
    if matches!(command.as_str(), "help" | "-h" | "--help") {
        use clap::CommandFactory;
        NativeInvocation::command().print_help()?;
        println!();
        return Ok(());
    }
    anyhow::bail!("未知原生工作流：{command}；使用 wx toolkit --help 查看可用命令")
}

#[derive(Parser)]
#[command(about = "只读统计所选账号配置、数据库、导出文件和语音转录进度")]
struct RunStatusArgs {
    /// 覆盖导出目录；默认配置文件旁的 exported_chats
    #[arg(long)]
    exported_dir: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

fn export_chats(output: Option<String>, transcribe: bool, extra: Vec<String>) -> Result<()> {
    let mut argv = vec!["wx toolkit export-chats".to_owned()];
    if let Some(output) = output {
        argv.push(output);
    }
    if transcribe {
        argv.push("--with-transcriptions".into());
    }
    argv.extend(extra);
    let args = super::export_all::Args::try_parse_from(argv).unwrap_or_else(|error| error.exit());
    crate::service::operation_client::run(Operation::ExportAll {
        args,
        prepare: false,
        announce: false,
    })
}
