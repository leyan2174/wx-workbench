use super::*;
use crate::database_media::{DatabaseMediaError, ErrorKind};
use crate::mcp_audio::{resolve_legacy_audio, LegacyAudioResolution};

async fn legacy(f: &Fixture, id: i64) -> anyhow::Result<LegacyAudioResolution> {
    resolve_legacy_audio(&f.db, &f.names, "wxid_peer", id).await
}

fn kind(error: &anyhow::Error) -> ErrorKind {
    error.downcast_ref::<DatabaseMediaError>().unwrap().kind
}

#[tokio::test]
async fn legacy_media_id_differs_from_message_id_and_never_falls_back() {
    for upper in [false, true] {
        let f = Fixture::new(upper).await;
        let before = f.snapshot();
        let LegacyAudioResolution::Found(voice) = legacy(&f, 700).await.unwrap() else {
            panic!("expected unique voice");
        };
        assert_eq!(voice, f.message_voice(Some(123)).unwrap());
        assert_eq!(voice.evidence.media_local_id, 700);
        assert_eq!(voice.evidence.message_local_id, 7);
        assert_eq!(
            kind(&legacy(&f, 7).await.unwrap_err()),
            ErrorKind::MediaNotFound
        );
        assert_eq!(before, f.snapshot());
    }
}

#[tokio::test]
async fn legacy_repeated_media_id_is_structured_ambiguity_without_type_or_time_filter() {
    for shard in [2, 3] {
        let f = Fixture::new(false).await;
        f.sql(shard, "INSERT INTO VoiceInfo VALUES(9,700,999,999,NULL)");
        let before = f.snapshot();
        assert!(matches!(
            legacy(&f, 700).await.unwrap(),
            LegacyAudioResolution::AmbiguousMedia
        ));
        assert_eq!(before, f.snapshot());
    }
}

#[tokio::test]
async fn legacy_reverse_server_ambiguity_precedes_message_type_and_timestamp() {
    let table = format!("Msg_{:x}", md5::compute("wxid_peer"));
    for shard in [0, 1] {
        let f = Fixture::new(false).await;
        f.sql(0, &format!("UPDATE [{table}] SET local_type=1"));
        f.sql(
            shard,
            &format!("INSERT INTO [{table}] VALUES(8,34,999,100,0,NULL)"),
        );
        let before = f.snapshot();
        assert!(matches!(
            legacy(&f, 700).await.unwrap(),
            LegacyAudioResolution::AmbiguousMessage
        ));
        assert_eq!(before, f.snapshot());
    }
}

#[tokio::test]
async fn legacy_unique_reverse_candidate_still_requires_forward_media_uniqueness() {
    let f = Fixture::new(false).await;
    f.sql(
        3,
        "INSERT INTO VoiceInfo VALUES(9,701,123,100,X'232153494C4B5F5633')",
    );
    let before = f.snapshot();
    assert!(matches!(
        legacy(&f, 700).await.unwrap(),
        LegacyAudioResolution::AmbiguousMedia
    ));
    assert_eq!(before, f.snapshot());
}

#[tokio::test]
async fn legacy_server_proof_selects_source_not_a_guessed_matching_number() {
    let f = Fixture::new(false).await;
    let table = format!("Msg_{:x}", md5::compute("wxid_peer"));
    f.sql(
        1,
        &format!("INSERT INTO [{table}] VALUES(7,34,123,999,0,NULL)"),
    );
    f.sql(
        0,
        &format!("INSERT INTO [{table}] VALUES(700,1,123,998,0,NULL)"),
    );
    let before = f.snapshot();
    let LegacyAudioResolution::Found(voice) = legacy(&f, 700).await.unwrap() else {
        panic!("expected server-proven message");
    };
    assert_eq!(voice.evidence.message_source, "message/message_0.db");
    assert_eq!(voice.evidence.message_local_id, 7);
    assert_eq!(before, f.snapshot());
}

