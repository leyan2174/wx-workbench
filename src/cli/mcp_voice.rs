//! MCP 主机语音执行；后台仅提供经过验证的原始音频，不接触后端凭证。
use super::asr::{BackendArgs, BackendKind};
use crate::{
    attachment::local_files::HostOutputGuard,
    ipc::{Response, MAX_PREPARED_VOICE_RESPONSE_BYTES},
    mcp::protocol::{CallContext, DispatchError},
    runtime::RuntimeContext,
    toolkit::{
        asr::{self, cached, prepared_audio, receipt},
        audio::publish::publish_wav_noclobber,
    },
};
use chrono::{Local, TimeZone};
use serde_json::json;
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

#[derive(clap::Args, Debug, Clone, Default)]
pub struct Args {
    #[command(flatten)]
    pub backend: BackendArgs,
    /// 显式主机转录缓存文件；省略时不持久化转录缓存。
    #[arg(long)]
    pub voice_cache_file: Option<PathBuf>,
}

#[derive(Clone, Copy)]
pub enum Operation {
    Decode,
    Transcribe,
}

pub struct Pending {
    operation: Operation,
    configured_local_python: bool,
    media_id: i64,
    args: Args,
    output: Option<HostOutputGuard>,
    cache: Option<HostOutputGuard>,
    bound: Option<RuntimeContext>,
    expected_username: Option<String>,
    prepared_backend: Option<asr::Backend>,
    // 字段按声明顺序释放：先关闭路径守卫，再清理请求独占临时目录。
    _temporary: Option<tempfile::TempDir>,
}

impl Args {
    pub fn prepare(
        &self,
        operation: Operation,
        media_id: i64,
        output_root: Option<&Path>,
        context: &CallContext,
    ) -> Result<Pending, DispatchError> {
        self.prepare_with_mode(operation, media_id, output_root, context, false)
    }

    /// 仅由宿主启动参数调用；模型和引擎只能在绑定账号后从固定配置读取。
    pub fn prepare_configured_local(
        &self,
        media_id: i64,
        context: &CallContext,
    ) -> Result<Pending, DispatchError> {
        self.prepare_with_mode(Operation::Transcribe, media_id, None, context, true)
    }

    fn prepare_with_mode(
        &self,
        operation: Operation,
        media_id: i64,
        output_root: Option<&Path>,
        context: &CallContext,
        configured_local_python: bool,
    ) -> Result<Pending, DispatchError> {
        context.check()?;
        let mut args = self.clone();
        let mut temporary = None;
        // 参数和上传授权先于账号、模型、凭证以及后台读取。
        let output = match operation {
            Operation::Decode => Some(
                HostOutputGuard::new(&host_path(output_root.ok_or(DispatchError::Unavailable)?)?)
                    .map_err(|_| DispatchError::Unavailable)?,
            ),
            Operation::Transcribe => {
                let b = &self.backend;
                let valid = b.timeout_seconds > 0
                    && !b.language.trim().is_empty()
                    && if configured_local_python {
                        matches!(b.backend, BackendKind::Local)
                            && !b.allow_upload
                            && b.openai_base_url.is_none()
                            && b.openai_model.is_none()
                            && b.api_key_file.is_none()
                            && b.whisper_binary.is_none()
                            && b.whisper_model.is_none()
                            && b.temp_root.is_none()
                            && b.threads != Some(0)
                    } else { match b.backend {
                        BackendKind::Local => {
                            !b.allow_upload
                                && b.openai_base_url.is_none()
                                && b.openai_model.is_none()
                                && b.api_key_file.is_none()
                                && b.whisper_binary.is_some()
                                && b.whisper_model.is_some()
                                && b.threads != Some(0)
                        }
                        BackendKind::ExplicitOpenAi => {
                            b.allow_upload
                                && b.openai_base_url.is_some()
                                && b.openai_model.is_some()
                                && b.api_key_file.is_some()
                                && b.whisper_binary.is_none()
                                && b.whisper_model.is_none()
                                && b.threads.is_none()
                                && b.temp_root.is_none()
                        }
                    }};
                if !valid {
                    return Err(DispatchError::Unavailable);
                }
                for path in [
                    &mut args.backend.whisper_binary,
                    &mut args.backend.whisper_model,
                    &mut args.backend.api_key_file,
                    &mut args.backend.temp_root,
                    &mut args.voice_cache_file,
                ]
                .into_iter()
                .flatten()
                {
                    *path = host_path(path)?;
                }
                match b.backend {
                    BackendKind::Local => {
                        if args.backend.temp_root.is_none() {
                            let directory = tempfile::Builder::new()
                                .prefix("wx-cli-mcp-voice-")
                                .tempdir()
                                .map_err(|_| DispatchError::Unavailable)?;
                            args.backend.temp_root = Some(directory.path().to_owned());
                            temporary = Some(directory);
                        }
                        Some(
                            HostOutputGuard::new(
                                args.backend
                                    .temp_root
                                    .as_deref()
                                    .ok_or(DispatchError::Unavailable)?,
                            )
                            .map_err(|_| DispatchError::Unavailable)?,
                        )
                    }
                    BackendKind::ExplicitOpenAi => None,
                }
            }
        };
        let cache = if matches!(operation, Operation::Transcribe) {
            args.voice_cache_file
                .as_ref()
                .map(|path| {
                    let guard =
                        HostOutputGuard::new(path.parent().ok_or(DispatchError::Unavailable)?)
                            .map_err(|_| DispatchError::Unavailable)?;
                    guard
                        .verify_replaceable_file(path)
                        .map_err(|_| DispatchError::Unavailable)?;
                    Ok(guard)
                })
                .transpose()?
        } else {
            None
        };
        Ok(Pending {
            operation,
            configured_local_python,
            media_id,
            args,
            output,
            cache,
            bound: None,
            expected_username: None,
            prepared_backend: None,
            _temporary: temporary,
        })
    }
}

