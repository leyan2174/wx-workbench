//! 真实加密账号与 daemon；仅本地 whisper 子进程是可控测试后端。
#![cfg(windows)]
#[path = "fixtures/mcp-voice-runtime/accounts.rs"]
mod accounts;
#[allow(dead_code)]
#[path = "fixtures/mcp-voice-runtime/artifacts.rs"]
mod artifacts;
#[allow(dead_code)]
#[path = "fixtures/mcp-readonly-runtime/support.rs"]
mod support;
use serde_json::{json, Value};
use std::{fs, path::Path};
use support::{Account, Mcp};

fn safe_failure(reply: Value, expected: &str) {
    assert_eq!(
        reply["result"],
        json!({"isError":true,"content":[{"type":"text","text":expected}]})
    );
    assert!(reply.get("error").is_none());
}

fn args(id: i64) -> Value {
    json!({"chat_name":"peer","local_id":id})
}

fn text(reply: Value) -> String {
    assert!(reply.get("error").is_none(), "{reply}");
    let result = &reply["result"];
    assert_eq!(result["isError"], false, "{reply}");
    let value = result["content"][0]["text"].as_str().unwrap().to_owned();
    assert_eq!(
        result,
        &json!({"isError":false,"content":[{"type":"text","text":value}]})
    );
    assert!(
        serde_json::from_str::<Value>(&value).is_err(),
        "audio success must be legacy plain text"
    );
    value
}

fn local(
    account: &Account,
    root: &Path,
    model: &Path,
    cache: Option<&Path>,
    frame: Option<&str>,
) -> Mcp {
    let mut args = vec![
        "--media-output-root",
        root.to_str().unwrap(),
        "--backend",
        "local",
        "--whisper-binary",
        artifacts::executable().to_str().unwrap(),
        "--whisper-model",
        model.to_str().unwrap(),
        "--language",
        "zh",
        "--threads",
        "2",
        "--timeout-seconds",
        "5",
        "--temp-root",
        root.to_str().unwrap(),
    ];
    if let Some(cache) = cache {
        args.extend(["--voice-cache-file", cache.to_str().unwrap()]);
    }
    if let Some(frame) = frame {
        args.extend(["--max-frame-bytes", frame]);
    }
    let mut mcp = account.mcp_with_args(&args);
    mcp.ready();
    mcp
}

#[test]
fn real_daemon_prepares_legacy_media_ids_without_publishing_or_transcribing() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Account::new(home.path(), "A");
    let mut b = Account::new(home.path(), "B");
    accounts::seed(&a);
    accounts::seed(&b);
    let before_a = a.snapshot();
    let before_b = b.snapshot();
    a.start();
    b.start();
    for account in [&a, &b] {
        for command in ["decode_voice", "transcribe_voice"] {
            assert_eq!(
                account
                    .ipc(json!({"cmd":command,"chat":"peer","local_id":700}))
                    .unwrap(),
                json!({"ok":true,"prepared_audio":artifacts::prepared("peer",account.marker,700)})
            );
        }
    }
    assert_eq!(a.snapshot(), before_a);
    assert_eq!(b.snapshot(), before_b);
}

#[test]
fn real_voice_media_ids_decode_exact_wav_and_preserve_session_and_accounts() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Account::new(home.path(), "A");
    let mut b = Account::new(home.path(), "B");
    accounts::seed(&a);
    accounts::seed(&b);
    let before_a = a.snapshot();
    let before_b = b.snapshot();
    a.start();
    b.start();
    let output = tempfile::tempdir().unwrap();
    let mut ma = a.mcp_with_args(&["--media-output-root", output.path().to_str().unwrap()]);
    let mut mb = b.mcp_with_args(&["--media-output-root", output.path().to_str().unwrap()]);
    ma.ready();
    mb.ready();
    // 在同一个 MCP 上按媒体 ID 取音频，不能把消息 ID 7 当作媒体 ID 700。
    safe_failure(ma.call("decode_voice", args(7)), "Query failed");
    let first = artifacts::assert_decode(
        &text(ma.call("decode_voice", args(700))),
        "A",
        700,
        output.path(),
    );
    let saved = fs::read(&first).unwrap();
    let duplicate = ma.call("decode_voice", args(700));
    artifacts::assert_decode(
        &text(ma.call("decode_voice", args(701))),
        "A",
        701,
        output.path(),
    );
    safe_failure(duplicate, "Query failed");
    assert_eq!(fs::read(&first).unwrap(), saved);
    let second = artifacts::assert_decode(
        &text(mb.call("decode_voice", args(700))),
        "B",
        700,
        output.path(),
    );
    assert_ne!(first, second);
    assert_ne!(fs::read(&second).unwrap(), saved);
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 3);
    for name in ["decode_voice", "transcribe_voice"] {
        for invalid in [
            json!({"chat_name":"peer","local_id":0}),
            json!({"chat_name":"peer","local_id":-1}),
            json!({"chat_name":"peer","local_id":700,"create_time":artifacts::TIMESTAMP}),
            json!({"chat_name":"peer","local_id":700,"output_root":"forbidden"}),
            json!({"chat_name":"peer","local_id":700,"backend":"local"}),
        ] {
            assert_eq!(ma.call(name, invalid)["error"]["code"], -32602);
        }
    }
    ma.finish();
    mb.finish();
    assert_eq!(a.snapshot(), before_a);
    assert_eq!(b.snapshot(), before_b);
}

