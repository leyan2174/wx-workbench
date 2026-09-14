//! 为严格资源读取器准备独立快照；不修改缓存的日志模式或删除伴随文件。

use anyhow::{ensure, Context, Result};
use rusqlite::{backup::Backup, Connection, OpenFlags};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(crate) struct ResourceSnapshot(tempfile::TempDir);

impl ResourceSnapshot {
    pub(crate) fn new(source: &Path) -> Result<Self> {
        let before = std::fs::metadata(source)?;
        let identity = same_file::Handle::from_path(source)?;
        ensure!(before.is_file(), "resource cache must be a file");
        // 与缓存共用受限父目录，避免把解密数据写入系统公共临时目录。
        let directory = tempfile::Builder::new()
            .prefix("resource-snapshot-")
            .tempdir_in(source.parent().context("resource cache parent missing")?)?;
        let snapshot = Self(directory);
        let input = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        input.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
        let mut output = Connection::open(snapshot.path())?;
        {
            let backup = Backup::new(&input, &mut output)?;
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                ensure!(Instant::now() < deadline, "resource snapshot timed out");
                match backup.step(256)? {
                    rusqlite::backup::StepResult::Done => break,
                    rusqlite::backup::StepResult::More => {}
                    rusqlite::backup::StepResult::Busy | rusqlite::backup::StepResult::Locked => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    _ => anyhow::bail!("unexpected resource backup state"),
                }
            }
        }
        // 备份可能继承源库的 WAL 标志；只在尚未发布的私有副本上转换日志模式。
        let mode: String = output.query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))?;
        ensure!(
            mode.eq_ignore_ascii_case("delete"),
            "resource snapshot journal mode invalid"
        );
        drop(output);
        drop(input);
        let after = std::fs::metadata(source)?;
        ensure!(
            identity == same_file::Handle::from_path(source)?
                && before.len() == after.len()
                && before.modified()? == after.modified()?,
            "resource cache changed during snapshot"
        );
        Ok(snapshot)
    }

    pub(crate) fn path(&self) -> std::path::PathBuf {
        self.0.path().join("resource.db")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wal_snapshot_contains_commits_only_and_removes_its_own_files() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source.db");
        let writer = Connection::open(&source)?;
        writer.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE data(value); INSERT INTO data VALUES(1); BEGIN; INSERT INTO data VALUES(2);")?;
        let snapshot = ResourceSnapshot::new(&source)?;
        let path = snapshot.path();
        let reader = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let count: i64 = reader.query_row("SELECT count(*) FROM data", [], |row| row.get(0))?;
        assert_eq!(count, 1);
        let mode: String = reader.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        assert_eq!(mode, "delete");
        assert!(!Path::new(&format!("{}-wal", path.display())).exists());
        assert!(Path::new(&format!("{}-wal", source.display())).exists());
        drop(reader);
        drop(snapshot);
        assert!(!path.exists());
        writer.execute_batch("ROLLBACK")?;
        Ok(())
    }
}
