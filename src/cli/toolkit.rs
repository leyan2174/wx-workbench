use anyhow::Result;
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::path::PathBuf;

use super::output::{print_value, resolve};
use crate::toolkit as native;

use crate::toolkit::legacy::{python_available, toolkit_python, toolkit_root};

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
    /// 查询或导出企业微信已解密离线快照，不自动发现账号
    Enterprise {
        /// 包含 message.db，可选 user.db / session.db 的离线目录
        snapshot: PathBuf,
        /// 本人企业账号数字 ID；不指定时不推断“我”
        #[arg(long)]
        self_id: Option<i64>,
        #[command(subcommand)]
        command: super::enterprise::QueryCommand,
    },
    /// 原生朋友圈预览：JSON / HTML，默认离线，可显式恢复缓存或下载媒体
    ExportSnsNative {
        /// 已解密 SNS SQLite 数据库
        sns_db: PathBuf,
        /// 新输出目录，须位于源数据库目录之外
        output_dir: PathBuf,
        /// 已解密联系人数据库
        #[arg(long)]
        contact_db: Option<PathBuf>,
        /// 按 user_name 筛选，逗号分隔；默认读取 WECHAT_EXPORT_CONTACTS
        #[arg(long)]
        contacts: Option<String>,
        /// 可选固定时区偏移，例如 +08:00；默认本机时区
        #[arg(long, allow_hyphen_values = true)]
        utc_offset: Option<String>,
        /// 显式授权下载未从本地缓存恢复的媒体；默认不联网
        #[arg(long)]
        download_media: bool,
        /// 创建或更新绑定同一数据库来源的时间线；默认仍为 fresh 整目录发布
        #[arg(long)]
        update: bool,
        /// 显式认领无来源绑定的旧时间线；已知来源冲突仍拒绝
        #[arg(long, requires = "update")]
        adopt_existing: bool,
        #[command(flatten)]
        local_cache: super::export_sns::LocalCacheArgs,
    },
    /// 解密企业微信离线主库：4096 字节页；拒绝 WAL/日志和已有输出
    DecryptEnterprise {
        /// 输入数据库文件
        input: PathBuf,
        /// 输出明文数据库文件
        output: PathBuf,
        /// UTF-8 文本文件，包含 32 位十六进制原始密钥；不通过命令行传密钥
        #[arg(long)]
        key_file: PathBuf,
    },
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
    /// 企业微信授权取钥、批量主库解密及多会话多格式导出
    EnterpriseBatch(super::enterprise_batch::Args),
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

#[derive(Serialize)]
struct ToolkitStatus {
    native_commands: Vec<&'static str>,
    wechat_decrypt_dir: String,
    wechat_decrypt_dir_exists: bool,
    python: String,
    python_exists: bool,
    config_json: String,
    config_json_exists: bool,
    scripts: Vec<ScriptStatus>,
    env_overrides: EnvOverrides,
}

#[derive(Serialize)]
struct ScriptStatus {
    name: &'static str,
    path: String,
    exists: bool,
}

#[derive(Serialize)]
struct EnvOverrides {
    wx_wechat_decrypt_dir: Option<String>,
    wx_wechat_decrypt_python: Option<String>,
}

