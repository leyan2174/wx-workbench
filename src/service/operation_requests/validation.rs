//! Pure request checks. Never open files, load an account, or construct an ASR backend here.
use super::{asr::BackendArgs, asr_batch::BatchArgs};
use crate::service::operations::{Operation, ToolkitOperation};
use anyhow::{ensure, Result};
use std::path::{Component, Path};

fn backend(args: &BackendArgs) -> Result<()> {
    args.validate_explicit()?;
    Ok(())
}

fn batch(args: &BatchArgs) -> Result<()> {
    if args.explicit_backend {
        backend(&args.backend)?;
    }
    Ok(())
}

fn range(start: Option<&str>, end: Option<&str>) -> Result<()> {
    let start = start
        .map(crate::service::time::parse_timestamp)
        .transpose()?;
    let end = end.map(crate::service::time::parse_timestamp).transpose()?;
    ensure!(
        !matches!((start, end), (Some(start), Some(end)) if start > end),
        "起始时间不能晚于结束时间"
    );
    Ok(())
}

fn dates(start: Option<&str>, end: Option<&str>) -> Result<()> {
    let start = start.map(crate::service::time::parse_time).transpose()?;
    let end = end.map(crate::service::time::parse_time_end).transpose()?;
    ensure!(
        !matches!((start, end), (Some(start), Some(end)) if start > end),
        "起始时间不能晚于结束时间"
    );
    Ok(())
}

fn output(path: &Path) -> Result<()> {
    ensure!(
        !path
            .components()
            .any(|part| matches!(part, Component::ParentDir)),
        "输出路径不能包含 .."
    );
    Ok(())
}

fn delta(args: &super::export_delta::Args) -> Result<()> {
    range(Some(&args.start), args.end.as_deref())?;
    let window = crate::application::chat_delta_export::DeltaWindow {
        start: Some(crate::service::time::parse_timestamp(&args.start)?),
        end: args
            .end
            .as_deref()
            .map(crate::service::time::parse_timestamp)
            .transpose()?,
        run_id: args.run_id.clone().unwrap_or_else(|| "request".into()),
        utc_offset_seconds: 0,
        generated_at: String::new(),
    };
    window.validate()?;
    output(&args.output)
}

fn image(args: &super::image_keys::Args) -> Result<()> {
    ensure!(
        args.offline != args.authorize_memory_scan,
        "请选择 --offline 或 --authorize-memory-scan，不能同时使用"
    );
    ensure!(
        (1..=3600).contains(&args.timeout) && (1..=32768).contains(&args.max_mib),
        "扫描预算超出允许范围"
    );
    args.sample.validate_request()
}

fn all(args: &super::export_all::Args) -> Result<()> {
    args.validate()?;
    if args.with_transcriptions && !args.dry_run && args.write_plan_csv.is_none() {
        batch(&args.asr)?;
    }
    Ok(())
}

fn chat_plan(args: &super::chat_plan::Args) -> Result<()> {
    ensure!(
        (1..=6).contains(&args.threads),
        "threads 必须在 1..=6 范围内"
    );
    ensure!(
        !args.users.is_empty() || args.chats_json.is_some(),
        "需要 --user 或 --chats-json"
    );
    ensure!(
        args.source_dir.is_none() || args.media_dir.is_none(),
        "--source-dir 与 --media-dir 冲突"
    );
    range(args.start.as_deref(), args.end.as_deref())
}

fn export_chats(args: &super::export_chats::Args) -> Result<()> {
    range(args.start.as_deref(), args.end.as_deref())?;
    ensure!(
        args.plan_mode.is_none() || args.from_plan_csv.is_some(),
        "--plan-mode 需要 --from-plan-csv"
    );
    Ok(())
}

fn cleanup(args: &super::cleanup_native::Args) -> Result<()> {
    if args.execute {
        ensure!(
            args.mode.is_none()
                && !args.dry_run
                && args.write_plan.is_none()
                && args.native_inventories.is_empty()
                && args.adoption_manifest.is_none(),
            "执行只能使用先前审阅的计划，不接受替换扫描输入"
        );
        ensure!(args.plan.is_some(), "执行缺少 --plan");
        ensure!(args.confirm_account.is_some(), "执行缺少 --confirm-account");
        ensure!(!args.select.is_empty(), "执行缺少 --select");
    } else {
        ensure!(
            args.plan.is_none() && args.select.is_empty(),
            "没有 --execute 时不接受计划执行参数"
        );
    }
    ensure!(
        !(args.authorize_legacy || args.authorize_key_removal) || args.confirm_account.is_some(),
        "接管旧文件或移除密钥需确认账号 ID"
    );
    ensure!(
        args.adoption_manifest.is_none()
            || (args.authorize_legacy && args.confirm_account.is_some()),
        "接管清单需要明确旧文件授权和账号确认"
    );
    ensure!(
        !args.authorize_key_removal
            || (!args.authorize_legacy
                && args.native_inventories.is_empty()
                && args.adoption_manifest.is_none()),
        "密钥移除不能与普通或旧文件清理混用"
    );
    Ok(())
}