impl Pending {
    pub fn bind_username(&mut self, username: String) -> Result<(), DispatchError> {
        if username.trim().is_empty() || username.len() > 4096 {
            return Err(DispatchError::InvalidResponse);
        }
        self.expected_username = Some(username);
        Ok(())
    }

    /// 只读历史强身份索引；命中不启动后台或识别进程，未命中仍走真实音频流程。
    pub fn try_cached(
        &mut self,
        exact_username: &str,
        context: &CallContext,
        before_return: impl Fn() -> Result<(), DispatchError>,
    ) -> Result<Option<Response>, DispatchError> {
        let Some(path) = &self.args.voice_cache_file else {
            return Ok(None);
        };
        if !matches!(self.operation, Operation::Transcribe) {
            return Ok(None);
        }
        let bound = self.bound.as_ref().ok_or(DispatchError::Unavailable)?;
        self.verify()?;
        context.check()?;
        before_return()?;
        if self.prepared_backend.is_none() {
            self.prepared_backend = Some(
                self.args
                    .backend
                    .clone()
                    .build()
                    .map_err(|_| DispatchError::Unavailable)?,
            );
        }
        limit_backend(
            self.prepared_backend.as_mut().expect("backend prepared"),
            context,
        )?;
        let outcome = receipt::lookup_success(
            path,
            &bound.id,
            exact_username,
            self.media_id,
            self.prepared_backend.as_ref().expect("backend prepared"),
        );
        self.verify()?;
        context.check()?;
        before_return()?;
        match outcome {
            receipt::LookupOutcome::Hit(hit) => {
                let text = transcription_text(hit.create_time, &hit.transcription)?;
                context.check_text_result(&text)?;
                Ok(Some(Response::ok(json!({"mcp_text":text}))))
            }
            receipt::LookupOutcome::Miss
            | receipt::LookupOutcome::Conflict
            | receipt::LookupOutcome::Unavailable => Ok(None),
        }
    }

