#[allow(dead_code)]
mod common;
use rusqlite::Connection;
use session_board::{
    config::Config,
    model::Status,
    source::{Source, grok::Grok, opencode::Opencode},
};
use std::{fs, path::Path, process::Command};
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
fn no_sidecars(path: &Path) {
    for entry in fs::read_dir(path.parent().unwrap()).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            !["-wal", "-shm", "-journal"]
                .iter()
                .any(|suffix| name.ends_with(suffix)),
            "unexpected sidecar: {name}"
        );
    }
}
fn command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sboard"));
    common::no_tmux(&mut command, home);
    command
        .env("HOME", home)
        .env("SBOARD_STATE_DIR", home.join("state"))
        .env("SBOARD_CONFIG_DIR", home.join("config"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("GROK_HOME")
        .env_remove("OPENCODE_DB");
    command
}
#[test]
fn grok_fixture_rules_and_bounded_utf8_content() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("space ?#%/session.sqlite");
    database(&path, include_str!("../fixtures/grok/session_search.sql"));
    let result = grok(&path).scan(&Config::default());
    assert_eq!(result.sessions.len(), 2);
    assert_eq!(result.skipped_sessions, 1);
    assert!(result.warnings.is_empty());
    let recent = result
        .sessions
        .iter()
        .find(|s| s.id == "grok:recent")
        .unwrap();
    assert_eq!(recent.cwd, Path::new("/home/user/project"));
    assert_eq!(recent.title, "Title placeholder");
    assert_eq!(recent.last_activity_at, NOW - 60);
    assert_eq!(recent.started_at, NOW - 60);
    assert_eq!(recent.status, Status::Done); // A question cannot imply waiting in this flat index.
    assert_eq!(recent.resume_hint, "grok --resume=recent");
    assert_eq!(recent.transcript_path, path);
    let old = result.sessions.iter().find(|s| s.id == "grok:old").unwrap();
    assert_eq!(old.status, Status::Idle);
    assert_eq!(old.title, "First line placeholder");
    let text = format!("{}END", "界😀".repeat(3000));
    Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE session_docs SET content=?1 WHERE session_id='recent'",
            [&text],
        )
        .unwrap();
    let result = grok(&path).scan(&Config::default());
    let recent = result
        .sessions
        .iter()
        .find(|s| s.id == "grok:recent")
        .unwrap();
    assert!(recent.last_user_text.is_empty());
    assert!(recent.last_assistant_text.len() <= 8192);
    assert!(recent.last_assistant_text.ends_with("END"));
    assert!(
        grok(&path)
            .scan(&Config {
                window_days: 0,
                ..Default::default()
            })
            .sessions
            .is_empty()
    );
    no_sidecars(&path);
}
#[test]
fn opencode_fixture_rules_and_texts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("opencode.db");
    database(&path, include_str!("../fixtures/opencode/opencode.sql"));
    let result = opencode(&path).scan(&Config::default());
    assert_eq!(result.sessions.len(), 6);
    assert_eq!(result.skipped_sessions, 1);
    assert_eq!(result.status_reasons.len(), 1);
    for (id, status) in [
        ("working", Status::Working),
        ("stuck", Status::Error),
        ("waiting", Status::Waiting),
        ("done", Status::Done),
        ("idle", Status::Idle),
        ("error", Status::Error),
    ] {
        let session = result
            .sessions
            .iter()
            .find(|s| s.id == format!("opencode:{id}"))
            .unwrap();
        assert_eq!(session.status, status, "{id}");
        assert_eq!(session.last_user_text, "User placeholder");
        assert_eq!(
            session.last_assistant_text,
            if id == "waiting" {
                "Decision placeholder?"
            } else {
                "Assistant placeholder"
            }
        );
        assert_eq!(session.started_at, 1793530000);
        assert_eq!(session.resume_hint, format!("opencode --session={id}"));
        assert_eq!(session.cwd, Path::new("/home/user/project"));
        if id == "done" {
            assert_eq!(session.title, "User placeholder");
        }
    }
    no_sidecars(&path);
}
#[test]
fn sqlite_missing_special_and_corrupt_stores_are_counted() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("store");
    let config = Config::default();
    let check = |warnings| {
        for source in [&grok(&path) as &dyn Source, &opencode(&path)] {
            let scan = source.scan(&config);
            assert!(scan.sessions.is_empty());
            assert_eq!(scan.skipped_stores, 1);
            assert_eq!(!scan.warnings.is_empty(), warnings);
        }
    };
    check(false);
    fs::create_dir(&path).unwrap();
    check(true);
    fs::remove_dir(&path).unwrap();
    #[cfg(unix)]
    {
        assert!(
            Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        check(true);
        fs::remove_file(&path).unwrap();
    }
    fs::write(&path, "corrupt SQLite").unwrap();
    check(true);
    fs::remove_file(&path).unwrap();
    database(&path, "CREATE TABLE unexpected(id);");
    check(true);
    no_sidecars(&path);
}
#[test]
fn sqlite_wal_snapshot_never_creates_sidecars() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("store.db");
    database(&path, include_str!("../fixtures/grok/session_search.sql"));
    // Persist WAL mode in the header, with all data checkpointed and sidecars removed.
    Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA journal_mode=WAL;")
        .unwrap();
    no_sidecars(&path);
    let scan = grok(&path).scan(&Config::default());
    assert_eq!(scan.sessions.len(), 2);
    assert!(scan.warnings.is_empty());
    no_sidecars(&path);
}
#[test]
fn doctor_and_agent_filters_on_isolated_home() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(".grok/sessions/session_search.sqlite");
    database(&path, include_str!("../fixtures/grok/session_search.sql"));
    let output = command(temp.path())
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stores: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let stores = &stores["stores"];
    assert_eq!(stores[2]["agent"], "grok");
    assert_eq!(stores[2]["exists"], true);
    assert_eq!(stores[2]["sessions"], 2);
    assert_eq!(stores[2]["skipped_sessions"], 1);
    assert_eq!(stores[3]["agent"], "opencode");
    assert_eq!(stores[3]["exists"], false);
    assert_eq!(stores[3]["skipped_stores"], 1);
    for (agent, code, count) in [("grok", 0, 2), ("opencode", 2, 0)] {
        let output = command(temp.path())
            .args(["list", "--agent", agent, "--all", "--json"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(code));
        let projects: Vec<session_board::model::Project> =
            serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            projects.iter().map(|p| p.sessions.len()).sum::<usize>(),
            count
        );
    }
    database(
        &temp.path().join(".local/share/opencode/opencode.db"),
        include_str!("../fixtures/opencode/opencode.sql"),
    );
    let output = command(temp.path())
        .args(["list", "--agent", "opencode", "--all", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let projects: Vec<session_board::model::Project> =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(projects[0].sessions.len(), 6);
    fs::create_dir(temp.path().join("config")).unwrap();
    fs::write(
        temp.path().join("config/config.toml"),
        "[agents]\ngrok=false\nopencode=false\n",
    )
    .unwrap();
    let output = command(temp.path())
        .args(["list", "--all", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!([])
    );
    no_sidecars(&path);
}

#[test]
fn opencode_completion_recovery_malformed_rows_and_preview_bounds() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("opencode.db");
    database(&path, include_str!("../fixtures/opencode/opencode.sql"));
    let connection = Connection::open(&path).unwrap();
    // Complete a tool; an open assistant still generates until its completion marker.
    connection
        .execute(
            "UPDATE part SET data=?1 WHERE id='working-tool'",
            [r#"{"type":"tool","state":{"status":"completed"}}"#],
        )
        .unwrap();
    let status = || {
        opencode(&path)
            .scan(&Config::default())
            .sessions
            .into_iter()
            .find(|s| s.id == "opencode:working")
            .unwrap()
            .status
    };
    assert_eq!(status(), Status::Working);
    connection
        .execute(
            "UPDATE message SET data=?1 WHERE id='working-assistant'",
            [r#"{"role":"assistant","finish":"tool-calls"}"#],
        )
        .unwrap();
    assert_eq!(status(), Status::Working);
    connection
        .execute(
            "UPDATE message SET data=?1 WHERE id='working-assistant'",
            [r#"{"role":"assistant","time":{"completed":1793534340000}}"#],
        )
        .unwrap();
    assert_eq!(status(), Status::Done);
    connection
        .execute(
            "UPDATE part SET data=?1 WHERE id='working-tool'",
            [r#"{"type":"tool","state":{"status":"error"}}"#],
        )
        .unwrap();
    assert_eq!(status(), Status::Error);
    connection.execute_batch("INSERT INTO message VALUES ('recovered', 'working', 1793534340001, 1793534340001, '{\"role\":\"assistant\",\"finish\":\"stop\"}');
        INSERT INTO part VALUES ('recovered-text', 'recovered', 'working', 1793534340001, 1793534340001, '{\"type\":\"text\",\"text\":\"Recovered\"}');
        INSERT INTO message VALUES ('thinking-only', 'waiting', 1793534340001, 1793534340001, '{\"role\":\"assistant\",\"finish\":\"stop\"}');
        INSERT INTO part VALUES ('bad', 'done-assistant', 'done', 1793534340001, 1793534340001, 'malformed');
        INSERT INTO message VALUES ('bad', 'done', 1793534340001, 1793534340001, 'malformed');
        INSERT INTO session VALUES ('invalid-type', NULL, '/home/user/project', 'Bad', 'invalid', 1793534340000);").unwrap();
    assert_eq!(status(), Status::Done);
    let text = format!("{}END?", "界😀".repeat(3000));
    for id in ["done-prompt", "done-answer"] {
        connection
            .execute(
                "UPDATE part SET data=?1 WHERE id=?2",
                [
                    serde_json::json!({"type":"text","text":text}).to_string(),
                    id.into(),
                ],
            )
            .unwrap();
    }
    let scan = opencode(&path).scan(&Config::default());
    assert_eq!(scan.skipped_lines, 2);
    assert_eq!(scan.skipped_sessions, 2);
    assert_eq!(
        scan.sessions
            .iter()
            .find(|s| s.id == "opencode:waiting")
            .unwrap()
            .status,
        Status::Done
    );
    let done = scan
        .sessions
        .iter()
        .find(|s| s.id == "opencode:done")
        .unwrap();
    assert_eq!(done.status, Status::Waiting);
    for text in [&done.last_user_text, &done.last_assistant_text] {
        assert!(text.len() <= 8192);
        assert!(text.ends_with("END?"));
    }
    no_sidecars(&path);
}

#[test]
fn active_wal_database_and_frames_are_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("store.db");
    database(&path, include_str!("../fixtures/grok/session_search.sql"));
    let writer = Connection::open(&path).unwrap();
    writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; UPDATE session_docs SET title='Uncheckpointed' WHERE session_id='recent';").unwrap();
    let snapshot = || {
        let mut files: Vec<_> = fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), fs::read(entry.path()).unwrap())
            })
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files
    };
    let before = snapshot();
    let result = grok(&path).scan(&Config::default());
    assert_eq!(result.sessions.len(), 2);
    assert_eq!(
        result
            .sessions
            .iter()
            .find(|s| s.id == "grok:recent")
            .unwrap()
            .title,
        "Uncheckpointed"
    );
    assert!(result.warnings.is_empty());
    let after = snapshot();
    assert_eq!(after.len(), before.len());
    for ((name, bytes), (after_name, after_bytes)) in before.iter().zip(&after) {
        assert_eq!(name, after_name);
        if name.to_string_lossy().ends_with("-shm") {
            // Full SHM bytes are captured. SQLite may rewrite them only as
            // part of the WAL protocol while an existing WAL is present.
            assert!(
                before
                    .iter()
                    .any(|(n, _)| n.to_string_lossy().ends_with("-wal"))
            );
            assert!(
                after
                    .iter()
                    .any(|(n, _)| n.to_string_lossy().ends_with("-wal"))
            );
            assert!(!bytes.is_empty() && !after_bytes.is_empty());
        } else {
            assert!(bytes == after_bytes, "SQLite changed {:?}", name);
        }
    }
    drop(writer);
    no_sidecars(&path);
}