fn database(args: &super::asr_database::TranscribeDatabaseNativeArgs) -> Result<()> {
    backend(&args.backend)?;
    ensure!(args.local_id > 0, "local_id must be positive");
    ensure!(
        !args.username.is_empty()
            && args.username.len() <= 1024
            && !args.username.chars().any(char::is_control),
        "invalid username"
    );
    let source = args.source.replace('\\', "/").to_ascii_lowercase();
    ensure!(
        source
            .strip_prefix("message/message_")
            .and_then(|v| v.strip_suffix(".db"))
            .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())),
        "invalid message source"
    );
    ensure!(
        args.cache_file.is_some() == args.cache_account.is_some(),
        "--cache-file and --cache-account must be supplied together"
    );
    if let Some(account) = &args.cache_account {
        ensure!(
            !account.trim().is_empty(),
            "--cache-account must not be empty"
        );
    }
    if let Some(path) = &args.cache_file {
        output(path)?;
        ensure!(
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json")),
            "cache output must be a .json file"
        );
    }
    Ok(())
}

fn setup(args: &super::setup_native::Args) -> Result<()> {
    let explicit = args.db_dir.is_some()
        || args.backend.is_some()
        || args.whisper_binary.is_some()
        || args.whisper_model.is_some()
        || args.local_model.is_some()
        || args.openai_key_env.is_some();
    ensure!(
        !args.check || !(args.apply || args.dry_run || args.interactive || explicit),
        "--check 不能与配置修改或交互参数混用"
    );
    ensure!(!(args.apply && args.dry_run), "--apply 与 --dry-run 冲突");
    ensure!(!args.yes || args.apply, "--yes 需要 --apply");
    ensure!(!args.interactive, "交互输入必须由前端收集后提交");
    ensure!(
        !args.apply || args.yes,
        "非 TTY 写入需要 --apply --yes；缺省只预览"
    );
    if let Some(name) = &args.openai_key_env {
        crate::infrastructure::configuration::valid_env_name(name)?;
    }
    if let Some(model) = &args.local_model {
        ensure!(
            !model.trim().is_empty() && model.len() <= 4096 && !model.chars().any(char::is_control),
            "本地模型配置无效"
        );
    }
    if let Some(backend) = args.backend {
        use super::setup_native::Backend;
        ensure!(
            matches!(backend, Backend::WhisperCpp)
                || (args.whisper_binary.is_none() && args.whisper_model.is_none()),
            "binary/model 参数需要 whisper_cpp 后端"
        );
        ensure!(
            matches!(backend, Backend::PythonWhisper) || args.local_model.is_none(),
            "local-model 参数需要 python_whisper 后端"
        );
        ensure!(
            matches!(backend, Backend::OpenAiCompatible) || args.openai_key_env.is_none(),
            "凭据环境变量参数需要 openai_compatible 后端"
        );
    }
    Ok(())
}

fn directory(args: &super::export_messages::Args) -> Result<()> {
    if let Some(formats) = &args.formats {
        crate::application::chat_directory::Format::parse_list(formats)?;
    }
    if let Some(bytes) = args.max_media_bytes {
        ensure!(
            (1..=500 * 1024 * 1024).contains(&bytes),
            "单附件上限必须在 1..500MiB"
        );
    }
    if let Some(total) = args.max_total_media_bytes {
        ensure!(
            total > 0 && args.max_media_bytes.is_none_or(|single| total >= single),
            "累计媒体预算不能小于单附件上限"
        );
    }
    Ok(())
}