    pub fn bind(mut self, runtime: &RuntimeContext) -> Result<Self, DispatchError> {
        if self.bound.is_some() {
            return Err(DispatchError::Unavailable);
        }
        if self.configured_local_python {
            self.prepared_backend = Some(configured_local_backend(runtime, &self.args.backend)?);
        }
        for guard in self.output.iter_mut().chain(self.cache.iter_mut()) {
            protect_runtime(guard, runtime).map_err(|_| DispatchError::Unavailable)?;
            if matches!(self.operation, Operation::Transcribe) {
                for path in [
                    &self.args.backend.whisper_binary,
                    &self.args.backend.whisper_model,
                    &self.args.backend.api_key_file,
                ]
                .into_iter()
                .flatten()
                {
                    guard
                        .pin_input(path)
                        .map_err(|_| DispatchError::Unavailable)?;
                }
                if let Some(asr::Backend::LegacyPythonLocal(config)) = &self.prepared_backend {
                    for path in config.host_input_paths() {
                        guard.pin_input(&path).map_err(|_| DispatchError::Unavailable)?;
                    }
                }
            }
        }
        if let (Some(output), Some(cache_path)) = (&mut self.output, &self.args.voice_cache_file) {
            if matches!(self.operation, Operation::Transcribe) {
                output
                    .protect(cache_path)
                    .map_err(|_| DispatchError::Unavailable)?;
            }
        }
        self.bound = Some(runtime.clone());
        Ok(self)
    }

    pub fn finish(
        mut self,
        response: Response,
        runtime: &RuntimeContext,
        context: &CallContext,
        before_commit: impl Fn() -> Result<(), DispatchError>,
    ) -> Result<Response, DispatchError> {
        context.check()?;
        let bound = self.bound.as_ref().ok_or(DispatchError::Unavailable)?;
        if !same_runtime(bound, runtime) {
            return Err(DispatchError::Unavailable);
        }
        if !response.ok
            || response.error.is_some()
            || response
                .data
                .get("exit_code")
                .is_some_and(|value| value.as_i64() != Some(0))
            || response
                .data
                .get("error")
                .is_some_and(|value| !value.is_null())
        {
            return Err(DispatchError::QueryFailed);
        }
        let value = response
            .data
            .get("prepared_audio")
            .ok_or(DispatchError::InvalidResponse)?;
        let mut payload = BoundedBytes(Vec::new());
        serde_json::to_writer(&mut payload, value).map_err(|_| DispatchError::InvalidResponse)?;
        let voice = prepared_audio::decode(
            &payload.0,
            prepared_audio::Limits {
                max_audio_bytes: asr::database_media::MAX_VOICE_BYTES,
                max_response_bytes: MAX_PREPARED_VOICE_RESPONSE_BYTES,
            },
        )
        .map_err(|_| DispatchError::InvalidResponse)?;
        if voice.evidence.media_local_id != self.media_id
            || self
                .expected_username
                .as_ref()
                .is_some_and(|expected| expected != &voice.evidence.username)
        {
            return Err(DispatchError::InvalidResponse);
        }
        self.verify()?;
        context.check()?;
        before_commit()?;
        match self.operation {
            Operation::Decode => {
                let wav =
                    asr::prepare_wav_bytes(&voice.silk).map_err(|_| DispatchError::QueryFailed)?;
                let guard = self.output.as_ref().ok_or(DispatchError::Unavailable)?;
                let mut ready = None;
                let mut failure = None;
                let published = publish_wav_noclobber(&wav, guard, |metadata| {
                    let text = format!(
                        "解码成功!\n  文件: {}\n  时长: {:.1}秒\n  大小: {} bytes",
                        metadata.path.display(),
                        metadata.pcm_bytes as f64 / (metadata.sample_rate as f64 * 2.0),
                        grouped(metadata.size)
                    );
                    let check = context
                        .check_text_result(&text)
                        .and_then(|_| context.check())
                        .and_then(|_| before_commit());
                    if let Err(error) = check {
                        failure = Some(error);
                        anyhow::bail!("voice publication rejected");
                    }
                    ready = Some(Response::ok(json!({"mcp_text": text})));
                    Ok(())
                });
                // 发布之后只返回提交前构造的响应，不再进行可失败的文件访问。
                match published {
                    Ok(_) => Ok(ready.expect("publisher invokes callback before commit")),
                    Err(_) => Err(failure.unwrap_or(DispatchError::QueryFailed)),
                }
            }
            Operation::Transcribe => {
                let mut backend = match self.prepared_backend.take() {
                    Some(backend) => backend,
                    None => self
                        .args
                        .backend
                        .clone()
                        .build()
                        .map_err(|_| DispatchError::Unavailable)?,
                };
                self.verify()?;
                context.check()?;
                before_commit()?;
                // IPC、凭证读取和提交前复核共用调用预算，不能重新获得完整后端超时。
                context.check()?;
                limit_backend(&mut backend, context)?;
                let mut cache_rejection = None;
                let result = if let Some(path) = &self.args.voice_cache_file {
                    cached::transcribe_cached_with_receipt_checked(
                        &cached::CachedRequest {
                            cache_path: path,
                            account: &bound.id,
                            username: &voice.evidence.username,
                            source: &voice.evidence.message_source,
                            local_id: voice.evidence.message_local_id,
                            create_time: voice.evidence.create_time,
                            silk: &voice.silk,
                        },
                        &voice.evidence,
                        &backend,
                        |transcription| {
                            let check =
                                transcription_text(voice.evidence.create_time, transcription)
                                    .and_then(|text| context.check_text_result(&text))
                                    .and_then(|_| self.verify())
                                    .and_then(|_| context.check())
                                    .and_then(|_| before_commit());
                            if let Err(error) = check {
                                cache_rejection = Some(error);
                                anyhow::bail!("voice cache publication rejected");
                            }
                            Ok(())
                        },
                    )
                    .map(|outcome| outcome.cached.transcription)
                } else {
                    asr::transcribe_audio_bytes(&voice.silk, &backend)
                };
                // 区分宿主主动拒绝与可降级的缓存写入失败。
                if let Some(error) = cache_rejection {
                    return Err(error);
                }
                let result = result
                    .map_err(|_| context.check().err().unwrap_or(DispatchError::QueryFailed))?;
                let text = transcription_text(voice.evidence.create_time, &result)?;
                self.verify()?;
                context.check_text_result(&text)?;
                before_commit()?;
                Ok(Response::ok(json!({"mcp_text": text})))
            }
        }
    }

