//! 显式静态快照中的单条语音转录；不认证账号来源、不猜分片、不创建 SILK 中转文件。
use crate::toolkit::asr::{cached, database_media, transcribe_audio_bytes};
use anyhow::{ensure, Result};
use clap::Args;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

#[derive(Args, serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct TranscribeDatabaseNativeArgs {
    /// 显式单账号静态已解密根目录；来源由调用方保证，并非已认证账号
    #[arg(long)]
    pub decrypted_dir: PathBuf,
    /// 精确 username，不按昵称或备注推断
    #[arg(long)]
    pub username: String,
    /// 完整消息分片来源，例如 message/message_0.db
    #[arg(long)]
    pub source: String,
    /// 此消息分片中该联系人消息表的 local_id，不是媒体库 local_id
    #[arg(long, value_parser = clap::value_parser!(i64).range(1..))]
    pub local_id: i64,
    /// 可选成功转录缓存；必须位于可信、稳定且独立于数据库的目录
    #[arg(long, requires = "cache_account")]
    pub cache_file: Option<PathBuf>,
    /// 调用方显式提供的缓存账号命名空间；不是账号认证
    #[arg(long, requires = "cache_file")]
    pub cache_account: Option<String>,
    #[command(flatten)]
    pub backend: super::asr::BackendArgs,
}

pub fn cmd_transcribe_database_native(args: TranscribeDatabaseNativeArgs) -> Result<()> {
    let output = transcribe(args)?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn transcribe(args: TranscribeDatabaseNativeArgs) -> Result<Value> {
    let protected: Vec<_> = [
        args.backend.whisper_binary.clone(),
        args.backend.whisper_model.clone(),
        args.backend.api_key_file.clone(),
    ]
    .into_iter()
    .flatten()
    .collect();
    // 后端构建先检查显式上传授权，授权失败时不读取数据库或音频。
    let backend = args.backend.build()?;
    if let Some(path) = &args.cache_file {
        ensure!(
            args.cache_account
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty()),
            "--cache-account must not be empty"
        );
        validate_cache_path(path, &args.decrypted_dir, &protected)?;
    }
    let voice = database_media::resolve_voice(
        &args.decrypted_dir,
        database_media::MessageIdentity {
            username: &args.username,
            source: &args.source,
            local_id: args.local_id,
        },
    )?;
    let (result, cache_report) = if let Some(path) = &args.cache_file {
        let outcome = cached::transcribe_cached(
            &cached::CachedRequest {
                cache_path: path,
                account: args
                    .cache_account
                    .as_deref()
                    .expect("validated cache account"),
                username: &voice.evidence.username,
                source: &voice.evidence.message_source,
                local_id: voice.evidence.message_local_id,
                create_time: voice.evidence.create_time,
                silk: &voice.silk,
            },
            &backend,
        )?;
        (
            outcome.transcription,
            Some(json!({
                "state":outcome.cache_state, "create_time":outcome.create_time
            })),
        )
    } else {
        (transcribe_audio_bytes(&voice.silk, &backend)?, None)
    };
    let evidence = voice.evidence;
    // 显式挑选来源字段，绝不序列化原始音频或凭证，也不声称快照账号已认证。
    let mut output = json!({
        "transcription": result,
        "evidence": {
            "account_authenticated": false,
            "account_provenance": "caller_supplied_decrypted_snapshot",
            "username": evidence.username,
            "message_source": evidence.message_source,
            "message_table": evidence.message_table,
            "message_local_id": evidence.message_local_id,
            "server_id": evidence.server_id,
            "create_time": evidence.create_time,
            "media_source": evidence.media_source,
            "media_rowid": evidence.media_rowid,
            "media_chat_name_id": evidence.media_chat_name_id,
            "media_local_id": evidence.media_local_id,
        }
    });
    if let Some(cache) = cache_report {
        output["cache"] = cache;
    }
    Ok(output)
}

