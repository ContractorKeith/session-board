use super::SourceScan;
use rusqlite::{Connection, Error, ErrorCode, OpenFlags};
use std::{io, path::Path};

/// A no-lock read cannot create WAL sidecars. WAL databases fall back to the
/// checkpointed immutable snapshot; never copy or modify an agent's database.
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
    let attempt = |query: &str| -> rusqlite::Result<Connection> {
        let connection = Connection::open_with_flags(format!("{uri}?{query}"), flags)?;
        connection.busy_timeout(std::time::Duration::ZERO)?;
        connection.query_row("SELECT count(*) FROM sqlite_schema", [], |_| Ok(()))?;
        Ok(connection)
    };
    let connection = match attempt("mode=ro&nolock=1") {
        Err(Error::SqliteFailure(ref error, _))
            if matches!(
                error.code,
                ErrorCode::DatabaseBusy
                    | ErrorCode::DatabaseLocked
                    | ErrorCode::CannotOpen
                    | ErrorCode::ReadOnly
                    | ErrorCode::SystemIoFailure
            ) =>
        {
            result.warnings.push(format!(
                "{}: using immutable SQLite snapshot; uncheckpointed WAL changes are not visible",
                path.display()
            ));
            attempt("mode=ro&immutable=1")
        }
        other => other,
    };
    match connection {
        Ok(connection) => Some(connection),
        Err(error) => {
            failed(path, &error, result);
            None
        }
    }
}

pub(super) fn failed(path: &Path, error: &impl std::fmt::Display, result: &mut SourceScan) {
    result.skipped_stores += 1;
    result.warnings.push(format!("{}: {error}", path.display()));
}
