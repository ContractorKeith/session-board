use super::{SourceScan, sqlite_vfs};
use rusqlite::{Connection, OpenFlags};
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
    time::SystemTime,
};

// Inspect the path itself: even a symlink to a regular file is not a regular
// store/sidecar entry. Missing sidecars are normal; missing stores are counted.
fn regular_entry(path: &Path) -> io::Result<Option<std::fs::Metadata>> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => Ok(Some(meta)),
        Ok(_) => Err(crate::storage::at(path, "not a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(crate::storage::at(path, error)),
    }
}
fn check_paths(path: &Path) -> io::Result<()> {
    regular_entry(path)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "database disappeared"))?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        regular_entry(Path::new(&name))?;
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64),
    #[cfg(unix)]
    changed: (i64, i64),
}
impl Stamp {
    fn new(meta: std::fs::Metadata) -> io::Result<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            size: meta.len(),
            modified: meta.modified()?,
            #[cfg(unix)]
            identity: (meta.dev(), meta.ino()),
            #[cfg(unix)]
            changed: (meta.ctime(), meta.ctime_nsec()),
        })
    }
    fn same_file(&self, other: &Self) -> bool {
        #[cfg(unix)]
        {
            self.identity == other.identity
        }
        #[cfg(not(unix))]
        {
            self == other
        }
    }
}
#[derive(PartialEq, Eq)]
struct Snapshot {
    database: Stamp,
    // Header change counter (24..28), version-valid-for and SQLite version (92..100).
    header: [u8; 12],
    // WAL, SHM, rollback journal; include identity/ctime as well as size/mtime.
    sidecars: [Option<Stamp>; 3],
}
impl Snapshot {
    fn read(path: &Path) -> io::Result<Self> {
        check_paths(path)?;
        let mut file = std::fs::File::open(path)?;
        let database = Stamp::new(file.metadata()?)?;
        let mut header = [0; 100];
        file.read_exact(&mut header)?;
        let mut signals = [0; 12];
        signals[..4].copy_from_slice(&header[24..28]);
        signals[4..].copy_from_slice(&header[92..100]);
        let sidecar = |suffix: &str| {
            let mut name = path.as_os_str().to_os_string();
            name.push(suffix);
            regular_entry(Path::new(&name))?.map(Stamp::new).transpose()
        };
        Ok(Self {
            database,
            header: signals,
            sidecars: [sidecar("-wal")?, sidecar("-shm")?, sidecar("-journal")?],
        })
    }
    fn same_open_target(&self, other: &Self) -> bool {
        self.database.same_file(&other.database)
            && self
                .sidecars
                .iter()
                .zip(&other.sidecars)
                .all(|(before, after)| match (before, after) {
                    (Some(before), Some(after)) => before.same_file(after),
                    (None, None) => true,
                    _ => false,
                })
    }
}
struct Database {
    connection: Connection,
    path: PathBuf,
    stable_snapshot: Option<Snapshot>,
}

