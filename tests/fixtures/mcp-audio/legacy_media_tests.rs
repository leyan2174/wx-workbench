use super::*;
use crate::database_media::{DatabaseMediaError, ErrorKind};

async fn legacy(f: &Fixture, id: i64) -> anyhow::Result<database_media::DatabaseVoice> {
    database_media::resolve_voice_media_row(&f.sources(), "wxid_peer", id, "message/media_0.db", 1)
        .map_err(Into::into)
}

fn kind(error: &anyhow::Error) -> ErrorKind {
    error.downcast_ref::<DatabaseMediaError>().unwrap().kind
}

#[tokio::test]
async fn media_coordinates_never_fall_back_to_matching_local_ids() {
    let f = Fixture::new(false).await;
    let before = f.snapshot();
    for (source, rowid) in [
        ("message/media_1.db", 1),
        ("message/media_0.db", 2),
        ("message/media_0.db", 0),
    ] {
        let error =
            database_media::resolve_voice_media_row(&f.sources(), "wxid_peer", 700, source, rowid)
                .unwrap_err();
        assert_eq!(error.kind, ErrorKind::MediaNotFound);
    }
    assert_eq!(before, f.snapshot());
}

#[tokio::test]
async fn repeated_local_ids_with_distinct_server_proofs_select_exact_media_shards() {
    let f = Fixture::new(false).await;
    let table = format!("Msg_{:x}", md5::compute("wxid_peer"));
    f.sql(
        1,
        &format!("INSERT INTO [{table}] VALUES(8,34,124,101,0,NULL)"),
    );
    f.sql(
        3,
        "INSERT INTO VoiceInfo VALUES(9,700,124,101,X'232153494C4B5F563342')",
    );
    let before = f.snapshot();
    let first = legacy(&f, 700).await.unwrap();
    let second = database_media::resolve_voice_media_row(
        &f.sources(),
        "wxid_peer",
        700,
        "message/media_1.db",
        1,
    )
    .unwrap();
    assert_eq!(first.evidence.message_local_id, 7);
    assert_eq!(second.evidence.message_local_id, 8);
    assert_eq!(second.evidence.media_source, "message/media_1.db");
    assert_ne!(first.silk, second.silk);
    assert_eq!(before, f.snapshot());
}

#[tokio::test]
async fn legacy_media_id_differs_from_message_id_and_never_falls_back() {
    for upper in [false, true] {
        let f = Fixture::new(upper).await;
        let before = f.snapshot();
        let voice = legacy(&f, 700).await.unwrap();
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
async fn media_coordinate_selects_original_row_despite_repeated_local_id() {
    for shard in [2, 3] {
        let f = Fixture::new(false).await;
        f.sql(shard, "INSERT INTO VoiceInfo VALUES(9,700,999,999,NULL)");
        let before = f.snapshot();
        let voice = legacy(&f, 700).await.unwrap();
        assert_eq!(voice.evidence.media_source, "message/media_0.db");
        assert_eq!(voice.evidence.media_rowid, 1);
        assert_eq!(voice.evidence.server_id, 100);
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
        assert_eq!(
            kind(&legacy(&f, 700).await.unwrap_err()),
            ErrorKind::AmbiguousMessage
        );
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
    assert_eq!(
        kind(&legacy(&f, 700).await.unwrap_err()),
        ErrorKind::AmbiguousMedia
    );
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
    let voice = legacy(&f, 700).await.unwrap();
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
    let va = legacy(&a, 700).await.unwrap();
    let vb = legacy(&b, 700).await.unwrap();
    assert_ne!(va.silk, vb.silk);
    assert_eq!(before_a, a.snapshot());
    assert_eq!(before_b, b.snapshot());
}

#[tokio::test]
async fn legacy_reverse_rejects_missing_and_corrupt_sources() {
    for index in 0..SOURCES.len() {
        let f = Fixture::new(false).await;
        fs::write(&f.paths[index], b"synthetic corrupt").unwrap();
        assert!(legacy(&f, 700).await.is_err());
        fs::remove_file(&f.paths[index]).unwrap();
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
        let voice = legacy(&f, id).await.unwrap();
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
    assert!(legacy(&f, 700).await.is_ok());
    assert_eq!(before, f.snapshot());
    f.sql(1, &format!("CREATE VIEW [{table}] AS SELECT 1"));
    let before = f.snapshot();
    assert_eq!(
        kind(&legacy(&f, 700).await.unwrap_err()),
        ErrorKind::UnsupportedSchema
    );
    assert_eq!(before, f.snapshot());
}
