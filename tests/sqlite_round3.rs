#[allow(dead_code)]
mod common;
use rusqlite::Connection;
use session_board::{
    config::Config,
    source::{Source, grok::Grok},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
const NOW: i64 = 1793534400;
fn grok(path: &Path) -> session_board::source::SourceScan {
    Grok {
        path: path.into(),
        now: NOW,
    }
    .scan(&Config::default())
}
fn database(path: &Path) {
    Connection::open(path)
        .unwrap()
        .execute_batch(include_str!("../fixtures/grok/session_search.sql"))
        .unwrap();
}
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}
fn snapshot(path: &Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
    let mut files: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let e = entry.unwrap();
            (e.file_name(), fs::read(e.path()).unwrap())
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}
fn unchanged_except_shm(
    before: &[(std::ffi::OsString, Vec<u8>)],
    after: &[(std::ffi::OsString, Vec<u8>)],
) {
    // Compare every captured byte, allowing only SQLite's explicitly approved SHM protocol.
    let wal = before
        .iter()
        .any(|(n, _)| n.to_string_lossy().ends_with("-wal"))
        && after
            .iter()
            .any(|(n, _)| n.to_string_lossy().ends_with("-wal"));
    for (name, bytes) in before {
        let observed = after.iter().find(|(n, _)| n == name).unwrap();
        if name.to_string_lossy().ends_with("-shm") {
            assert!(wal);
            assert!(!bytes.is_empty());
            assert!(!observed.1.is_empty());
        } else {
            assert_eq!(&observed.1, bytes, "changed {:?}", name);
        }
    }
    for (name, bytes) in after {
        if !before.iter().any(|(n, _)| n == name) {
            assert!(
                wal && name.to_string_lossy().ends_with("-shm"),
                "new {:?}",
                name
            );
            assert!(!bytes.is_empty());
        }
    }
}
#[test]
fn issue84_stale_empty_journal_never_creates_wal_or_shm() {
    for wal_format in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.db");
        database(&path);
        if wal_format {
            Connection::open(&path)
                .unwrap()
                .execute_batch("PRAGMA journal_mode=WAL")
                .unwrap();
        }
        fs::write(sidecar(&path, "-journal"), []).unwrap();
        let before = snapshot(temp.path());
        let scan = grok(&path);
        if wal_format {
            assert!(scan.sessions.is_empty());
            assert_eq!(scan.skipped_stores, 1);
            assert_eq!(scan.warnings.len(), 1);
        } else {
            assert_eq!(scan.sessions.len(), 2);
            assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);
        }
        assert!(
            snapshot(temp.path()) == before,
            "stale journal caused database/sidecar mutation"
        );
    }
}

#[test]
fn issue84_orphan_wal_with_and_without_shm_keeps_committed_rows() {
    for keep_shm in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let orphan = temp.path().join("orphan");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&orphan).unwrap();
        let path = source.join("store.db");
        database(&path);
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; INSERT INTO session_docs VALUES ('orphan-row','/home/user/project',1793534340,'Committed orphan WAL','','')").unwrap();
        let destination = orphan.join("store.db");
        fs::copy(&path, &destination).unwrap();
        fs::copy(sidecar(&path, "-wal"), sidecar(&destination, "-wal")).unwrap();
        if keep_shm {
            fs::copy(sidecar(&path, "-shm"), sidecar(&destination, "-shm")).unwrap();
        }
        let before = snapshot(&orphan);
        let scan = grok(&destination);
        let after = snapshot(&orphan);
        assert!(
            scan.sessions.iter().any(|s| s.id == "grok:orphan-row"),
            "{:?}",
            scan.warnings
        );
        assert!(scan.warnings.is_empty());
        unchanged_except_shm(&before, &after);
        assert!(
            after
                .iter()
                .any(|(n, b)| n.to_string_lossy().ends_with("-shm") && !b.is_empty())
        );
        assert!(!sidecar(&destination, "-journal").exists());
    }
}
#[test]
fn issue85_null_and_non_numeric_timestamps_are_counted() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("store.db");
    database(&path);
    Connection::open(&path).unwrap().execute_batch("INSERT INTO session_docs VALUES ('null','/home/user/project',NULL,'','',''), ('string','/home/user/project','bad','','',''), ('blob','/home/user/project',x'0001','','',''), ('real','/home/user/project',1.5,'','','');").unwrap();
    let scan = grok(&path);
    assert_eq!(scan.skipped_sessions, 5);
    assert_eq!(scan.sessions.len(), 2);
    assert!(scan.warnings.is_empty());
}
#[test]
fn issue85_relative_database_override_resolves_before_doctor() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("opencode.db");
    Connection::open(&path)
        .unwrap()
        .execute_batch(include_str!("../fixtures/opencode/opencode.sql"))
        .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_sboard"));
    common::no_tmux(&mut command, temp.path());
    command
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .env("SBOARD_STATE_DIR", temp.path().join("state"))
        .env("SBOARD_CONFIG_DIR", temp.path().join("config"))
        .env("OPENCODE_DB", "opencode.db");
    for key in [
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "GROK_HOME",
        "XDG_DATA_HOME",
    ] {
        command.env_remove(key);
    }
    let output = command.args(["doctor", "--json"]).output().unwrap();
    assert!(output.status.success());
    let stores: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let stores = &stores["stores"];
    assert_eq!(
        stores[3]["path"],
        fs::canonicalize(&path).unwrap().to_str().unwrap()
    );
    assert_eq!(stores[3]["sessions"], 6);
}
#[test]
fn issue86_blob_tail_retains_embedded_nul_and_lossy_utf8() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("store.db");
    database(&path);
    let connection = Connection::open(&path).unwrap();
    for bytes in [
        b"before\0after".to_vec(),
        [b"before\0".as_slice(), &[0xff], b"after".as_slice()].concat(),
        b"invalid\xffafter".to_vec(),
        format!("{}THE END", "界".repeat(5000)).into_bytes(),
    ] {
        connection
            .execute(
                "UPDATE session_docs SET content=CAST(?1 AS TEXT) WHERE session_id='recent'",
                [&bytes],
            )
            .unwrap();
        let scan = grok(&path);
        let recent = scan
            .sessions
            .iter()
            .find(|s| s.id == "grok:recent")
            .unwrap();
        assert!(
            recent
                .last_assistant_text
                .ends_with(if bytes.ends_with(b"after") {
                    "after"
                } else {
                    "THE END"
                })
        );
        if bytes.contains(&0) {
            assert!(recent.last_assistant_text.contains('\0'));
        }
        if bytes.contains(&0xff) {
            assert!(recent.last_assistant_text.contains('\u{fffd}'));
        }
        assert!(recent.last_assistant_text.len() <= 8192);
    }
}