pub fn cmd_toolkit(cmd: ToolkitCommands) -> Result<()> {
    match cmd {
        ToolkitCommands::Setup(args) => super::setup_native::cmd(args),
        ToolkitCommands::Cleanup(args) => super::cleanup_native::cmd(args),
        ToolkitCommands::TranscribeDatabaseNative(args) => {
            super::asr_database::cmd_transcribe_database_native(args)
        }
        ToolkitCommands::ExportDeltaNative(args) => super::export_delta::cmd(args),
        ToolkitCommands::ChatPlanNative(args) => super::chat_plan::cmd(args),
        ToolkitCommands::TranscribeAudioNative(args) => {
            super::asr::cmd_transcribe_audio_native(args)
        }
        ToolkitCommands::TranscribeChatNative(args) => super::asr::cmd_transcribe_chat_native(args),
        ToolkitCommands::DecodeSnsVideo {
            input,
            output,
            key_file,
            wasm,
        } => super::sns_video::cmd_decode(input, output, key_file, wasm),
        ToolkitCommands::Enterprise {
            snapshot,
            self_id,
            command,
        } => super::enterprise::cmd_query(snapshot, self_id, command),
        ToolkitCommands::ExportSnsNative {
            sns_db,
            output_dir,
            contact_db,
            contacts,
            utc_offset,
            download_media,
            update,
            adopt_existing,
            local_cache,
        } => super::export_sns::cmd_export(
            sns_db,
            contact_db,
            output_dir,
            contacts,
            utc_offset,
            local_cache,
            download_media,
            update,
            adopt_existing,
        ),
        ToolkitCommands::DecryptEnterprise {
            input,
            output,
            key_file,
        } => super::enterprise::cmd_decrypt(input, output, key_file),
        ToolkitCommands::ExportChatsNative(args) => super::export_chats::cmd_export(args),
        ToolkitCommands::ExportEmoticons(args) => {
            let runtime = crate::runtime::RuntimeContext::load()?;
            let keys = super::toolkit_run_prepare::load_saved(&runtime)?;
            super::export_emoticons::export(runtime, keys, args)
        }
        ToolkitCommands::Status { json } => cmd_status(json),
        ToolkitCommands::Run { command, args } => cmd_run(command, args),
        ToolkitCommands::Decrypt {
            incremental,
            dry_run,
            args,
        } => {
            anyhow::ensure!(args.is_empty(), "decrypt 不支持额外参数，请使用 --help");
            let runtime = crate::runtime::RuntimeContext::load()?;
            let keys = super::toolkit_run_prepare::load_saved(&runtime)?;
            native::decrypt(
                &runtime,
                &keys,
                incremental,
                dry_run,
                native::DecryptMode::Strict,
            )
        }
        ToolkitCommands::ExportChats {
            output_dir,
            with_transcriptions,
            args,
        } => super::export_all::cmd(output_dir, with_transcriptions, args),
        ToolkitCommands::ExportSns(args) => super::sns_timeline::cmd(args),
        ToolkitCommands::ExportMessages(args) => super::export_messages::cmd(args),
        ToolkitCommands::EnterpriseBatch(args) => super::enterprise_batch::cmd(args),
        ToolkitCommands::SnsArchive(args) => super::sns_archive::cmd(args),
        ToolkitCommands::FindImageKey(args) => super::image_keys::cmd(args),
        ToolkitCommands::FindDatabaseKeys(args) => super::database_keys::cmd(args),
        ToolkitCommands::FindImageKeyMonitor(args) => super::image_keys::cmd_monitor(args),
        ToolkitCommands::Monitor(args) => super::monitor_native::cmd_monitor(args),
        ToolkitCommands::Latency(args) => super::monitor_native::cmd_latency(args),
        ToolkitCommands::DecodeImages {
            attach_dir,
            decoded_dir,
            aes_key,
            xor_key,
            force,
        } => native::decode_images(attach_dir, decoded_dir, aes_key, xor_key, force),
        ToolkitCommands::DecodeImage {
            dat_file,
            output_file,
        } => native::decode_image(dat_file, output_file),
        ToolkitCommands::BatchDecryptImages {
            input_dir,
            output_dir,
        } => native::batch_images(input_dir, output_dir),
        ToolkitCommands::VoiceToMp3 { input, output } => {
            let output = output.unwrap_or_else(|| {
                let mut path = PathBuf::from(&input);
                path.set_extension("mp3");
                path.to_string_lossy().into_owned()
            });
            let result = native::audio::convert_silk_to_mp3(
                std::path::Path::new(&input),
                std::path::Path::new(&output),
            )?;
            println!("{}", serde_json::to_string(&result)?);
            Ok(())
        }
        ToolkitCommands::VoiceBatch {
            config,
            output_dir,
            contacts,
        } => {
            let mut options = native::audio::batch::BatchOptions::from_config_file(&config)?;
            if let Some(output) = output_dir {
                options.output_dir = output;
            }
            if let Some(contacts) = contacts {
                options.contacts = native::audio::batch::parse_contact_filter(&contacts);
            }
            let report = native::audio::batch::convert_database(&options)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            anyhow::ensure!(
                report.failed == 0,
                "{} 条语音转换失败，其他结果已保留",
                report.failed
            );
            Ok(())
        }
        ToolkitCommands::TranscribeChat(args) => super::asr_batch::cmd(args),
        ToolkitCommands::Web(args) => super::web_native::cmd_web(args),
        ToolkitCommands::Gui(args) => super::web_native::cmd_gui(args),
    }
}

// 复用直接命令的参数定义，防止兼容入口与原生入口逐渐出现差异。
#[derive(Parser)]
struct NativeInvocation {
    #[command(subcommand)]
    command: ToolkitCommands,
}

