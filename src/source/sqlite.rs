use super::SourceScan;
use rusqlite::{Connection, OpenFlags};
use std::{
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(PartialEq, Eq)]
struct Snapshot {
    database: (u64, SystemTime),
    // WAL, SHM, rollback journal. Metadata also detects a previously empty
    // journal becoming active without changing the set of filenames.
    sidecars: [Option<(u64, SystemTime)>; 3],
}
impl Snapshot {
    fn read(path: &Path) -> io::Result<Self> {
        let stamp = |path: &Path| match std::fs::metadata(path) {
            Ok(meta) => Ok(Some((meta.len(), meta.modified()?))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        };
        let sidecar = |suffix: &str| {
            let mut name = path.as_os_str().to_os_string();
            name.push(suffix);
            stamp(Path::new(&name))
        };
        Ok(Self {
            database: stamp(path)?
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "database disappeared"))?,
            sidecars: [sidecar("-wal")?, sidecar("-shm")?, sidecar("-journal")?],
        })
    }
}
struct Database {
    connection: Connection,
    path: PathBuf,
    immutable_snapshot: Option<Snapshot>,
}

/// Existing WAL permits SQLite's SHM protocol; no WAL requires a checked immutable read.
fn open(path: &Path, result: &mut SourceScan) -> Option<Database> {
    if let Err(error) = crate::storage::regular_file(path) {
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
    // Without WAL, a nonempty rollback journal can contain spilled uncommitted
    // pages. Skip it rather than risk recovery or reading through its lock.
    if !wal && before.sidecars[2].is_some_and(|(size, _)| size > 0) {
        failed(
            path,
            &"database may be locked: nonempty rollback journal",
            result,
        );
        return None;
    }
    let query = if wal {
        "mode=ro"
    } else {
        "mode=ro&immutable=1"
    };
    let connection = (|| -> rusqlite::Result<Connection> {
        let connection = Connection::open_with_flags(format!("{uri}?{query}"), flags)?;
        connection.busy_timeout(std::time::Duration::from_millis(200))?;
        connection.query_row("SELECT count(*) FROM sqlite_schema", [], |_| Ok(()))?;
        Ok(connection)
    })();
    match connection {
        Ok(connection) => Some(Database {
            connection,
            path: absolute,
            immutable_snapshot: (!wal).then_some(before),
        }),
        Err(error) => {
            failed(path, &error, result);
            None
        }
    }
}

/// Keep validation around the complete scan so neither source can return a
/// partial immutable view after a writer changes the store under it.
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
    if let Some(before) = database.immutable_snapshot {
        match Snapshot::read(&database.path) {
            Ok(after) if before == after => (),
            Ok(_) => failed(
                path,
                &"database changed during immutable scan; discarded result",
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
        // Also exercise the plain mode=ro path, where immutable cannot mask a
        // mistakenly read-write connection flag.
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
    fn issue83_writer_spills_after_first_query_and_partial_titles_are_discarded() {
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
            assert!(result.warnings[0].contains("changed during immutable scan"));
        }
    }
}