#[test]
fn real_transcription_uses_local_wav_backend_and_account_bound_cache() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Account::new(home.path(), "A");
    let mut b = Account::new(home.path(), "B");
    accounts::seed(&a);
    accounts::seed(&b);
    let before_a = a.snapshot();
    let before_b = b.snapshot();
    a.start();
    b.start();
    let output = tempfile::tempdir().unwrap();
    let backend = tempfile::tempdir().unwrap();
    let model_a = artifacts::model(backend.path(), "A", 700, "甲账号语音测试");
    let model_b = artifacts::model(backend.path(), "B", 700, "乙账号语音测试");
    let cache_root = tempfile::tempdir().unwrap();
    let cache = cache_root.path().join("voice-cache.json");
    let mut ma = local(&a, output.path(), &model_a, Some(&cache), None);
    artifacts::assert_transcribe(
        &text(ma.call("transcribe_voice", args(700))),
        "甲账号语音测试",
        700,
    );
    artifacts::assert_transcribe(
        &text(ma.call("transcribe_voice", args(700))),
        "甲账号语音测试",
        700,
    );
    assert_eq!(
        fs::read_to_string(model_a.with_extension("calls")).unwrap(),
        "wav-verified\n"
    );
    ma.finish();
    let mut mb = local(&b, output.path(), &model_b, Some(&cache), None);
    artifacts::assert_transcribe(
        &text(mb.call("transcribe_voice", args(700))),
        "乙账号语音测试",
        700,
    );
    mb.finish();
    assert_eq!(
        fs::read_to_string(model_b.with_extension("calls")).unwrap(),
        "wav-verified\n"
    );
    // 转录失败后，不重启当前 MCP，解码另一条有效语音。
    let fail = artifacts::model(backend.path(), "A", 700, "FAIL");
    let mut ma = local(&a, output.path(), &fail, None, None);
    safe_failure(ma.call("transcribe_voice", args(700)), "Query failed");
    artifacts::assert_decode(
        &text(ma.call("decode_voice", args(701))),
        "A",
        701,
        output.path(),
    );
    ma.finish();
    assert_eq!(
        fs::read_dir(output.path()).unwrap().count(),
        1,
        "transcription must not leave permanent WAV files"
    );
    assert_eq!(a.snapshot(), before_a);
    assert_eq!(b.snapshot(), before_b);
}

#[test]
fn persistent_cache_hit_survives_removed_sources_with_new_daemon_session() {
    let home = tempfile::tempdir().unwrap();
    let mut account = Account::new(home.path(), "A");
    accounts::seed(&account);
    account.start();
    let output = tempfile::tempdir().unwrap();
    let backend = tempfile::tempdir().unwrap();
    let model = artifacts::model(backend.path(), "A", 700, &"转".repeat(512));
    let cache_root = tempfile::tempdir().unwrap();
    let cache = cache_root.path().join("voice-cache.json");
    let mut mcp = local(&account, output.path(), &model, Some(&cache), None);
    let original = text(mcp.call("transcribe_voice", args(700)));
    assert_eq!(
        text(mcp.call(
            "transcribe_voice",
            json!({"chat_name":"姓名A","local_id":700})
        )),
        original
    );
    let before = fs::read(&cache).unwrap();
    assert!(serde_json::from_slice::<Value>(&before)
        .unwrap()
        .get("_wx_asr_receipts")
        .is_some());
    account.stop();
    // 仅移动本测试账号的合成源目录；原路径保持缺失，不能靠旧源重新转写。
    let source = account.root().join("db_storage/message");
    let removed = account.root().join("removed-message-sources");
    assert!(source
        .canonicalize()
        .unwrap()
        .starts_with(account.root().canonicalize().unwrap()));
    fs::rename(&source, &removed).unwrap();
    account.start();
    safe_failure(
        mcp.call("transcribe_voice", args(700)),
        "Query backend unavailable",
    );
    mcp.finish();
    let mut reopened = local(&account, output.path(), &model, Some(&cache), None);
    assert_eq!(text(reopened.call("transcribe_voice", args(700))), original);
    reopened.finish();
    let mut limited = local(&account, output.path(), &model, Some(&cache), Some("1024"));
    safe_failure(
        limited.call("transcribe_voice", args(700)),
        "Query result exceeds safe limit",
    );
    limited.finish();
    assert_eq!(fs::read(&cache).unwrap(), before);
    assert_eq!(
        fs::read_to_string(model.with_extension("calls")).unwrap(),
        "wav-verified\n"
    );
    assert!(!source.exists());
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
}