    fn verify(&self) -> Result<(), DispatchError> {
        for guard in self.output.iter().chain(self.cache.iter()) {
            guard.verify().map_err(|_| DispatchError::Unavailable)?;
        }
        if let (Some(guard), Some(path)) = (&self.cache, &self.args.voice_cache_file) {
            guard
                .verify_replaceable_file(path)
                .map_err(|_| DispatchError::Unavailable)?;
        }
        Ok(())
    }
}

fn configured_local_backend(
    runtime: &RuntimeContext,
    args: &BackendArgs,
) -> Result<asr::Backend, DispatchError> {
    let build = || -> anyhow::Result<asr::Backend> {
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        std::fs::File::open(&runtime.config_path)?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        anyhow::ensure!(bytes.len() <= 1024 * 1024, "configuration exceeds limit");
        let config: serde_json::Value = serde_json::from_slice(&bytes)?;
        let model = configured_local_model(&config)?;
        let local = asr::local_python::LocalPythonConfig::discover(
            model,
            (args.language != "auto").then(|| args.language.clone()),
            args.threads,
            std::time::Duration::from_secs(args.timeout_seconds),
            args.temp_root.clone().ok_or_else(|| anyhow::anyhow!("private work directory missing"))?,
            runtime.config_path.parent().ok_or_else(|| anyhow::anyhow!("configuration parent missing"))?.to_owned(),
        )?;
        Ok(asr::Backend::LegacyPythonLocal(local))
    };
    build().map_err(|_| DispatchError::Unavailable)
}

fn configured_local_model(config: &serde_json::Value) -> anyhow::Result<String> {
    anyhow::ensure!(config.get("transcription_backend").and_then(serde_json::Value::as_str) == Some("local"),
        "configured local Python requires an explicit local backend");
    match config.get("local_whisper_model") {
        None => Ok("base".into()),
        Some(value) => {
            let model = value.as_str().filter(|model| !model.trim().is_empty())
                .ok_or_else(|| anyhow::anyhow!("invalid configured local model"))?;
            Ok(model.to_owned())
        }
    }
}

fn limit_backend(backend: &mut asr::Backend, context: &CallContext) -> Result<(), DispatchError> {
    context.check()?;
    let remaining = context.remaining();
    match backend {
        asr::Backend::LegacyPythonLocal(config) => {
            config.tighten_timeout(remaining).map_err(|_| DispatchError::TimedOut)?;
        }
        asr::Backend::Local(config) => {
            config.timeout = config.timeout.min(remaining);
            if config.timeout.is_zero() { return Err(DispatchError::TimedOut); }
        }
        asr::Backend::ExplicitOpenAi { client, .. } => {
            client.tighten_timeout(remaining).map_err(|_| DispatchError::TimedOut)?;
        }
    }
    Ok(())
}

