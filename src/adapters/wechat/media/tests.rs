use super::*;
use crate::adapters::wechat::messages::SourceFile;
use rusqlite::Connection;

fn message_file(root: &Path, name: &str, copies: usize) -> SourceFile {
    let path = root.join(name);
    let conn = Connection::open(&path).unwrap();
    let table = format!("Msg_{:x}", md5::compute("synthetic"));
    conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,server_id INTEGER,message_content BLOB,WCDB_CT_message_content INTEGER)")).unwrap();
    for _ in 0..copies {
        conn.execute(
            &format!("INSERT INTO [{table}] VALUES(7,3,123,91,?1,0)"),
            [b"synthetic".as_slice()],
        )
        .unwrap();
    }
    SourceFile {
        logical_name: format!("message/{name}"),
        path,
        kind: SourceKind::Ordinary,
    }
}

fn resource_file(root: &Path, copies: usize) -> std::path::PathBuf {
    let path = root.join("resource.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE ChatName2Id(user_name TEXT); INSERT INTO ChatName2Id VALUES('synthetic'); CREATE TABLE MessageResourceInfo(chat_id INTEGER,message_local_id INTEGER,message_local_type INTEGER,message_create_time INTEGER,packed_info BLOB)").unwrap();
    for _ in 0..copies {
        conn.execute(
            "INSERT INTO MessageResourceInfo VALUES(1,7,3,123,?1)",
            [b"0123456789abcdef0123456789abcdef".as_slice()],
        )
        .unwrap();
    }
    path
}

fn selector() -> MessageSelector<'static> {
    MessageSelector {
        username: "synthetic",
        local_id: 7,
        timestamp: Some(123),
    }
}

#[test]
fn listing_full_type_identity_does_not_weaken_strict_decode_identity() {
    let root = tempfile::tempdir().unwrap();
    let file = message_file(root.path(), "message_0.db", 1);
    let table = format!("Msg_{:x}", md5::compute("synthetic"));
    let conn = Connection::open(&file.path).unwrap();
    conn.execute_batch(&format!(
        "ALTER TABLE [{table}] ADD COLUMN real_sender_id INTEGER DEFAULT 0;
         INSERT INTO [{table}] (local_id,local_type,create_time,server_id,message_content,WCDB_CT_message_content)
         VALUES(7,1,123,92,X'61',0),(7,4294967299,123,93,X'62',0)"
    )).unwrap();
    drop(conn);
    let snapshot = Snapshot::open(vec![file], ["synthetic".into()]).unwrap();
    let plain = image_listing_reference(&snapshot, &selector(), 3).unwrap();
    let flagged = image_listing_reference(&snapshot, &selector(), 3 + (1i64 << 32)).unwrap();
    assert_ne!(plain, flagged);
    let error = snapshot
        .resolve(&selector(), SourceKind::Ordinary)
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<crate::business::messages::Error>(),
        Some(&crate::business::messages::Error::Ambiguous)
    );
}

#[test]
fn directory_digest_uses_all_sources_and_preserves_raw_identity() {
    let root = tempfile::tempdir().unwrap();
    let first = message_file(root.path(), "message_0.db", 1);
    let second = message_file(root.path(), "message_1.db", 1);
    let resources = resource_file(root.path(), 1);
    let one = Snapshot::open(vec![first.clone()], ["synthetic".into()]).unwrap();
    assert!(image_digest(
        &one,
        &selector(),
        &first.logical_name,
        3,
        &[resources.clone()]
    )
    .is_ok());
    assert_eq!(
        image_digest(
            &one,
            &selector(),
            &first.logical_name,
            3 + (1i64 << 32),
            &[resources.clone()]
        )
        .unwrap_err()
        .failure,
        Failure::StaleEvidence
    );
    let all = Snapshot::open(vec![first.clone(), second], ["synthetic".into()]).unwrap();
    assert_eq!(
        image_digest(&all, &selector(), &first.logical_name, 3, &[resources])
            .unwrap_err()
            .failure,
        Failure::Ambiguous
    );
}