#[test]
fn voice_internal_preparation_budget_is_separate_from_final_text_frame_limit() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Account::new(home.path(), "A");
    accounts::seed(&a);
    let before = a.snapshot();
    a.start();
    let prepared = a
        .ipc(json!({"cmd":"decode_voice","chat":"peer","local_id":700}))
        .unwrap();
    assert!(serde_json::to_vec(&prepared).unwrap().len() > 1024);
    let output = tempfile::tempdir().unwrap();
    let backend = tempfile::tempdir().unwrap();
    let oversized = "语".repeat(1024);
    let model = artifacts::model(backend.path(), "A", 700, &oversized);
    let mut mcp = local(&a, output.path(), &model, None, Some("1024"));
    // 外部 1KiB 不能误用作 IPC 音频预算；内部准备结果不直接显示给调用者。
    artifacts::assert_decode(
        &text(mcp.call("decode_voice", args(700))),
        "A",
        700,
        output.path(),
    );
    safe_failure(
        mcp.call("transcribe_voice", args(700)),
        "Query result exceeds safe limit",
    );
    artifacts::assert_decode(
        &text(mcp.call("decode_voice", args(701))),
        "A",
        701,
        output.path(),
    );
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 2);
    mcp.finish();
    assert_eq!(a.snapshot(), before);
}

#[test]
fn same_audio_and_backend_still_require_separate_account_cache_entries() {
    let home = tempfile::tempdir().unwrap();
    let mut a = Account::new(home.path(), "A");
    // 内容故意完全相同，只有配置路径和账号身份不同。
    let mut b = Account::new(home.path(), "A");
    assert_ne!(a.root(), b.root());
    accounts::seed(&a);
    accounts::seed(&b);
    let before_a = a.snapshot();
    let before_b = b.snapshot();
    a.start();
    b.start();
    let output = tempfile::tempdir().unwrap();
    let backend = tempfile::tempdir().unwrap();
    let cache_root = tempfile::tempdir().unwrap();
    let cache = cache_root.path().join("shared-cache.json");
    let model = artifacts::model(backend.path(), "A", 700, "同内容账号隔离");
    for account in [&a, &b, &a] {
        let mut mcp = local(account, output.path(), &model, Some(&cache), None);
        artifacts::assert_transcribe(
            &text(mcp.call("transcribe_voice", args(700))),
            "同内容账号隔离",
            700,
        );
        mcp.finish();
    }
    assert_eq!(
        fs::read_to_string(model.with_extension("calls")).unwrap(),
        "wav-verified\nwav-verified\n"
    );
    assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
    assert_eq!(a.snapshot(), before_a);
    assert_eq!(b.snapshot(), before_b);
}

#[test]
fn voice_without_explicit_output_or_backend_does_not_start_account_or_read_credentials() {
    use std::os::windows::fs::OpenOptionsExt;
    let home = tempfile::tempdir().unwrap();
    let a = Account::new(home.path(), "A");
    accounts::seed(&a);
    let output = tempfile::tempdir().unwrap();
    let secrets = tempfile::tempdir().unwrap();
    let key = secrets.path().join("locked-key.txt");
    fs::write(&key, "DO-NOT-READ").unwrap();
    let _locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&key)
        .unwrap();
    fs::write(
        a.root().join("config.json"),
        "invalid: account must not be loaded",
    )
    .unwrap();
    fs::write(
        a.root().join("keys.json"),
        "invalid: keys must not be loaded",
    )
    .unwrap();
    let before = a.snapshot();
    for (name, options) in [
        ("decode_voice", vec![]),
        (
            "decode_voice",
            vec!["--media-output-root", "does-not-exist"],
        ),
        (
            "transcribe_voice",
            vec!["--media-output-root", output.path().to_str().unwrap()],
        ),
        (
            "transcribe_voice",
            vec![
                "--backend",
                "explicit-open-ai",
                "--api-key-file",
                key.to_str().unwrap(),
                "--openai-base-url",
                "forbidden-not-a-url",
                "--openai-model",
                "synthetic",
            ],
        ),
    ] {
        let mut mcp = a.mcp_with_args(&options);
        mcp.ready();
        let list = mcp.rpc("tools/list", json!({}));
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 17);
        safe_failure(mcp.call(name, args(700)), "Query backend unavailable");
        mcp.finish();
        assert_eq!(a.snapshot(), before);
        assert!(!a.root().join("decrypted").exists());
        assert!(!a.root().join("daemon.log").exists());
        assert!(!home.path().join("accounts").exists());
        assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
    }
}