fn validate_cache_path(path: &Path, database_root: &Path, protected: &[PathBuf]) -> Result<()> {
    ensure!(
        path.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json")),
        "cache output must be a .json file"
    );
    for part in path.components() {
        ensure!(
            part != Component::ParentDir,
            "cache path must not contain parent traversal"
        );
        if let Component::Normal(name) = part {
            let name = name
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("invalid cache path"))?;
            ensure!(
                !name.contains([':', '\0']) && !name.ends_with(['.', ' ']),
                "unsafe cache path component"
            );
        }
    }
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    ensure!(
        !regex::Regex::new(r"(?i)^(con|prn|aux|nul|com[1-9¹²³]|lpt[1-9¹²³])(?:\.|$)")?
            .is_match(name),
        "reserved cache filename"
    );
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    ensure!(
        parent.is_dir(),
        "cache parent must be an existing trusted directory"
    );
    crate::toolkit::separate(database_root, path)?;
    for source in protected {
        crate::toolkit::separate(source, path)?;
        if source.exists() && path.exists() {
            ensure!(
                !same_file::is_same_file(source, path)?,
                "cache aliases a protected input"
            );
        }
    }
    if path.try_exists()? {
        // 目录外的硬链接仍可能是源数据库本身，必须按文件身份逐一核对。
        for source in database_media::source_files(database_root)? {
            ensure!(
                !same_file::is_same_file(&source, path)?,
                "cache aliases a database input"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};
    use rusqlite::Connection;
    use std::{fs, path::Path};

    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        args: TranscribeDatabaseNativeArgs,
    }

    fn args(root: &Path, source: &str, engine: &Path) -> TranscribeDatabaseNativeArgs {
        let input = vec![
            "wx".to_owned(),
            "--decrypted-dir".into(),
            root.to_string_lossy().into_owned(),
            "--username".into(),
            "alice".into(),
            "--source".into(),
            source.into(),
            "--local-id".into(),
            "7".into(),
            "--whisper-binary".into(),
            engine.join("fake.exe").to_string_lossy().into_owned(),
            "--whisper-model".into(),
            engine.join("model.bin").to_string_lossy().into_owned(),
            "--temp-root".into(),
            engine.to_string_lossy().into_owned(),
            "--language".into(),
            "zh".into(),
            "--threads".into(),
            "2".into(),
        ];
        TestCli::try_parse_from(input).unwrap().args
    }

    fn databases(root: &Path) {
        fs::create_dir(root.join("message")).unwrap();
        let table = format!("Msg_{:x}", md5::compute("alice"));
        for (shard, server) in [(0, 100), (1, 200)] {
            let conn = Connection::open(root.join(format!("message/message_{shard}.db"))).unwrap();
            conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,server_id INTEGER,create_time INTEGER); INSERT INTO [{table}] VALUES(7,34,{server},123)")).unwrap();
        }
        let media = Connection::open(root.join("message/media_8.db")).unwrap();
        media.execute_batch("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id(rowid,user_name) VALUES(9,'alice'); CREATE TABLE VoiceInfo(chat_name_id INTEGER,local_id INTEGER,svr_id INTEGER,create_time INTEGER,voice_data BLOB)").unwrap();
        for server in [100, 200] {
            media
                .execute(
                    "INSERT INTO VoiceInfo VALUES(9,700,?1,123,?2)",
                    rusqlite::params![
                        server,
                        include_bytes!("../../../tests/fixtures/audio/silence.silk").as_slice()
                    ],
                )
                .unwrap();
        }
    }

    fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
        let mut data: Vec<_> = fs::read_dir(root.join("message"))
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect();
        data.sort();
        data
    }

    #[test]
    fn args_and_handler_contract_require_full_explicit_identity() {
        TestCli::command().debug_assert();
        let _: fn(TranscribeDatabaseNativeArgs) -> Result<()> = cmd_transcribe_database_native;
        assert!(TestCli::try_parse_from(["wx"]).is_err());
        assert!(TestCli::try_parse_from([
            "wx",
            "--decrypted-dir",
            "root",
            "--username",
            "alice",
            "--source",
            "message/message_0.db",
            "--local-id",
            "0"
        ])
        .is_err());
    }

    #[test]
    fn cloud_authorization_precedes_credentials_and_database_access() {
        let parsed = TestCli::try_parse_from([
            "wx",
            "--decrypted-dir",
            "missing-root",
            "--username",
            "alice",
            "--source",
            "message/message_0.db",
            "--local-id",
            "7",
            "--backend",
            "explicit-open-ai",
            "--api-key-file",
            "missing.key",
            "--cache-file",
            "missing-parent/cache.json",
            "--cache-account",
            "synthetic",
        ])
        .unwrap();
        assert!(transcribe(parsed.args)
            .unwrap_err()
            .to_string()
            .contains("--allow-upload"));
    }

    #[test]
    fn ambiguous_database_stops_before_starting_backend() {
        let root = tempfile::tempdir().unwrap();
        databases(root.path());
        let media = root.path().join("message/media_8.db");
        Connection::open(&media)
            .unwrap()
            .execute_batch("INSERT INTO VoiceInfo SELECT * FROM VoiceInfo WHERE svr_id=100")
            .unwrap();
        let before = snapshot(root.path());
        let result = transcribe(args(root.path(), "message/message_0.db", root.path()));
        let error = result.unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<database_media::DatabaseMediaError>()
                .unwrap()
                .kind,
            database_media::ErrorKind::AmbiguousMedia
        );
        assert_eq!(before, snapshot(root.path()));
    }

    #[test]
    fn unsupported_silk_is_not_staged_or_sent_to_model() {
        let root = tempfile::tempdir().unwrap();
        databases(root.path());
        Connection::open(root.path().join("message/media_8.db"))
            .unwrap()
            .execute(
                "UPDATE VoiceInfo SET voice_data=?1",
                [b"#!SILK_V3invalid".as_slice()],
            )
            .unwrap();
        let before = snapshot(root.path());
        assert!(transcribe(args(root.path(), "message/message_0.db", root.path())).is_err());
        assert_eq!(before, snapshot(root.path()));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    fn compile_engine(engine: &Path) {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/asr-local/fake.rs");
        let mut command = std::process::Command::new("rustc");
        command
            .arg("--edition=2021")
            .arg(source)
            .arg("-o")
            .arg(engine.join("fake.exe"));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        eprintln!("synthetic compiler command: {command:?}");
        let output = command.output().unwrap();
        eprint!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success());
        fs::write(engine.join("model.bin"), "ok").unwrap();
    }

    #[test]
    fn synthetic_database_to_local_bytes_pipeline_has_evidence_without_audio() {
        let root = tempfile::tempdir().unwrap();
        let engine = tempfile::tempdir().unwrap();
        databases(root.path());
        compile_engine(engine.path());
        let before = snapshot(root.path());
        let engine_files = fs::read_dir(engine.path()).unwrap().count();
        for (source, server) in [("message/message_0.db", 100), ("message/message_1.db", 200)] {
            let value = transcribe(args(root.path(), source, engine.path())).unwrap();
            assert_eq!(value["transcription"]["text"], "hello world");
            assert_eq!(value["transcription"]["backend"], "whisper_cpp");
            assert_eq!(value["evidence"]["message_source"], source);
            assert_eq!(value["evidence"]["server_id"], server);
            assert_eq!(value["evidence"]["media_local_id"], 700);
            assert_eq!(value["evidence"]["account_authenticated"], false);
            assert_eq!(value.as_object().unwrap().len(), 2);
            for forbidden in ["silk", "voice_data", "audio", "api_key", "decrypted_dir"] {
                assert!(!value["evidence"]
                    .as_object()
                    .unwrap()
                    .contains_key(forbidden));
                assert!(!value.as_object().unwrap().contains_key(forbidden));
            }
        }
        assert_eq!(before, snapshot(root.path()));
        assert_eq!(engine_files, fs::read_dir(engine.path()).unwrap().count());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn cache_flags_are_paired_and_preflight_protects_sources() {
        let base = [
            "wx",
            "--decrypted-dir",
            "root",
            "--username",
            "alice",
            "--source",
            "message/message_0.db",
            "--local-id",
            "7",
        ];
        assert!(
            TestCli::try_parse_from(base.into_iter().chain(["--cache-file", "cache.json"]))
                .is_err()
        );
        assert!(
            TestCli::try_parse_from(base.into_iter().chain(["--cache-account", "synthetic"]))
                .is_err()
        );
        assert!(TestCli::try_parse_from(base.into_iter().chain([
            "--cache-file",
            "cache.json",
            "--cache-account",
            "synthetic"
        ]))
        .is_ok());
        let root = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        for path in [
            root.path().join("cache.json"),
            output.path().join("CON.json"),
            output.path().join("stream:cache.json"),
            output.path().join("../cache.json"),
            output.path().join("missing/cache.json"),
        ] {
            assert!(
                validate_cache_path(&path, root.path(), &[]).is_err(),
                "{}",
                path.display()
            );
        }
        let model = output.path().join("model.json");
        let alias = output.path().join("cache.json");
        fs::write(&model, b"protected input").unwrap();
        fs::hard_link(&model, &alias).unwrap();
        assert!(validate_cache_path(&alias, root.path(), std::slice::from_ref(&model)).is_err());
        assert_eq!(fs::read(&model).unwrap(), b"protected input");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn external_database_hardlinks_are_rejected_before_sqlite_access() {
        for name in [
            "message/message_0.db",
            "message/MESSAGE_17.DB",
            "message/media_8.db",
            "message/MEDIA_19.DB",
            "contact/contact.db",
        ] {
            let root = tempfile::tempdir().unwrap();
            let output = tempfile::tempdir().unwrap();
            fs::create_dir(root.path().join("message")).unwrap();
            fs::create_dir(root.path().join("contact")).unwrap();
            let source = root.path().join(name);
            fs::write(&source, b"not SQLite: source sentinel").unwrap();
            let cache = output.path().join("cache.json");
            fs::hard_link(&source, &cache).unwrap();
            let mut request = args(root.path(), "message/message_0.db", output.path());
            request.cache_file = Some(cache.clone());
            request.cache_account = Some("synthetic".into());
            let error = transcribe(request).unwrap_err();
            assert_eq!(
                error.to_string(),
                "cache aliases a database input",
                "{name}: {error:#}"
            );
            assert_eq!(fs::read(&source).unwrap(), b"not SQLite: source sentinel");
            assert_eq!(fs::read(&cache).unwrap(), b"not SQLite: source sentinel");
            assert_eq!(fs::read_dir(output.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn source_inventory_is_nonrecursive_and_rejects_source_aliases_and_directories() {
        let root = tempfile::tempdir().unwrap();
        let message = root.path().join("message");
        fs::create_dir(&message).unwrap();
        let source = message.join("message_0.db");
        fs::write(&source, b"sentinel").unwrap();
        fs::create_dir(message.join("unrelated")).unwrap();
        fs::write(message.join("unrelated/media_0.db"), b"not a source").unwrap();
        assert_eq!(database_media::source_files(root.path()).unwrap().len(), 1);
        let alias = message.join("media_0.db");
        fs::hard_link(&source, &alias).unwrap();
        assert_eq!(
            database_media::source_files(root.path()).unwrap_err().kind,
            database_media::ErrorKind::UnsafePath
        );
        fs::remove_file(&alias).unwrap();
        fs::create_dir(&alias).unwrap();
        assert_eq!(
            database_media::source_files(root.path()).unwrap_err().kind,
            database_media::ErrorKind::UnsafePath
        );
        assert_eq!(fs::read(source).unwrap(), b"sentinel");
    }

    #[test]
    fn source_inventory_bounds_even_unrelated_directory_entries() {
        let root = tempfile::tempdir().unwrap();
        let message = root.path().join("message");
        fs::create_dir(&message).unwrap();
        for index in 0..4097 {
            fs::create_dir(message.join(format!("unrelated-{index}"))).unwrap();
        }
        assert_eq!(
            database_media::source_files(root.path()).unwrap_err().kind,
            database_media::ErrorKind::LimitExceeded
        );
    }

    #[cfg(windows)]
    #[test]
    fn source_inventory_rejects_directory_junctions_without_reading_target() {
        use std::os::windows::process::CommandExt;
        for name in ["message", "contact"] {
            let root = tempfile::tempdir().unwrap();
            let target = tempfile::tempdir().unwrap();
            fs::write(target.path().join("sentinel"), b"untouched").unwrap();
            if name == "contact" {
                fs::create_dir(root.path().join("message")).unwrap();
            }
            let link = root.path().join(name);
            let mut command = std::process::Command::new("cmd");
            command
                .args(["/c", "mklink", "/J"])
                .arg(&link)
                .arg(target.path())
                .creation_flags(0x0800_0000);
            eprintln!("synthetic junction command: {command:?}");
            let output = command.output().unwrap();
            eprint!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.status.success());
            let result = database_media::source_files(root.path());
            fs::remove_dir(&link).unwrap();
            assert_eq!(
                result.unwrap_err().kind,
                database_media::ErrorKind::UnsafePath
            );
            assert_eq!(
                fs::read(target.path().join("sentinel")).unwrap(),
                b"untouched"
            );
            assert_eq!(fs::read_dir(target.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn database_cache_hits_and_preserves_foreign_account_and_sources() {
        let root = tempfile::tempdir().unwrap();
        let engine = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        databases(root.path());
        compile_engine(engine.path());
        let before = snapshot(root.path());
        let cache = output.path().join("cache.json");
        let engine_files = || {
            let mut names: Vec<_> = fs::read_dir(engine.path())
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            names.sort();
            names
        };
        let files_before = engine_files();
        let run = |account: &str| {
            let mut request = args(root.path(), "message/message_0.db", engine.path());
            request.cache_file = Some(cache.clone());
            request.cache_account = Some(account.into());
            transcribe(request).unwrap()
        };
        let first = run("synthetic-account-a");
        assert_eq!(first["cache"]["state"], "stored");
        let hit = run("synthetic-account-a");
        assert_eq!(hit["cache"]["state"], "hit");
        assert_eq!(hit["transcription"], first["transcription"]);
        assert_eq!(hit["cache"]["create_time"], 123);
        fs::write(engine.path().join("model.bin"), "ok\n").unwrap();
        assert_eq!(run("synthetic-account-a")["cache"]["state"], "stored");
        let cache_before = fs::read(&cache).unwrap();
        let foreign = run("synthetic-account-b");
        assert_eq!(foreign["cache"]["state"], "read_unavailable");
        assert_eq!(foreign["transcription"]["text"], "hello world");
        assert_eq!(cache_before, fs::read(&cache).unwrap());
        assert_eq!(before, snapshot(root.path()));
        assert_eq!(engine_files(), files_before);
    }
}