#[test]
fn directory_digest_keeps_reused_local_ids_at_distinct_times_separate() {
    let root = tempfile::tempdir().unwrap();
    let file = message_file(root.path(), "message_0.db", 1);
    let resources = resource_file(root.path(), 1);
    let table = format!("Msg_{:x}", md5::compute("synthetic"));
    Connection::open(&file.path)
        .unwrap()
        .execute_batch(&format!("INSERT INTO [{table}] VALUES(7,3,124,92,X'61',0)"))
        .unwrap();
    Connection::open(&resources)
        .unwrap()
        .execute(
            "INSERT INTO MessageResourceInfo VALUES(1,7,3,124,?1)",
            [b"abcdef0123456789abcdef0123456789".as_slice()],
        )
        .unwrap();
    let snapshot = Snapshot::open(vec![file.clone()], ["synthetic".into()]).unwrap();
    let early = image_digest(
        &snapshot,
        &selector(),
        &file.logical_name,
        3,
        &[resources.clone()],
    )
    .unwrap();
    let later = image_digest(
        &snapshot,
        &MessageSelector {
            timestamp: Some(124),
            ..selector()
        },
        &file.logical_name,
        3,
        &[resources],
    )
    .unwrap();
    assert_ne!(early, later);
}

#[test]
fn discovery_only_reads_resource_metadata_and_revalidates_its_issuer() {
    let root = tempfile::tempdir().unwrap();
    let file = message_file(root.path(), "message_0.db", 1);
    let resources = resource_file(root.path(), 1);
    let snapshot = Snapshot::open(vec![file], ["synthetic".into()]).unwrap();
    let mut source = ImageSource::open(&snapshot, &selector(), &resources).unwrap();
    let message = source.message().clone();
    let discovered = source.discover(&message, Kind::Image).unwrap();
    let reference = &discovered[0].reference;
    assert_eq!(discovered[0].stored_bytes, None);
    assert_eq!(reference.association(), AssociationPolicy::StrictMessage);
    assert_eq!(reference.completeness(), Completeness::Complete);
    assert_eq!(
        source.resource_evidence(reference).unwrap(),
        (1, "0123456789abcdef0123456789abcdef")
    );
    let mut other = ImageSource::open(&snapshot, &selector(), &resources).unwrap();
    other.discover(&message, Kind::Image).unwrap();
    assert_eq!(
        other.revalidate(reference).unwrap_err().failure,
        Failure::StaleEvidence
    );
    let forged = Reference::new(
        &source.owner,
        message,
        Kind::Voice,
        Some(1),
        AssociationPolicy::ExplicitLegacyMediaId,
        Completeness::Complete,
    )
    .unwrap();
    assert_eq!(
        source.revalidate(&forged).unwrap_err().failure,
        Failure::InvalidReference
    );
    // No DAT file, output directory, decoder, credentials or network endpoint exists.
}

#[test]
fn same_time_message_and_resource_duplicates_are_not_absence() {
    let root = tempfile::tempdir().unwrap();
    let first = message_file(root.path(), "message_0.db", 1);
    let second = message_file(root.path(), "message_1.db", 1);
    let resources = resource_file(root.path(), 2);
    let snapshot = Snapshot::open(vec![first, second], ["synthetic".into()]).unwrap();
    assert_eq!(
        ImageSource::open(&snapshot, &selector(), &resources)
            .err()
            .unwrap()
            .failure,
        Failure::Ambiguous
    );
    drop(snapshot);
    let snapshot = Snapshot::open(
        vec![SourceFile {
            logical_name: "message/message_0.db".into(),
            path: root.path().join("message_0.db"),
            kind: SourceKind::Ordinary,
        }],
        ["synthetic".into()],
    )
    .unwrap();
    let mut source = ImageSource::open(&snapshot, &selector(), &resources).unwrap();
    let message = source.message().clone();
    assert_eq!(
        source.discover(&message, Kind::Image).unwrap_err().failure,
        Failure::Ambiguous
    );
}