/// Normal read-only locking applies to every on-disk store, including MEMORY-journal writers.
fn open(path: &Path, result: &mut SourceScan) -> Option<Database> {
    open_with(path, result, || {})
}
fn open_with(
    path: &Path,
    result: &mut SourceScan,
    before_connection: impl FnOnce(),
) -> Option<Database> {
    if let Err(error) = check_paths(path) {
        result.skipped_stores += 1;
        if error.kind() != io::ErrorKind::NotFound {
            result.warnings.push(format!("{}: {error}", path.display()));
        }
        return None;
    }
    let absolute = match std::fs::canonicalize(path) {
        Ok(path) => path,
        Err(error) => {
            failed(path, &error, result);
            return None;
        }
    };
    // Percent-encode URI delimiters, including literal '%' in project paths.
    let mut uri = String::from("file:");
    for byte in absolute.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(byte) {
            uri.push(char::from(*byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_URI;
    let before = match Snapshot::read(&absolute) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            failed(path, &error, result);
            return None;
        }
    };
    let wal = before.sidecars[0].is_some();
    let query = "mode=ro";
    before_connection();
    let connection = match sqlite_vfs::open(&format!("{uri}?{query}"), flags) {
        Ok(connection) => connection,
        Err(error) => {
            failed(path, &error, result);
            return None;
        }
    };
    // sqlite3_open_v2 normally defers WAL/SHM access until the schema probe.
    // Revalidate before that first read; the VFS also guards the later OS open.
    match Snapshot::read(&absolute) {
        Ok(after) if before.same_open_target(&after) => (),
        Ok(_) => {
            drop(connection);
            failed(
                path,
                &"SQLite store or sidecar set changed during open; discarded connection",
                result,
            );
            return None;
        }
        Err(error) => {
            drop(connection);
            failed(path, &error, result);
            return None;
        }
    }
    let probe = connection
        .busy_timeout(std::time::Duration::from_millis(200))
        .and_then(|()| connection.query_row("SELECT count(*) FROM sqlite_schema", [], |_| Ok(())));
    if let Err(error) = probe {
        failed(path, &error, result);
        return None;
    }
    Some(Database {
        connection,
        path: absolute,
        stable_snapshot: (!wal).then_some(before),
    })
}

/// Keep validation around the complete scan so neither source can return a
/// stale file view after a writer replaces or changes a non-WAL store.
pub(super) fn scan(
    path: &Path,
    result: &mut SourceScan,
    read: impl FnOnce(&Connection, &mut SourceScan) -> rusqlite::Result<()>,
) {
    let Some(database) = open(path, result) else {
        return;
    };
    if let Err(error) = read(&database.connection, result) {
        failed(path, &error, result);
        return;
    }
    if let Some(before) = database.stable_snapshot {
        match Snapshot::read(&database.path) {
            Ok(after) if before == after => (),
            Ok(_) => failed(
                path,
                &"database changed during scan; discarded result",
                result,
            ),
            Err(error) => failed(path, &error, result),
        }
    }
}

pub(super) fn failed(path: &Path, error: &impl std::fmt::Display, result: &mut SourceScan) {
    result.sessions.clear();
    result.status_reasons.clear();
    result.skipped_stores += 1;
    result.warnings.push(format!("{}: {error}", path.display()));
}

/// Only conversion failures concern a row; step/I/O errors fail the whole store.
pub(super) fn row<T>(row: rusqlite::Result<T>, skipped: &mut usize) -> rusqlite::Result<Option<T>> {
    match row {
        Ok(value) => Ok(Some(value)),
        Err(
            rusqlite::Error::InvalidColumnType(..)
            | rusqlite::Error::FromSqlConversionFailure(..)
            | rusqlite::Error::IntegralValueOutOfRange(..),
        ) => {
            *skipped += 1;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub(super) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn issue68_database_fd_and_sql_writes_are_read_only() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE test(id)")
            .unwrap();
        // Exercise WAL protocol with a genuinely read-only database fd.
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch("PRAGMA journal_mode=WAL; INSERT INTO test VALUES (1)")
            .unwrap();
        let mut result = SourceScan::default();
        let reader = open(&path, &mut result).unwrap();
        assert!(result.warnings.is_empty());
        #[cfg(target_os = "linux")]
        {
            let target = std::fs::canonicalize(&path).unwrap();
            let descriptors: Vec<_> = std::fs::read_dir("/proc/self/fd")
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| std::fs::read_link(entry.path()).ok().as_ref() == Some(&target))
                .collect();
            // One writer and one reader descriptor must be distinguishable.
            let access: Vec<_> = descriptors
                .iter()
                .map(|entry| {
                    let info = std::fs::read_to_string(
                        std::path::Path::new("/proc/self/fdinfo").join(entry.file_name()),
                    )
                    .unwrap();
                    let flags = info
                        .lines()
                        .find_map(|line| line.strip_prefix("flags:\t"))
                        .unwrap();
                    u32::from_str_radix(flags, 8).unwrap() & 3
                })
                .collect();
            assert_eq!(
                access.iter().filter(|&&mode| mode == 0).count(),
                1,
                "fd modes: {access:?}"
            );
        }
        let error = reader
            .connection
            .execute("INSERT INTO test VALUES (2)", [])
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("attempt to write a readonly database"),
            "{error}"
        );
    }
    #[test]
    fn issue83_writer_spills_after_first_query_and_reader_respects_lock() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.db");
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("CREATE TABLE test(title TEXT); PRAGMA synchronous=OFF; PRAGMA cache_size=2; BEGIN; WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<3000) INSERT INTO test SELECT 'COMMITTED' || printf('%01000d', x) FROM n; COMMIT;").unwrap();
        // Pre-opened writer: it starts only after open()'s schema query. Keeping
        // it alive preserves the exclusive lock and uncommitted spilled pages.
        let mut result = SourceScan::default();
        scan(&path, &mut result, |connection, result| {
            writer.execute_batch(
                "BEGIN IMMEDIATE; UPDATE test SET title='UNCOMMITTED' || printf('%01000d', rowid);",
            )?;
            let title: String = connection.query_row(
                "SELECT title FROM test WHERE title LIKE 'UNCOMMITTED%' LIMIT 1",
                [],
                |r| r.get(0),
            )?;
            assert!(
                title.starts_with("UNCOMMITTED"),
                "race fixture did not spill"
            );
            result.sessions.push(crate::model::Session {
                id: "grok:race".into(),
                agent: crate::model::Agent::Grok,
                cwd: temp.path().into(),
                project_root: temp.path().into(),
                title,
                started_at: 1793534400,
                last_activity_at: 1793534400,
                status: crate::model::Status::Done,
                last_user_text: String::new(),
                last_assistant_text: String::new(),
                transcript_path: path.clone(),
                resume_hint: String::new(),
                pane: None,
            });
            Ok(())
        });
        assert!(result.sessions.is_empty(), "uncommitted title escaped");
        assert_eq!(result.skipped_stores, 1);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains(path.to_str().unwrap()));
        assert!(
            result.warnings[0].contains("locked"),
            "{:?}",
            result.warnings
        );
        writer.execute_batch("ROLLBACK").unwrap();
    }
    #[test]
    fn issue83_database_size_mtime_and_sidecar_changes_are_checked() {
        for change in ["size", "mtime", "sidecar"] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("store.db");
            Connection::open(&path)
                .unwrap()
                .execute_batch("CREATE TABLE test(id)")
                .unwrap();
            let mut result = SourceScan::default();
            scan(&path, &mut result, |connection, _| {
                connection.query_row("SELECT count(*) FROM test", [], |_| Ok(()))?;
                match change {
                    "size" => {
                        use std::io::Write;
                        std::fs::OpenOptions::new()
                            .append(true)
                            .open(&path)
                            .unwrap()
                            .write_all(&[0])
                            .unwrap();
                    }
                    "mtime" => {
                        let modified = std::fs::metadata(&path).unwrap().modified().unwrap()
                            + std::time::Duration::from_secs(2);
                        std::fs::OpenOptions::new()
                            .write(true)
                            .open(&path)
                            .unwrap()
                            .set_times(std::fs::FileTimes::new().set_modified(modified))
                            .unwrap();
                    }
                    "sidecar" => {
                        let mut journal = path.as_os_str().to_os_string();
                        journal.push("-journal");
                        std::fs::write(std::path::PathBuf::from(journal), []).unwrap();
                    }
                    _ => unreachable!(),
                }
                Ok(())
            });
            assert_eq!(result.skipped_stores, 1, "missed {change} change");
            assert_eq!(result.warnings.len(), 1);
            assert!(result.warnings[0].contains("changed during scan"));
        }
    }
    #[test]
    fn issue99_wal_removed_between_decision_and_connection_is_not_recreated() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE test(id); PRAGMA journal_mode=WAL")
            .unwrap();
        let wal = temp.path().join("store.db-wal");
        std::fs::write(&wal, []).unwrap();
        let mut result = SourceScan::default();
        let database = open_with(&path, &mut result, || {
            std::fs::remove_file(&wal).unwrap();
        });
        assert!(!wal.exists(), "reader recreated the removed WAL");
        assert!(
            !temp.path().join("store.db-shm").exists(),
            "discarded open created SHM"
        );
        assert!(database.is_none());
        assert_eq!(result.skipped_stores, 1);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains(path.to_str().unwrap()));
    }

    #[test]
    fn issue100_committed_rewrite_and_replacement_with_restored_mtime_are_rejected() {
        for replace in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("store.db");
            let writer = Connection::open(&path).unwrap();
            writer
                .execute_batch("CREATE TABLE test(title TEXT); INSERT INTO test VALUES ('OLD')")
                .unwrap();
            let before = std::fs::metadata(&path).unwrap();
            let mut result = SourceScan::default();
            scan(&path, &mut result, |connection, _| {
                let value: String =
                    connection.query_row("SELECT title FROM test", [], |r| r.get(0))?;
                assert_eq!(value, "OLD");
                if replace {
                    let replacement = temp.path().join("replacement.db");
                    Connection::open(&replacement)
                        .unwrap()
                        .execute_batch(
                            "CREATE TABLE test(title TEXT); INSERT INTO test VALUES ('NEW')",
                        )
                        .unwrap();
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(&replacement)
                        .unwrap()
                        .set_times(
                            std::fs::FileTimes::new().set_modified(before.modified().unwrap()),
                        )
                        .unwrap();
                    std::fs::rename(replacement, &path).unwrap();
                } else {
                    writer.execute("UPDATE test SET title='NEW'", [])?;
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(&path)
                        .unwrap()
                        .set_times(
                            std::fs::FileTimes::new().set_modified(before.modified().unwrap()),
                        )
                        .unwrap();
                }
                let after = std::fs::metadata(&path).unwrap();
                assert_eq!(after.len(), before.len());
                assert_eq!(after.modified().unwrap(), before.modified().unwrap());
                Ok(())
            });
            assert_eq!(
                result.skipped_stores, 1,
                "same-size/restored-mtime change escaped: replace={replace}"
            );
            assert_eq!(result.warnings.len(), 1);
        }
    }

    #[test]
    fn issue100_memory_journal_spill_with_restored_mtime_is_locked() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.db");
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF; PRAGMA cache_size=2; CREATE TABLE test(title TEXT); BEGIN; WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<3000) INSERT INTO test SELECT printf('%-1024s', 'COMMITTED') FROM n; COMMIT;").unwrap();
        let before = std::fs::metadata(&path).unwrap();
        let mut result = SourceScan::default();
        scan(&path, &mut result, |connection, result| {
            writer.execute_batch(
                "BEGIN IMMEDIATE; UPDATE test SET title=printf('%-1024s', 'UNCOMMITTED')",
            )?;
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(before.modified().unwrap()))
                .unwrap();
            let after = std::fs::metadata(&path).unwrap();
            assert_eq!(after.len(), before.len());
            assert_eq!(after.modified().unwrap(), before.modified().unwrap());
            assert!(!temp.path().join("store.db-journal").exists());
            let title: String = connection.query_row(
                "SELECT title FROM test WHERE title LIKE 'UNCOMMITTED%' LIMIT 1",
                [],
                |r| r.get(0),
            )?;
            assert!(title.starts_with("UNCOMMITTED"), "fixture did not spill");
            result.sessions.push(crate::model::Session {
                id: "grok:race".into(),
                agent: crate::model::Agent::Grok,
                cwd: temp.path().into(),
                project_root: temp.path().into(),
                title,
                started_at: 1793534400,
                last_activity_at: 1793534400,
                status: crate::model::Status::Done,
                last_user_text: String::new(),
                last_assistant_text: String::new(),
                transcript_path: path.clone(),
                resume_hint: String::new(),
                pane: None,
            });
            Ok(())
        });
        assert!(
            result.sessions.is_empty(),
            "uncommitted MEMORY-journal title escaped"
        );
        assert_eq!(result.skipped_stores, 1);
        assert_eq!(result.warnings.len(), 1);
        assert!(
            result.warnings[0].contains("locked"),
            "{:?}",
            result.warnings
        );
        writer.execute_batch("ROLLBACK").unwrap();
    }
    #[test]
    fn issue99_wal_removed_at_actual_vfs_open_cannot_be_created() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE test(id); PRAGMA journal_mode=WAL")
            .unwrap();
        let wal = temp.path().join("store.db-wal");
        std::fs::write(&wal, []).unwrap();
        let removed = std::rc::Rc::new(std::cell::Cell::new(false));
        let observed = removed.clone();
        let raced_wal = wal.clone();
        sqlite_vfs::BEFORE_SIDECAR_OPEN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                // Do not panic across the C ABI. Assert the removal afterwards.
                observed.set(std::fs::remove_file(raced_wal).is_ok());
            }));
        });
        let mut result = SourceScan::default();
        let database = open(&path, &mut result);
        sqlite_vfs::BEFORE_SIDECAR_OPEN.with(|hook| {
            hook.borrow_mut().take();
        });
        assert!(removed.get(), "fixture never reached the actual WAL open");
        assert!(database.is_none());
        assert!(
            !wal.exists(),
            "VFS recreated a WAL after the post-open check"
        );
        assert!(!temp.path().join("store.db-shm").exists());
        assert!(!temp.path().join("store.db-journal").exists());
        assert_eq!(result.skipped_stores, 1);
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn issue100_header_change_counters_are_read_from_the_file() {
        use std::io::{Seek, SeekFrom, Write};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE test(id)")
            .unwrap();
        for offset in [24, 92, 96] {
            let before = Snapshot::read(&path).unwrap();
            let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.seek(SeekFrom::Start(offset)).unwrap();
            file.write_all(&[0x7f, 0, 0, offset as u8]).unwrap();
            let after = Snapshot::read(&path).unwrap();
            assert_ne!(
                before.header, after.header,
                "missing header signal at {offset}"
            );
        }
    }
}
