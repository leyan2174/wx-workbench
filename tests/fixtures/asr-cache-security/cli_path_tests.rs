use crate::cli::asr_database::{cmd_transcribe_database_native, TranscribeDatabaseNativeArgs};
use clap::Parser;
use std::{fs, path::Path};

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    args: crate::cli::operation_args::asr_database::TranscribeDatabaseNativeArgs,
}

fn args(
    root: &Path,
    cache: &Path,
    engine: &Path,
    key: Option<&Path>,
) -> TranscribeDatabaseNativeArgs {
    let mut values = vec![
        "fixture".into(),
        "--decrypted-dir".into(),
        root.to_string_lossy().into_owned(),
        "--username".into(),
        "peer".into(),
        "--source".into(),
        "message/message_0.db".into(),
        "--local-id".into(),
        "1".into(),
        "--cache-file".into(),
        cache.to_string_lossy().into_owned(),
        "--cache-account".into(),
        "synthetic".into(),
    ];
    if let Some(key) = key {
        values.extend([
            "--backend".into(),
            "openai_compatible".into(),
            "--allow-upload".into(),
            "--openai-base-url".into(),
            "http://127.0.0.1:9/v1".into(),
            "--openai-model".into(),
            "synthetic".into(),
            "--api-key-file".into(),
            key.to_string_lossy().into_owned(),
        ]);
    } else {
        values.extend([
            "--whisper-binary".into(),
            engine.join("program.json").to_string_lossy().into_owned(),
            "--whisper-model".into(),
            engine.join("model.json").to_string_lossy().into_owned(),
        ]);
    }
    Cli::try_parse_from(values).unwrap().args.into()
}

#[test]
fn cli_rejects_source_directory_and_protected_file_aliases_before_database_access() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("db");
    fs::create_dir(&root).unwrap();
    for name in ["program.json", "model.json", "key.json"] {
        fs::write(dir.path().join(name), b"SYNTHETIC_CREDENTIAL").unwrap();
    }
    let inside = root.join("cache.json");
    let error = cmd_transcribe_database_native(args(&root, &inside, dir.path(), None)).unwrap_err();
    assert!(
        error.to_string().contains("outside the source directory"),
        "{error:#}"
    );
    assert!(!inside.exists());
    for name in ["program.json", "model.json", "key.json"] {
        let source = dir.path().join(name);
        let alias = dir.path().join(format!("alias-{name}"));
        fs::hard_link(&source, &alias).unwrap();
        let key = (name == "key.json").then_some(source.as_path());
        let error =
            cmd_transcribe_database_native(args(&root, &alias, dir.path(), key)).unwrap_err();
        println!("PROTECTED ALIAS {name}: {error:#}");
        assert!(
            error
                .to_string()
                .contains("cache aliases a protected input"),
            "{error:#}"
        );
        assert!(!format!("{error:#}").contains("SYNTHETIC_CREDENTIAL"));
        assert_eq!(fs::read(&source).unwrap(), b"SYNTHETIC_CREDENTIAL");
        assert_eq!(fs::read(&alias).unwrap(), b"SYNTHETIC_CREDENTIAL");
    }
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
}

#[test]
fn cli_external_database_hardlink_must_be_rejected_at_path_preflight() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("db");
    fs::create_dir_all(root.join("message")).unwrap();
    let source = root.join("message/message_0.db");
    fs::write(&source, b"synthetic database sentinel").unwrap();
    let alias = dir.path().join("cache.json");
    fs::hard_link(&source, &alias).unwrap();
    assert!(same_file::is_same_file(&source, &alias).unwrap());
    let error = cmd_transcribe_database_native(args(&root, &alias, dir.path(), None)).unwrap_err();
    println!("DATABASE HARDLINK ERROR: {error:#}");
    assert_eq!(fs::read(&source).unwrap(), b"synthetic database sentinel");
    assert_eq!(fs::read(&alias).unwrap(), b"synthetic database sentinel");
    assert!(
        error.to_string().contains("cache aliases")
            || error.to_string().contains("outside the source directory"),
        "未在路径预检拒绝数据库硬链接，进入后续数据库解析: {error:#}"
    );
}