fn transcription_text(
    create_time: i64,
    result: &asr::Transcription,
) -> Result<String, DispatchError> {
    let timestamp = Local
        .timestamp_opt(create_time, 0)
        .single()
        .ok_or(DispatchError::InvalidResponse)?;
    Ok(format!(
        "[{}] ({})\n{}",
        timestamp.format("%Y-%m-%d %H:%M"),
        result.language,
        result.text
    ))
}

fn host_path(path: &Path) -> Result<PathBuf, DispatchError> {
    let raw = path.to_str().ok_or(DispatchError::Unavailable)?;
    if raw.is_empty() || raw.split(['/', '\\']).any(|part| part == "..") {
        return Err(DispatchError::Unavailable);
    }
    std::path::absolute(path).map_err(|_| DispatchError::Unavailable)
}

fn protect_runtime(guard: &mut HostOutputGuard, runtime: &RuntimeContext) -> anyhow::Result<()> {
    for path in [
        &runtime.config.db_dir,
        &runtime.config.decrypted_dir,
        &runtime.directory,
        &runtime.cache_dir(),
    ] {
        guard.protect_future(path)?;
    }
    for path in [&runtime.config_path, &runtime.config.keys_file] {
        guard.protect(path)?;
    }
    Ok(())
}

fn same_runtime(a: &RuntimeContext, b: &RuntimeContext) -> bool {
    a.id == b.id
        && a.root == b.root
        && a.directory == b.directory
        && a.config_path == b.config_path
        && a.config.db_dir == b.config.db_dir
        && a.config.keys_file == b.config.keys_file
        && a.config.decrypted_dir == b.config.decrypted_dir
        && a.config.wechat_process == b.config.wechat_process
}

fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

