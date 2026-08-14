use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde::Serialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::output::{print_value, resolve};

const BUNDLED_WECHAT_DECRYPT_DIR: &str = r"vendor\wechat-decrypt";

#[derive(Subcommand)]
pub enum ToolkitCommands {
    /// 显示本机 wechat-decrypt 源码、Python 环境和可用能力
    Status {
        /// 输出 JSON（默认 YAML）
        #[arg(long)]
        json: bool,
    },
    /// 运行 wechat-decrypt 一键入口：status / decrypt / decode-images / export / all / emoticons / web
    Run {
        /// main.py 子命令；省略时启动 Web UI
        #[arg(default_value = "web")]
        command: String,
        /// 透传给 wechat-decrypt main.py 的参数
        #[arg(last = true, trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// 提取密钥并解密全部数据库
    Decrypt {
        /// 增量模式
        #[arg(short = 'i', long)]
        incremental: bool,
        /// 只预览，不写出
        #[arg(long)]
        dry_run: bool,
        /// 额外透传参数
        #[arg(last = true, trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// 批量导出全部聊天记录
    ExportChats {
        /// 输出目录；默认使用 wechat-decrypt 的 exported_chats
        output_dir: Option<String>,
        /// 附带语音转录
        #[arg(short = 't', long)]
        with_transcriptions: bool,
        /// 额外透传参数
        #[arg(last = true, trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// 导出朋友圈时间线和缓存图片
    ExportSns {
        /// 用 WECHAT_EXPORT_CONTACTS 限定联系人，逗号分隔
        #[arg(long)]
        contacts: Option<String>,
    },
    /// 批量解密微信 .dat 图片
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
    /// 批量解密一个目录下的 .dat 图片
    BatchDecryptImages {
        /// 输入目录
        input_dir: String,
        /// 输出目录
        output_dir: Option<String>,
    },
    /// 把 SILK 语音转换成 MP3
    VoiceToMp3 {
        /// 输入文件或目录，透传给 voice_to_mp3.py
        input: String,
        /// 输出文件或目录
        output: Option<String>,
    },
    /// 对导出的聊天 JSON 做语音转录
    TranscribeChat {
        /// 输入聊天 JSON
        input_json: String,
        /// 输出 JSON
        output_json: Option<String>,
    },
    /// 启动 wechat-decrypt Web UI
    Web,
    /// 启动 wechat-decrypt 桌面 GUI
    Gui,
}

#[derive(Serialize)]
struct ToolkitStatus {
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
        ToolkitCommands::Status { json } => cmd_status(json),
        ToolkitCommands::Run { command, args } => run_main(command, args),
        ToolkitCommands::Decrypt {
            incremental,
            dry_run,
            args,
        } => {
            let mut argv = Vec::new();
            if incremental {
                argv.push("--incremental".to_string());
            }
            if dry_run {
                argv.push("--dry-run".to_string());
            }
            argv.extend(args);
            run_script("decrypt_db.py", argv, None)
        }
        ToolkitCommands::ExportChats {
            output_dir,
            with_transcriptions,
            args,
        } => {
            let mut argv = Vec::new();
            if let Some(output_dir) = output_dir {
                argv.push(output_dir);
            }
            if with_transcriptions {
                argv.push("--with-transcriptions".to_string());
            }
            argv.extend(args);
            run_script("export_all_chats.py", argv, None)
        }
        ToolkitCommands::ExportSns { contacts } => {
            let mut envs = Vec::new();
            if let Some(contacts) = contacts {
                envs.push(("WECHAT_EXPORT_CONTACTS".to_string(), contacts));
            }
            run_script("export_sns.py", Vec::new(), Some(envs))
        }
        ToolkitCommands::DecodeImages {
            attach_dir,
            decoded_dir,
            aes_key,
            xor_key,
            force,
        } => {
            let mut argv = vec!["decode-images".to_string()];
            push_opt(&mut argv, "--attach-dir", attach_dir);
            push_opt(&mut argv, "--decoded-dir", decoded_dir);
            push_opt(&mut argv, "--aes-key", aes_key);
            push_opt(&mut argv, "--xor-key", xor_key);
            if force {
                argv.push("--force".to_string());
            }
            run_script("main.py", argv, None)
        }
        ToolkitCommands::DecodeImage {
            dat_file,
            output_file,
        } => {
            let mut argv = vec![dat_file];
            if let Some(output_file) = output_file {
                argv.push(output_file);
            }
            run_script("decode_image.py", argv, None)
        }
        ToolkitCommands::BatchDecryptImages {
            input_dir,
            output_dir,
        } => {
            let mut argv = vec![input_dir];
            if let Some(output_dir) = output_dir {
                argv.push(output_dir);
            }
            run_script("batch_decrypt_images.py", argv, None)
        }
        ToolkitCommands::VoiceToMp3 { input, output } => {
            let output = output.unwrap_or_else(|| {
                let mut path = PathBuf::from(&input);
                path.set_extension("mp3");
                path.to_string_lossy().into_owned()
            });
            run_script("wx_toolkit_voice_to_mp3.py", vec![input, output], None)
        }
        ToolkitCommands::TranscribeChat {
            input_json,
            output_json,
        } => {
            let mut argv = vec![input_json];
            if let Some(output_json) = output_json {
                argv.push(output_json);
            }
            run_script("transcribe_chat.py", argv, None)
        }
        ToolkitCommands::Web => run_main("web".to_string(), Vec::new()),
        ToolkitCommands::Gui => run_script("app_gui.py", Vec::new(), None),
    }
}

fn cmd_status(json: bool) -> Result<()> {
    let root = toolkit_root();
    let python = toolkit_python();
    let scripts = [
        "main.py",
        "decrypt_db.py",
        "export_all_chats.py",
        "export_sns.py",
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

fn run_main(command: String, args: Vec<String>) -> Result<()> {
    let mut argv = vec![command];
    argv.extend(args);
    run_script("main.py", argv, None)
}

fn run_script(
    script: &str,
    args: Vec<String>,
    extra_env: Option<Vec<(String, String)>>,
) -> Result<()> {
    let root = toolkit_root();
    let python = toolkit_python();
    ensure_toolkit_ready(&root, &python, script)?;
    let script_path = root.join(script);
    let mut command = Command::new(&python);
    command
        .arg(&script_path)
        .args(args.iter().map(OsString::from))
        .current_dir(&root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    command.env("WECHAT_DECRYPT_APP_DIR", &root);
    command.env("PYTHONUTF8", "1");
    if let Some(extra_env) = extra_env {
        for (key, value) in extra_env {
            command.env(key, value);
        }
    }
    let status = command
        .status()
        .with_context(|| format!("启动 wechat-decrypt 脚本失败: {}", script_path.display()))?;
    if !status.success() {
        let code = status
            .code()
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        bail!("wechat-decrypt 脚本退出失败: {script}，exit={code}");
    }
    Ok(())
}

fn ensure_toolkit_ready(root: &Path, python: &Path, script: &str) -> Result<()> {
    if !root.is_dir() {
        bail!(
            "找不到 wechat-decrypt 源码目录: {}。可用 WX_WECHAT_DECRYPT_DIR 覆盖。",
            root.display()
        );
    }
    if !python_available(python) {
        bail!(
            "找不到 wechat-decrypt Python: {}。可用 WX_WECHAT_DECRYPT_PYTHON 覆盖。",
            python.display()
        );
    }
    let script_path = root.join(script);
    if !script_path.is_file() {
        bail!("找不到 wechat-decrypt 脚本: {}", script_path.display());
    }
    Ok(())
}

fn toolkit_root() -> PathBuf {
    if let Some(root) = std::env::var_os("WX_WECHAT_DECRYPT_DIR") {
        return PathBuf::from(root);
    }

    for candidate in toolkit_root_candidates() {
        if candidate.is_dir() {
            return candidate;
        }
    }

    PathBuf::from(BUNDLED_WECHAT_DECRYPT_DIR)
}

fn toolkit_root_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join(BUNDLED_WECHAT_DECRYPT_DIR));
            if let Some(project_dir) = exe_dir.parent().and_then(|target_dir| target_dir.parent()) {
                candidates.push(project_dir.join(BUNDLED_WECHAT_DECRYPT_DIR));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(BUNDLED_WECHAT_DECRYPT_DIR));
    }
    candidates.push(PathBuf::from(BUNDLED_WECHAT_DECRYPT_DIR));
    candidates
}

fn toolkit_python() -> PathBuf {
    if let Some(python) = std::env::var_os("WX_WECHAT_DECRYPT_PYTHON") {
        return PathBuf::from(python);
    }

    for candidate in toolkit_python_candidates() {
        if candidate.is_file() {
            return candidate;
        }
    }

    PathBuf::from("python")
}

fn toolkit_python_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join(".venv").join("Scripts").join("python.exe"));
            candidates.push(exe_dir.join("python").join("python.exe"));
            if let Some(project_dir) = exe_dir.parent().and_then(|target_dir| target_dir.parent()) {
                candidates.push(project_dir.join(".venv").join("Scripts").join("python.exe"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(".venv").join("Scripts").join("python.exe"));
    }
    candidates
}

fn python_available(python: &Path) -> bool {
    if python.is_file() {
        return true;
    }
    if python.components().count() == 1 {
        return Command::new(python)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
    }
    false
}

fn push_opt(argv: &mut Vec<String>, name: &str, value: Option<String>) {
    if let Some(value) = value {
        argv.push(name.to_string());
        argv.push(value);
    }
}
