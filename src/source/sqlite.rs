use super::SourceScan;
use rusqlite::{Connection, OpenFlags};
use std::{io, path::Path};

/// Live stores read committed WAL frames; stopped stores need no sidecars.
pub(super) fn open(path: &Path, result: &mut SourceScan) -> Option<Connection> {
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
    let sidecar = |suffix: &str| {
        let mut name = absolute.as_os_str().to_os_string();
        name.push(suffix);
        std::path::PathBuf::from(name).exists()
    };
    // A rollback journal also requires locking: immutable would read spilled,
    // uncommitted pages through the writer's exclusive lock.
    let live = sidecar("-wal") || sidecar("-shm") || sidecar("-journal");
    let query = if live {
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
        Ok(connection) => Some(connection),
        Err(error) => {
            failed(path, &error, result);
            None
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
            .execute("INSERT INTO test VALUES (2)", [])
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("attempt to write a readonly database"),
            "{error}"
        );
    }
}