struct BoundedBytes(Vec<u8>);
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_PREPARED_VOICE_RESPONSE_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("prepared voice limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod configured_local_tests {
    use super::*;
    use std::{fs, time::Duration};

    const PRIVATE: &str = "SYNTHETIC_PRIVATE_LOCAL_MODEL";

    fn runtime(root: &Path, config: serde_json::Value) -> RuntimeContext {
        let config_path = root.join("config.json");
        fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let keys_file = root.join("keys.json");
        fs::write(&keys_file, b"{}").unwrap();
        RuntimeContext {
            config: crate::config::Config {
                db_dir: root.join("db"), keys_file,
                decrypted_dir: root.join("decrypted"), wechat_process: String::new(),
            },
            config_path, root: root.join("runtime-root"), id: "synthetic-account".into(),
            directory: root.join("runtime-root/account"),
        }
    }

    #[test]
    fn configured_local_requires_host_opt_in_and_rejects_mixed_parameters() {
        let context = CallContext::default();
        assert!(matches!(Args::default().prepare(Operation::Transcribe, 1, None, &context), Err(DispatchError::Unavailable)));
        let candidates = [
            BackendArgs { allow_upload: true, ..Default::default() },
            BackendArgs { backend: BackendKind::ExplicitOpenAi, ..Default::default() },
            BackendArgs { api_key_file: Some("missing-private-key".into()), ..Default::default() },
            BackendArgs { openai_base_url: Some("https://example.invalid".into()), ..Default::default() },
            BackendArgs { openai_model: Some(PRIVATE.into()), ..Default::default() },
            BackendArgs { whisper_binary: Some("missing-python".into()), ..Default::default() },
            BackendArgs { whisper_model: Some(PRIVATE.into()), ..Default::default() },
            BackendArgs { temp_root: Some("caller-selected-work".into()), ..Default::default() },
            BackendArgs { threads: Some(0), ..Default::default() },
            BackendArgs { timeout_seconds: 0, ..Default::default() },
        ];
        for backend in candidates {
            let args = Args { backend, ..Default::default() };
            assert!(matches!(args.prepare_configured_local(1, &context), Err(DispatchError::Unavailable)));
        }
    }

    #[test]
    fn configured_local_model_requires_local_without_cloud_fallback() {
        for config in [json!({}), json!(null), json!({"transcription_backend":"openai", "openai_api_key":PRIVATE}),
            json!({"transcription_backend":"whisper_cpp"}), json!({"transcription_backend":true}),
            json!({"transcription_backend":PRIVATE})] {
            let error = configured_local_model(&config).unwrap_err();
            assert!(!format!("{error:#} {error:?}").contains(PRIVATE));
        }
        assert_eq!(configured_local_model(&json!({"transcription_backend":"local"})).unwrap(), "base");
        assert_eq!(configured_local_model(&json!({"transcription_backend":"local", "local_whisper_model":"small"})).unwrap(), "small");
        for model in [json!(null), json!(false), json!(""), json!(" \t"), json!({"secret":PRIVATE})] {
            assert!(configured_local_model(&json!({"transcription_backend":"local", "local_whisper_model":model})).is_err());
        }
    }

    #[test]
    fn configured_local_configuration_errors_are_redacted_and_bounded() {
        let root = tempfile::tempdir().unwrap();
        let rt = runtime(root.path(), json!({"transcription_backend":"openai", "openai_api_key":PRIVATE}));
        let args = BackendArgs { temp_root: Some(root.path().join("private-work")), ..Default::default() };
        assert_eq!(configured_local_backend(&rt, &args).unwrap_err(), DispatchError::Unavailable);
        for bytes in [format!("{{invalid-{PRIVATE}"), PRIVATE.repeat(40_000),
            serde_json::to_string(&json!({"transcription_backend":"local", "local_whisper_model":format!("{PRIVATE}\n")})).unwrap()] {
            fs::write(&rt.config_path, bytes).unwrap();
            let error = configured_local_backend(&rt, &args).unwrap_err();
            assert_eq!(error, DispatchError::Unavailable);
            assert!(!format!("{error:?}").contains(PRIVATE));
        }
        assert!(!args.temp_root.unwrap().exists());
    }

    #[cfg(windows)]
    #[test]
    fn configured_local_bind_builds_real_backend_in_private_work_without_inference() {
        for cache_enabled in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let model = root.path().join(format!("{PRIVATE}.pt"));
        fs::write(&model, b"synthetic model, never loaded").unwrap();
        let rt = runtime(root.path(), json!({"transcription_backend":"local", "local_whisper_model":model}));
        let context = CallContext::default();
        let cache_dir = root.path().join("host-cache");
        fs::create_dir(&cache_dir).unwrap();
        let args = Args {
            voice_cache_file: cache_enabled.then(|| cache_dir.join("voices.json")),
            ..Default::default()
        };
        let pending = args.prepare_configured_local(1, &context).unwrap();
        let work = pending.args.backend.temp_root.clone().unwrap();
        assert!(pending.prepared_backend.is_none());
        let mut pending = pending.bind(&rt).unwrap();
        assert!(matches!(pending.prepared_backend, Some(asr::Backend::LegacyPythonLocal(_))));
        assert!(!format!("{:?}", pending.prepared_backend).contains(PRIVATE));
        limit_backend(pending.prepared_backend.as_mut().unwrap(), &context).unwrap();
        assert_eq!(fs::read_dir(&work).unwrap().count(), 0);
        pending.verify().unwrap();
        drop(pending);
        assert!(!work.exists());
        assert_eq!(fs::read_dir(cache_dir).unwrap().count(), 0);
        assert_eq!(fs::read(model).unwrap(), b"synthetic model, never loaded");
        }
    }

    #[cfg(windows)]
    #[test]
    fn configured_local_account_switch_is_rejected_before_inference() {
        let root = tempfile::tempdir().unwrap();
        let rt = runtime(root.path(), json!({"transcription_backend":"local"}));
        let context = CallContext::default();
        let pending = Args::default().prepare_configured_local(1, &context).unwrap().bind(&rt).unwrap();
        let work = pending.args.backend.temp_root.clone().unwrap();
        let mut changed = rt.clone();
        changed.id = "other-account".into();
        assert!(matches!(pending.finish(Response::ok(json!({})), &changed, &context, || Ok(())), Err(DispatchError::Unavailable)));
        assert!(!work.exists());
    }

    #[test]
    fn configured_local_expired_call_is_rejected_before_preparation() {
        let context = CallContext::new(Default::default(), Duration::ZERO);
        assert!(matches!(Args::default().prepare_configured_local(1, &context), Err(DispatchError::TimedOut)));
    }
}