#[tokio::test]
async fn legacy_unproven_or_conflicting_messages_never_return_bytes() {
    let table = format!("Msg_{:x}", md5::compute("wxid_peer"));
    for (shard, sql, expected) in [
        (
            0,
            format!("DELETE FROM [{table}]"),
            ErrorKind::MessageNotFound,
        ),
        (
            0,
            format!("UPDATE [{table}] SET local_type=1"),
            ErrorKind::NotVoice,
        ),
        (
            0,
            format!("UPDATE [{table}] SET create_time=124"),
            ErrorKind::ConflictingEvidence,
        ),
        (
            2,
            "UPDATE VoiceInfo SET svr_id=0".into(),
            ErrorKind::ConflictingEvidence,
        ),
        (
            2,
            "UPDATE VoiceInfo SET voice_data=NULL".into(),
            ErrorKind::InvalidVoiceData,
        ),
        (
            2,
            "INSERT INTO Name2Id(user_name) VALUES('wxid_peer')".into(),
            ErrorKind::AmbiguousContact,
        ),
    ] {
        let f = Fixture::new(false).await;
        f.sql(shard, &sql);
        let before = f.snapshot();
        assert_eq!(kind(&legacy(&f, 700).await.unwrap_err()), expected);
        assert_eq!(before, f.snapshot());
    }
}

#[tokio::test]
async fn legacy_identical_ids_remain_separate_across_contacts_and_accounts() {
    let a = Fixture::new(false).await;
    let b = Fixture::new(false).await;
    a.sql(3, "INSERT INTO Name2Id(rowid,user_name) VALUES(10,'other'); INSERT INTO VoiceInfo VALUES(10,700,123,100,X'232153494C4B5F5633')");
    b.sql(2, "UPDATE VoiceInfo SET voice_data=X'232153494C4B5F563342'");
    let before_a = a.snapshot();
    let before_b = b.snapshot();
    let LegacyAudioResolution::Found(va) = legacy(&a, 700).await.unwrap() else {
        panic!("expected account A");
    };
    let LegacyAudioResolution::Found(vb) = legacy(&b, 700).await.unwrap() else {
        panic!("expected account B");
    };
    assert_ne!(va.silk, vb.silk);
    assert_eq!(before_a, a.snapshot());
    assert_eq!(before_b, b.snapshot());
}

#[tokio::test]
async fn legacy_complete_inventory_rejects_missing_unknown_and_corrupt_sources() {
    for index in 0..5 {
        let f = Fixture::new(false).await;
        fs::write(&f.paths[index], b"synthetic corrupt DB").unwrap();
        let before = f.snapshot();
        assert!(legacy(&f, 700).await.is_err());
        assert_eq!(before, f.snapshot());
    }
    for source in [
        "message/message_1.db",
        "message/media_1.db",
        "contact/contact.db",
    ] {
        let f = Fixture::new(true).await;
        fs::remove_file(f.db.db_dir().join(source)).unwrap();
        assert!(legacy(&f, 700).await.is_err());
    }
    for source in ["MEDIA_9.DB", "MESSAGE_9.DB"] {
        let f = Fixture::new(false).await;
        fs::write(
            f.db.db_dir().join("message").join(source),
            b"unknown synthetic DB",
        )
        .unwrap();
        assert!(legacy(&f, 700).await.is_err());
    }
}

#[tokio::test]
async fn legacy_media_id_preserves_exact_sqlite_i64_values() {
    for id in [i64::MIN, 0, i64::MAX] {
        let f = Fixture::new(false).await;
        Connection::open(&f.paths[2])
            .unwrap()
            .execute("UPDATE VoiceInfo SET local_id=?1", [id])
            .unwrap();
        let before = f.snapshot();
        let LegacyAudioResolution::Found(voice) = legacy(&f, id).await.unwrap() else {
            panic!("expected exact media id");
        };
        assert_eq!(voice.evidence.media_local_id, id);
        assert_eq!(voice.evidence.message_local_id, 7);
        assert_eq!(before, f.snapshot());
    }
}

#[tokio::test]
async fn legacy_absent_table_is_not_a_same_named_unsupported_view() {
    let f = Fixture::new(false).await;
    let table = format!("Msg_{:x}", md5::compute("wxid_peer"));
    f.sql(1, &format!("DROP TABLE [{table}]"));
    let before = f.snapshot();
    assert!(matches!(
        legacy(&f, 700).await.unwrap(),
        LegacyAudioResolution::Found(_)
    ));
    assert_eq!(before, f.snapshot());
    f.sql(1, &format!("CREATE VIEW [{table}] AS SELECT 1"));
    let before = f.snapshot();
    assert_eq!(
        kind(&legacy(&f, 700).await.unwrap_err()),
        ErrorKind::UnsupportedSchema
    );
    assert_eq!(before, f.snapshot());
}