fn cmd_run(command: String, args: Vec<String>) -> Result<()> {
    if command == "export-all" {
        let parsed = super::export_all::Args::try_parse_from(
            ["wx toolkit run export-all".to_owned()]
                .into_iter()
                .chain(args),
        )
        .unwrap_or_else(|error| error.exit());
        parsed.validate()?;
        return super::export_all::emit(super::export_all::export_for(
            &crate::runtime::RuntimeContext::load()?,
            parsed,
        )?);
    }
    if matches!(command.as_str(), "export" | "all") {
        let parsed = super::export_all::Args::try_parse_from(
            [format!("wx toolkit run {command}")]
                .into_iter()
                .chain(args),
        )
        .unwrap_or_else(|error| error.exit());
        parsed.validate()?;
        if parsed.dry_run {
            let runtime = crate::runtime::RuntimeContext::load()?;
            return super::export_all::emit(super::export_all::export_for(&runtime, parsed)?);
        }
        let prepared = super::toolkit_run_prepare::prepare()?;
        native::decrypt(
            &prepared.runtime,
            &prepared.keys,
            false,
            false,
            native::DecryptMode::Legacy,
        )?;
        let transcribed = parsed.with_transcriptions;
        super::export_all::emit(super::export_all::export_for(&prepared.runtime, parsed)?)?;
        if command == "all" && !transcribed {
            eprintln!("全量导出完成；语音转录需显式指定 --with-transcriptions 及后端配置。");
        }
        return Ok(());
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
        let prepared = super::toolkit_run_prepare::prepare()?;
        return match invocation.command {
            ToolkitCommands::ExportEmoticons(args) => {
                super::export_emoticons::export(prepared.runtime, prepared.keys, args)
            }
            ToolkitCommands::Decrypt {
                incremental,
                dry_run,
                ..
            } => native::decrypt(
                &prepared.runtime,
                &prepared.keys,
                incremental,
                dry_run,
                native::DecryptMode::Legacy,
            ),
            _ => unreachable!(),
        };
    }
    if matches!(command.as_str(), "status" | "-s") {
        let parsed = RunStatusArgs::try_parse_from(
            ["wx toolkit run status".to_owned()].into_iter().chain(args),
        )
        .unwrap_or_else(|error| error.exit());
        let status = native::run_status::inspect(
            &crate::config::find_config_file()?,
            parsed.exported_dir.as_deref(),
        )?;
        if parsed.json {
            println!("{}", serde_json::to_string_pretty(&status)?);
        } else {
            print!("{}", status.render());
        }
        return Ok(());
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
            | "enterprise-batch"
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

fn cmd_status(json: bool) -> Result<()> {
    let root = toolkit_root();
    let python = toolkit_python();
    let scripts = [
        "main.py",
        "decrypt_db.py",
        "export_all_chats.py",
        "export_sns.py",
        "export_sns_album.py",
        "sns_media_wasm/wasm_video_decode.js",
        "sns_media_wasm/wasm_video_decode.wasm",
        "sns_media_wasm/weflow_wasm_keystream.js",
        "decode_image.py",
        "batch_decrypt_images.py",
        "voice_to_mp3.py",
        "transcribe_chat.py",
        "monitor_web.py",
        "app_gui.py",
    ]
    .into_iter()
    .map(|name| {
        let path = root.join(name);
        ScriptStatus {
            name,
            path: path.to_string_lossy().into_owned(),
            exists: path.exists(),
        }
    })
    .collect();

    let config = root.join("config.json");
    let status = ToolkitStatus {
        native_commands: vec![
            "run status",
            "run decrypt",
            "run emoticons",
            "run decode-images",
            "export-emoticons",
            "transcribe-database-native",
            "export-delta-native",
            "chat-plan-native",
            "transcribe-audio-native",
            "transcribe-chat-native",
            "decode-sns-video",
            "decrypt",
            "decode-image",
            "decode-images",
            "batch-decrypt-images",
            "voice-to-mp3",
            "voice-batch",
            "export-chats-native",
            "decrypt-enterprise",
            "enterprise",
            "export-sns-native",
        ],
        wechat_decrypt_dir: root.to_string_lossy().into_owned(),
        wechat_decrypt_dir_exists: root.is_dir(),
        python: python.to_string_lossy().into_owned(),
        python_exists: python_available(&python),
        config_json: config.to_string_lossy().into_owned(),
        config_json_exists: config.is_file(),
        scripts,
        env_overrides: EnvOverrides {
            wx_wechat_decrypt_dir: std::env::var("WX_WECHAT_DECRYPT_DIR").ok(),
            wx_wechat_decrypt_python: std::env::var("WX_WECHAT_DECRYPT_PYTHON").ok(),
        },
    };
    print_value(&serde_json::to_value(status)?, &resolve(json))
}
