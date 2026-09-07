use super::*;

#[test]
fn source_fingerprints_detect_byte_changes_and_new_wal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    fs::write(&path, b"synthetic source").unwrap();
    let paths = vec![path.clone()];
    let before = source_states(&paths).unwrap();
    assert!(before == source_states(&paths).unwrap());
    fs::write(&path, b"different synthetic source bytes").unwrap();
    assert!(before != source_states(&paths).unwrap());
    let before = source_states(&paths).unwrap();
    fs::write(dir.path().join("source.db-wal"), b"synthetic wal").unwrap();
    assert!(before != source_states(&paths).unwrap());
}

#[test]
fn source_fingerprints_reject_missing_sources_and_directory_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    assert!(source_states(&[path.clone()]).is_err());
    fs::write(&path, b"synthetic").unwrap();
    fs::create_dir(dir.path().join("source.db-wal")).unwrap();
    assert!(source_states(&[path]).is_err());
}
