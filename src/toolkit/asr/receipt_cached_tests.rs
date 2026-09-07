use super::*;
use crate::toolkit::asr::{
    database_media::VoiceEvidence,
    receipt::{self, LookupOutcome, ReceiptState},
};

fn voice_evidence() -> VoiceEvidence {
    VoiceEvidence {
        username: "alice".into(),
        message_source: "message/message_0.db".into(),
        message_table: format!("Msg_{:x}", md5::compute("alice")),
        message_local_id: 1,
        server_id: 100,
        create_time: 100,
        media_source: "message/media_0.db".into(),
        media_rowid: 1,
        media_chat_name_id: 9,
        media_local_id: 700,
    }
}

fn voice_request(path: &Path) -> CachedRequest<'_> {
    CachedRequest {
        source: "message/message_0.db",
        ..request(path)
    }
}

#[test]
fn actual_cached_transcription_then_deleted_audio_hits_receipt_without_asr() {
    for mode in ["ok", "empty"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let source = dir.path().join("source.silk");
        fs::write(&source, SILK).unwrap();
        let good = backend(dir.path(), mode);
        let req = voice_request(&path);
        let evidence = voice_evidence();
        let result =
            transcribe_cached_with_receipt_checked(&req, &evidence, &good, |_| Ok(())).unwrap();
        assert_eq!(result.cached.cache_state, CacheState::Stored);
        assert_eq!(result.receipt, ReceiptState::Stored);
        fs::remove_file(&source).unwrap();
        let before = fs::read(&path).unwrap();
        let LookupOutcome::Hit(hit) =
            receipt::lookup_success(&path, "account-a", "alice", 700, &good)
        else {
            panic!("source-free cache hit expected");
        };
        assert_eq!(hit.transcription, result.cached.transcription);
        assert_eq!(hit.create_time, 100);
        assert_eq!(calls(dir.path()), 1);
        assert_eq!(before, fs::read(&path).unwrap());
        let again = transcribe_cached_with_receipt_checked(&req, &evidence, &good, |_| {
            panic!("no publication")
        })
        .unwrap();
        assert_eq!(again.cached.cache_state, CacheState::Hit);
        assert_eq!(again.receipt, ReceiptState::AlreadyPresent);
        assert_eq!(calls(dir.path()), 1);
    }
}

#[test]
fn existing_verified_hit_can_add_receipt_without_rerunning_backend() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let good = backend(dir.path(), "ok");
    let req = voice_request(&path);
    transcribe_cached(&req, &good).unwrap();
    assert!(matches!(
        receipt::lookup_success(&path, "account-a", "alice", 700, &good),
        LookupOutcome::Miss
    ));
    let before = fs::read(&path).unwrap();
    let mut checks = 0;
    let denied = transcribe_cached_with_receipt_checked(&req, &voice_evidence(), &good, |_| {
        checks += 1;
        anyhow::bail!("synthetic publication denial")
    })
    .unwrap();
    assert_eq!(denied.cached.cache_state, CacheState::Hit);
    assert_eq!(denied.receipt, ReceiptState::Unavailable);
    assert_eq!(checks, 1);
    assert_eq!(before, fs::read(&path).unwrap());
    let result =
        transcribe_cached_with_receipt_checked(&req, &voice_evidence(), &good, |_| Ok(())).unwrap();
    assert_eq!(result.cached.cache_state, CacheState::Hit);
    assert_eq!(result.receipt, ReceiptState::Stored);
    assert_eq!(calls(dir.path()), 1);
}

#[test]
fn checked_denial_never_publishes_half_an_entry_or_receipt() {
    for reject_at in [1, 2] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let good = backend(dir.path(), "ok");
        let req = voice_request(&path);
        let mut checks = 0;
        let result = transcribe_cached_with_receipt_checked(&req, &voice_evidence(), &good, |_| {
            checks += 1;
            anyhow::ensure!(checks != reject_at, "synthetic rejection");
            Ok(())
        });
        assert_eq!(checks, reject_at);
        if reject_at == 1 {
            assert!(result.is_err());
        } else {
            let result = result.unwrap();
            assert_eq!(result.cached.cache_state, CacheState::WriteUnavailable);
            assert_eq!(result.receipt, ReceiptState::Unavailable);
        }
        assert!(!path.exists());
        assert!(!dir.path().join(".cache.json.asr-cache.lock").exists());
        assert!(!fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|v| v.file_name().to_string_lossy().starts_with(".wx-cache-")));
        assert_eq!(calls(dir.path()), 1);
    }
}

#[test]
fn evidence_mismatch_precedes_model_and_cache_access() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let missing = Backend::Local(crate::toolkit::asr::local::LocalConfig::new(
        dir.path().join("missing.exe"),
        dir.path().join("missing.bin"),
    ));
    for field in 0..5 {
        let mut evidence = voice_evidence();
        match field {
            0 => evidence.username = "other".into(),
            1 => evidence.message_source = "message/message_1.db".into(),
            2 => evidence.message_local_id += 1,
            3 => evidence.create_time += 1,
            _ => evidence.media_local_id = 0,
        }
        let error = transcribe_cached_with_receipt_checked(
            &voice_request(&path),
            &evidence,
            &missing,
            |_| Ok(()),
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("receipt"));
        assert!(!path.exists());
    }
}

#[test]
fn repeated_media_proof_conflict_persists_even_when_message_cache_hits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let good = backend(dir.path(), "ok");
    let req = voice_request(&path);
    let mut evidence = voice_evidence();
    transcribe_cached_with_receipt_checked(&req, &evidence, &good, |_| Ok(())).unwrap();
    evidence.server_id += 1;
    let result =
        transcribe_cached_with_receipt_checked(&req, &evidence, &good, |_| Ok(())).unwrap();
    assert_eq!(result.cached.cache_state, CacheState::Hit);
    assert_eq!(result.receipt, ReceiptState::Conflict);
    assert!(matches!(
        receipt::lookup_success(&path, "account-a", "alice", 700, &good),
        LookupOutcome::Conflict
    ));
    assert_eq!(calls(dir.path()), 1);
}

#[test]
fn invalid_receipt_extension_preserves_disk_and_does_not_erase_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.json");
    let good = backend(dir.path(), "ok");
    let mut req = voice_request(&path);
    transcribe_cached(&req, &good).unwrap();
    let mut data: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    data["_wx_asr_receipts"] = serde_json::json!({"version": 99, "entries": {}});
    let before = serde_json::to_vec(&data).unwrap();
    fs::write(&path, &before).unwrap();
    let hit =
        transcribe_cached_with_receipt_checked(&req, &voice_evidence(), &good, |_| Ok(())).unwrap();
    assert_eq!(hit.cached.cache_state, CacheState::Hit);
    assert_eq!(hit.receipt, ReceiptState::Unavailable);
    assert_eq!(calls(dir.path()), 1);
    req.silk = include_bytes!("../../../tests/fixtures/audio/tone.silk");
    let fresh =
        transcribe_cached_with_receipt_checked(&req, &voice_evidence(), &good, |_| Ok(())).unwrap();
    assert_eq!(fresh.cached.transcription.text, "synthetic text");
    assert_eq!(fresh.cached.cache_state, CacheState::WriteUnavailable);
    assert_eq!(fresh.receipt, ReceiptState::Unavailable);
    assert_eq!(calls(dir.path()), 2);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(matches!(
        receipt::lookup_success(&path, "account-a", "alice", 700, &good),
        LookupOutcome::Unavailable
    ));
}