pub(crate) fn validate(operation: &Operation) -> Result<()> {
    match operation {
        Operation::Initialize {
            force,
            provider,
            restart,
            executable,
            timeout,
            ..
        } => {
            use super::key_provider::KeyProvider;
            ensure!(
                !restart || *provider == KeyProvider::Account,
                "--restart-wechat 仅用于 --key-provider account"
            );
            ensure!(
                *provider != KeyProvider::Account || (*force && *restart),
                "账号级捕获需要 --force --key-provider account --restart-wechat"
            );
            ensure!(
                executable.is_none() || *provider == KeyProvider::Account,
                "--wechat-exe 仅用于 --key-provider account"
            );
            ensure!(
                (10..=1800).contains(timeout),
                "capture timeout must be in 10..=1800"
            );
            Ok(())
        }
        Operation::DatabaseKeys { args } => {
            ensure!(
                args.authorize_memory_scan,
                "数据库取钥需要 --authorize-memory-scan 明确授权"
            );
            Ok(())
        }
        Operation::ImageKeys { args } => image(args),
        Operation::ImageKeyMonitor { args } => args.validate_request(),
        Operation::TranscribeAudio { args } => backend(&args.backend),
        Operation::TranscribeChat { args } => backend(&args.backend),
        Operation::TranscribeDatabase { args } => database(args),
        Operation::TranscribeBatch { args } => batch(&args.batch),
        Operation::ExportAll { args, .. } => all(args),
        Operation::ExportDelta { args } => delta(args),
        Operation::ExportChats { args } => export_chats(args),
        Operation::ChatPlan { args } => chat_plan(args),
        Operation::Monitor { args } => args.options().map(|_| ()),
        Operation::Latency { args } => args.options().map(|_| ()),
        Operation::Export {
            since,
            until,
            format,
            ..
        } => {
            ensure!(
                ["markdown", "txt", "json", "yaml"].contains(&format.as_str()),
                "不支持的导出格式"
            );
            dates(since.as_deref(), until.as_deref())
        }
        Operation::Voices { since, until, .. } => dates(since.as_deref(), until.as_deref()),
        Operation::SnsAlbum { args } => {
            ensure!(!args.user.trim().is_empty(), "相册作者不能为空");
            ensure!(
                !args.adopt_existing || args.output_dir.is_some(),
                "认领旧相册需要 --output-dir"
            );
            ensure!(
                args.output_dir.is_none() || args.output == Path::new("."),
                "--output-dir 与 --output 冲突"
            );
            dates(args.since.as_deref(), args.until.as_deref())
        }
        Operation::Setup { args } => setup(args),
        Operation::SetupPreview { args } => {
            ensure!(
                !args.check && !args.apply && !args.yes,
                "预览凭据只能用于配置预览"
            );
            setup(args)
        }
        Operation::SetupApply { args, review } => {
            ensure!(
                args.apply && args.yes && !args.check,
                "已审阅写入需要 --apply --yes"
            );
            setup(args)?;
            ensure!(
                review.config_path.is_absolute()
                    && args.config_path.as_ref() == Some(&review.config_path),
                "预览与写入必须固定同一配置路径"
            );
            let digest = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
            ensure!(
                digest(&review.arguments_sha256)
                    && review.snapshot_sha256.as_deref().is_none_or(digest),
                "无效的预览凭据"
            );
            ensure!(
                super::setup_native::argument_fingerprint(args, &review.config_path)?
                    == review.arguments_sha256,
                "预览后参数已改变，请重新预览"
            );
            Ok(())
        }
        Operation::ExportMessages { args } => directory(args),
        Operation::SnsTimeline { args } => {
            ensure!(
                !(args.download_media && args.no_remote),
                "--download-media 与 --no-remote 冲突"
            );
            Ok(())
        }
        Operation::Extract { attachment_id, .. } => {
            crate::attachment::AttachmentId::decode(attachment_id)?;
            Ok(())
        }
        Operation::Toolkit { operation } => toolkit(operation),
        Operation::Cleanup { args } => cleanup(args),
        Operation::ExportChat { .. }
        | Operation::SnsArchive { .. }
        | Operation::NewMessages { .. }
        | Operation::RunStatus { .. } => Ok(()),
    }
}

fn toolkit(operation: &ToolkitOperation) -> Result<()> {
    match operation {
        ToolkitOperation::ExportSnsNative {
            update,
            adopt_existing,
            utc_offset,
            local_cache,
            ..
        } => {
            ensure!(!adopt_existing || *update, "--adopt-existing 需要 --update");
            if let Some(offset) = utc_offset {
                offset.parse::<chrono::FixedOffset>()?;
            }
            local_cache.validate_request()?;
            Ok(())
        }
        ToolkitOperation::DecodeImages {
            aes_key, xor_key, ..
        } => {
            if let Some(key) = aes_key {
                crate::application::image_publication::parse_aes(key)?;
            }
            if let Some(key) = xor_key {
                crate::application::image_publication::parse_xor(key)?;
            }
            Ok(())
        }
        ToolkitOperation::DecodeSnsVideo { .. }
        | ToolkitOperation::ExportEmoticons(_)
        | ToolkitOperation::Status { .. }
        | ToolkitOperation::Decrypt { .. }
        | ToolkitOperation::DecodeImage { .. }
        | ToolkitOperation::BatchDecryptImages { .. }
        | ToolkitOperation::VoiceBatch { .. }
        | ToolkitOperation::VoiceToMp3 { .. } => Ok(()),
    }
}
