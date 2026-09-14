#[path = "mcp-readonly-runtime/encrypted_sqlite.rs"]
pub(crate) mod encrypted_sqlite;

pub use encrypted_sqlite::sqlite;

pub fn seed(plain: &std::path::Path, source: &std::path::Path) -> u64 {
    let scratch = tempfile::tempdir().unwrap();
    let copy = scratch.path().join("plain.db");
    std::fs::copy(plain, &copy).unwrap();
    encrypted_sqlite::encrypt(&copy, source);
    std::fs::metadata(source)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}
