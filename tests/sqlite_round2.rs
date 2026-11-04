#[allow(
    dead_code,
    reason = "each integration suite uses a subset of common helpers"
)]
mod common;
use rusqlite::Connection;
use session_board::{
    config::Config,
    model::Status,
    source::{Source, grok::Grok, opencode::Opencode},
};
use std::{
    fs,
    io::{Seek, SeekFrom, Write},
    path::Path,
    process::Command,
};
const NOW: i64 = 1793534400;
fn database(path: &Path, sql: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    Connection::open(path).unwrap().execute_batch(sql).unwrap();
}
fn grok(path: &Path) -> Grok {
    Grok {
        path: path.into(),
        now: NOW,
    }
}
fn opencode(path: &Path) -> Opencode {
    Opencode {
        path: path.into(),
        now: NOW,
    }
}
fn names(path: &Path) -> Vec<std::ffi::OsString> {
    let mut entries: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    entries.sort();
    entries
}
fn command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sboard"));
    common::no_tmux(&mut command, home);
    command
        .env("HOME", home)
        .env("SBOARD_STATE_DIR", home.join("state"))
        .env("SBOARD_CONFIG_DIR", home.join("config"));
    for key in [
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "GROK_HOME",
        "OPENCODE_DB",
        "XDG_DATA_HOME",
    ] {
        command.env_remove(key);
    }
    command
}
#[test]
fn issue60_committed_wal_rows_are_visible_without_new_sidecars() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("grok.db");
    database(&path, include_str!("../fixtures/grok/session_search.sql"));
    let writer = Connection::open(&path).unwrap();
    writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; INSERT INTO session_docs VALUES ('live', '/home/user/project', 1793534340, 'Committed WAL row', '', '');").unwrap();
    let before = names(temp.path());
    let scan = grok(&path).scan(&Config::default());
    assert!(scan.sessions.iter().any(|s| s.id == "grok:live"));
    assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);
    assert_eq!(names(temp.path()), before);
}
#[test]
fn issue61_spilled_uncommitted_pages_are_never_listed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("grok.db");
    database(&path, include_str!("../fixtures/grok/session_search.sql"));
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch("PRAGMA cache_size=2; BEGIN IMMEDIATE;")
        .unwrap();
    for n in 0..3000 {
        writer.execute("INSERT INTO session_docs VALUES (?1, '/home/user/project', 1793534340, 'UNCOMMITTED', ?2, '')", rusqlite::params![format!("pending-{n}"), "x".repeat(2000)]).unwrap();
    }
    let scan = grok(&path).scan(&Config::default());
    assert!(
        scan.sessions.is_empty(),
        "listed {} uncommitted/partial rows",
        scan.sessions.len()
    );
    assert_eq!(scan.skipped_stores, 1);
    assert_eq!(scan.warnings.len(), 1);
    assert!(scan.warnings[0].contains(path.to_str().unwrap()));
    assert!(scan.warnings[0].contains("locked"));
    writer.execute_batch("ROLLBACK").unwrap();
}
#[test]
fn issue62_corrupt_leaf_fails_entire_store_and_type_errors_skip_rows() {
    for is_grok in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.db");
        let sql = if is_grok {
            include_str!("../fixtures/grok/session_search.sql")
        } else {
            include_str!("../fixtures/opencode/opencode.sql")
        };
        database(&path, sql);
        let connection = Connection::open(&path).unwrap();
        let table = if is_grok { "session_docs" } else { "session" };
        connection.execute_batch("BEGIN").unwrap();
        for n in 0..1000 {
            if is_grok {
                connection.execute("INSERT INTO session_docs VALUES (?1, '/home/user/project', 1793534340, ?2, '', '')", rusqlite::params![format!("row-{n:04}"), "x".repeat(500)]).unwrap();
            } else {
                connection.execute("INSERT INTO session VALUES (?1, NULL, '/home/user/project', ?2, 1793530000000, 1793534340000)", rusqlite::params![format!("row-{n:04}"), "x".repeat(500)]).unwrap();
            }
        }
        connection.execute_batch("COMMIT").unwrap();
        let page_size: u32 = connection
            .query_row("PRAGMA page_size", [], |r| r.get(0))
            .unwrap();
        // Locate a middle table leaf using SQLite's dbstat virtual table.
        let page: u32 = connection.query_row("SELECT pageno FROM dbstat WHERE name=?1 AND pagetype='leaf' ORDER BY pageno LIMIT 1 OFFSET 10", [table], |r| r.get(0)).unwrap();
        drop(connection);
        let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(u64::from(page - 1) * u64::from(page_size)))
            .unwrap();
        file.write_all(&[0xff]).unwrap();
        drop(file);
        let scan = if is_grok {
            grok(&path).scan(&Config::default())
        } else {
            opencode(&path).scan(&Config::default())
        };
        assert_eq!(scan.skipped_stores, 1, "{table}: {:?}", scan.warnings);
        assert!(scan.sessions.is_empty(), "{table}: partial store escaped");
        assert_eq!(scan.warnings.len(), 1);
        assert!(scan.warnings[0].contains(path.to_str().unwrap()));
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("types.db");
    database(&path, include_str!("../fixtures/grok/session_search.sql"));
    Connection::open(&path).unwrap().execute_batch("INSERT INTO session_docs VALUES ('bad-type', '/home/user/project', 1793534340, x'FF', '', '');").unwrap();
    let scan = grok(&path).scan(&Config::default());
    assert_eq!(scan.skipped_stores, 0);
    assert_eq!(scan.skipped_sessions, 2);
    assert_eq!(scan.sessions.len(), 2);
}
#[test]
fn issue64_opencode_markers_are_typed_and_aborts_are_idle() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("opencode.db");
    database(&path, include_str!("../fixtures/opencode/opencode.sql"));
    let connection = Connection::open(&path).unwrap();
    for (data, expected, malformed) in [
        (
            r#"{"role":"assistant","finish":"tool-calls","time":{"completed":1793534340000}}"#,
            Status::Working,
            0,
        ),
        (
            r#"{"role":"assistant","finish":"unknown","time":{"completed":1793534340000}}"#,
            Status::Working,
            0,
        ),
        (
            r#"{"role":"assistant","finish":"","time":{"completed":1793534340000}}"#,
            Status::Working,
            0,
        ),
        (
            r#"{"role":"assistant","time":{"completed":1793534340000}}"#,
            Status::Done,
            0,
        ),
        (r#"{"role":"assistant","finish":"stop"}"#, Status::Done, 0),
        (
            r#"{"role":"assistant","error":{"name":"MessageAbortedError"},"time":{"completed":1793534340000}}"#,
            Status::Idle,
            0,
        ),
        (
            r#"{"role":"assistant","error":{"name":"APIError"}}"#,
            Status::Error,
            0,
        ),
        (r#"{"role":"assistant","error":false}"#, Status::Working, 1),
        (
            r#"{"role":"assistant","time":{"completed":false}}"#,
            Status::Working,
            1,
        ),
        (
            r#"{"role":"assistant","time":{"completed":"yesterday"}}"#,
            Status::Working,
            1,
        ),
        (r#"{"role":"assistant","finish":true}"#, Status::Working, 1),
    ] {
        connection
            .execute(
                "UPDATE message SET data=?1 WHERE id='done-assistant'",
                [data],
            )
            .unwrap();
        let scan = opencode(&path).scan(&Config::default());
        assert_eq!(
            scan.sessions
                .iter()
                .find(|s| s.id == "opencode:done")
                .unwrap()
                .status,
            expected,
            "{data}"
        );
        assert_eq!(scan.skipped_lines, malformed, "{data}");
    }
}
#[test]
fn issue65_overrides_match_doctor_and_registry() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let grok_home = home.join("custom-grok");
    let data_home = home.join("custom-data");
    let db = home.join("channel/opencode-beta.db");
    database(
        &grok_home.join("sessions/session_search.sqlite"),
        include_str!("../fixtures/grok/session_search.sql"),
    );
    database(
        &data_home.join("opencode/opencode.db"),
        include_str!("../fixtures/opencode/opencode.sql"),
    );
    database(&db, include_str!("../fixtures/opencode/opencode.sql"));
    for explicit in [false, true] {
        let mut cmd = command(home);
        cmd.env("GROK_HOME", &grok_home)
            .env("XDG_DATA_HOME", &data_home);
        if explicit {
            cmd.env("OPENCODE_DB", &db);
        }
        let output = cmd.args(["doctor", "--json"]).output().unwrap();
        assert!(output.status.success());
        let stores: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let stores = &stores["stores"];
        assert_eq!(stores[2]["sessions"], 2);
        assert_eq!(stores[3]["sessions"], 6);
        assert_eq!(
            stores[2]["path"],
            grok_home
                .join("sessions/session_search.sqlite")
                .to_str()
                .unwrap()
        );
        assert_eq!(
            stores[3]["path"],
            if explicit {
                db.clone()
            } else {
                data_home.join("opencode/opencode.db")
            }
            .to_str()
            .unwrap()
        );
    }
}
#[test]
fn issue66_ids_and_nul_cwds_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    for is_grok in [true, false] {
        let path = temp
            .path()
            .join(if is_grok { "grok.db" } else { "opencode.db" });
        database(
            &path,
            if is_grok {
                include_str!("../fixtures/grok/session_search.sql")
            } else {
                include_str!("../fixtures/opencode/opencode.sql")
            },
        );
        let connection = Connection::open(&path).unwrap();
        let insert = if is_grok {
            "INSERT INTO session_docs VALUES (?1, ?2, 1793534340, '', '', '')"
        } else {
            "INSERT INTO session VALUES (?1, NULL, ?2, '', 1793530000000, 1793534340000)"
        };
        for id in [
            "",
            "two words",
            "-rf",
            "x;touch_PWNED",
            "a\nline",
            "a\0b",
            "é",
        ] {
            connection
                .execute(insert, [id, "/home/user/project"])
                .unwrap();
        }
        connection
            .execute(insert, ["nul-cwd", "/home/user/proj\0suffix"])
            .unwrap();
        connection
            .execute(insert, ["valid._:-ID9", "/home/user/project"])
            .unwrap();
        let scan = if is_grok {
            grok(&path).scan(&Config::default())
        } else {
            opencode(&path).scan(&Config::default())
        };
        assert_eq!(scan.skipped_sessions, 9); // Eight invalid rows plus the base relative cwd.
        let agent = if is_grok { "grok" } else { "opencode" };
        let s = scan
            .sessions
            .iter()
            .find(|s| s.id == format!("{agent}:valid._:-ID9"))
            .unwrap();
        assert_eq!(
            s.resume_hint,
            format!(
                "{agent} --{}=valid._:-ID9",
                if is_grok { "resume" } else { "session" }
            )
        );
    }
}
#[test]
fn issue67_grok_titles_fields_future_clamp_and_millisecond_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("grok.db");
    database(&path, include_str!("../fixtures/grok/session_search.sql"));
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("UPDATE session_docs SET title='', content=char(10)||'  '||char(10)||'First nonblank'||char(10)||'Tail' WHERE session_id='recent';
        INSERT INTO session_docs VALUES ('empty', '/home/user/project', 1793534340, '', '', '');
        INSERT INTO session_docs VALUES ('future', '/home/user/project', 9223372036854775807, '', '', '');
        INSERT INTO session_docs VALUES ('boundary', '/home/user/project', 1000000000000, 'Old', '', '');").unwrap();
    let scan = grok(&path).scan(&Config::default());
    let recent = scan
        .sessions
        .iter()
        .find(|s| s.id == "grok:recent")
        .unwrap();
    assert_eq!(recent.title, "First nonblank");
    assert!(recent.last_user_text.is_empty());
    assert!(recent.last_assistant_text.ends_with("Tail"));
    assert_eq!(
        scan.sessions
            .iter()
            .find(|s| s.id == "grok:empty")
            .unwrap()
            .title,
        "empty"
    );
    assert_eq!(
        scan.sessions
            .iter()
            .find(|s| s.id == "grok:future")
            .unwrap()
            .last_activity_at,
        NOW
    );
    assert_eq!(scan.warnings.len(), 1);
    assert!(scan.warnings[0].contains("future"));
    assert!(!scan.sessions.iter().any(|s| s.id == "grok:boundary"));
    let all = grok(&path).scan(&Config {
        window_days: u32::MAX,
        ..Default::default()
    });
    assert_eq!(
        all.sessions
            .iter()
            .find(|s| s.id == "grok:boundary")
            .unwrap()
            .last_activity_at,
        1_000_000_000
    );
}
#[test]
#[ignore = "explicit RSS measurement on a 100,000-row history"]
fn issue67_large_store_memory_probe() {
    // Run explicitly for RSS measurements; default test keeps the costly setup out of CI.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("large.db");
    database(&path, include_str!("../fixtures/grok/session_search.sql"));
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("BEGIN; WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000)
        INSERT INTO session_docs SELECT 'old-'||x, '/home/user/project', 1, '', replace(hex(zeroblob(4096)), '00', 'xx'), '' FROM n; COMMIT;").unwrap();
    drop(connection);
    assert_eq!(grok(&path).scan(&Config::default()).sessions.len(), 2);
}
#[test]
fn selected_store_diagnostic_is_a_list_of_only_selected_paths() {
    let temp = tempfile::tempdir().unwrap();
    let output = command(temp.path())
        .args(["list", "--agent", "grok", "--json"])
        .output()
        .unwrap();
    let warning = String::from_utf8(output.stderr).unwrap();
    assert!(warning.contains("grok at "));
    assert!(!warning.contains("codex"));
    assert!(!warning.contains("opencode"));
    assert!(!warning.contains("claude_code"));
}

#[test]
fn issue68_no_transient_sidecars_during_scan() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("store.db");
    database(&path, include_str!("../fixtures/grok/session_search.sql"));
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("BEGIN; WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<5000)
        INSERT INTO session_docs SELECT 'row-'||x, '/home/user/project', 1793534340, 'Title', replace(hex(zeroblob(256)), '00', 'xx'), '' FROM n; COMMIT;").unwrap();
    drop(connection);
    let expected = names(temp.path());
    let (ready, started) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let polling = stop.clone();
    let directory = temp.path().to_path_buf();
    let before = expected.clone();
    let observer = std::thread::spawn(move || {
        ready.send(()).unwrap();
        let mut polls = 0;
        while !polling.load(Ordering::Relaxed) {
            assert_eq!(names(&directory), before, "sidecar appeared during scan");
            polls += 1;
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        polls
    });
    started.recv().unwrap();
    let scan = grok(&path).scan(&Config::default());
    stop.store(true, Ordering::Relaxed);
    assert!(observer.join().unwrap() > 0);
    assert_eq!(scan.sessions.len(), 5002);
    assert_eq!(names(temp.path()), expected);
}

#[test]
fn issue67_window_filters_content_before_row_conversion() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("grok.db");
    database(&path, include_str!("../fixtures/grok/session_search.sql"));
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "INSERT INTO session_docs VALUES ('ancient', '/home/user/project', 1, '', x'FF', '');",
        )
        .unwrap();
    let scan = grok(&path).scan(&Config::default());
    assert_eq!(scan.skipped_sessions, 1); // Only the relative-cwd row; old BLOB isn't decoded.
    assert!(scan.warnings.is_empty());
}
