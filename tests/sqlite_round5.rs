#[allow(
    dead_code,
    reason = "each integration suite uses a subset of common helpers"
)]
mod common;
use rusqlite::Connection;
use session_board::{
    config::Config,
    source::{Source, grok::Grok},
};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
const NOW: i64 = 1793534400;
fn database(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    Connection::open(path)
        .unwrap()
        .execute_batch(include_str!("../fixtures/grok/session_search.sql"))
        .unwrap();
}
fn command(home: &Path, control: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sboard"));
    common::no_tmux(&mut command, control);
    command
        .env("HOME", home)
        .env("GROK_HOME", home.join(".grok"))
        .env("SBOARD_STATE_DIR", control.join("state"))
        .env("SBOARD_CONFIG_DIR", control.join("config"));
    for key in [
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "OPENCODE_DB",
        "XDG_DATA_HOME",
    ] {
        command.env_remove(key);
    }
    command
}
#[test]
fn issue107_memory_writer_spilled_before_first_snapshot_is_skipped() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(".grok/sessions/session_search.sqlite");
    database(&path);
    let writer = Connection::open(&path).unwrap();
    writer.execute_batch("PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF; PRAGMA cache_size=2; BEGIN IMMEDIATE;
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<2999)
        INSERT INTO session_docs SELECT 'pending-'||x,'/home/user/project',1793534340,'UNCOMMITTED',replace(hex(zeroblob(1024)),'00','xx'),'' FROM n;").unwrap();
    assert!(
        !path
            .with_file_name("session_search.sqlite-journal")
            .exists()
    );
    // The lock is verified in a separate process, matching a real agent writer.
    let probe=Command::new("python3").args(["-c", "import sqlite3,sys; c=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro', uri=True, timeout=0)\ntry: c.execute('SELECT count(*) FROM session_docs'); raise AssertionError('writer did not spill')\nexcept sqlite3.OperationalError as e: assert 'locked' in str(e),str(e)"]).arg(&path).output().unwrap();
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let output = command(temp.path(), &temp.path().join("doctor-control"))
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let store = &report["stores"][2];
    assert_eq!(store["sessions"], 0);
    assert_eq!(store["skipped_stores"], 1);
    let warnings = store["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    let warning = warnings[0].as_str().unwrap();
    assert!(
        warning.contains(path.to_str().unwrap()) && warning.contains("locked"),
        "{warning}"
    );
    let output = command(temp.path(), &temp.path().join("list-control"))
        .args(["list", "--agent", "grok", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!([])
    );
    let warning = String::from_utf8(output.stderr).unwrap();
    assert_eq!(warning.lines().count(), 1);
    assert!(warning.contains("locked"));
    writer.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        Grok { path, now: NOW }
            .scan(&Config::default())
            .sessions
            .len(),
        2
    );
}
#[cfg(unix)]
#[test]
fn issue108_sidecar_special_files_are_skipped_without_hanging() {
    use std::os::unix::fs::symlink;
    for suffix in ["-wal", "-shm", "-journal"] {
        for kind in ["fifo", "directory", "socket", "dangling", "symlink"] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path();
            let path = home.join(".grok/sessions/session_search.sqlite");
            database(&path);
            Connection::open(&path)
                .unwrap()
                .execute_batch("PRAGMA journal_mode=WAL")
                .unwrap();
            let sidecar = path.with_file_name(format!("session_search.sqlite{suffix}"));
            match kind {
                "fifo" => {
                    assert!(
                        Command::new("mkfifo")
                            .arg(&sidecar)
                            .status()
                            .unwrap()
                            .success()
                    );
                }
                "directory" => {
                    fs::create_dir(&sidecar).unwrap();
                }
                "socket" => {
                    assert!(Command::new("python3").args(["-c", "import socket,sys; s=socket.socket(socket.AF_UNIX); s.bind(sys.argv[1])"])
                        .arg(sidecar.file_name().unwrap()).current_dir(sidecar.parent().unwrap()).status().unwrap().success());
                }
                "dangling" => {
                    symlink(home.join("missing"), &sidecar).unwrap();
                }
                "symlink" => {
                    let target = home.join("target");
                    fs::write(&target, []).unwrap();
                    symlink(target, &sidecar).unwrap();
                }
                _ => unreachable!(),
            };
            let mut child = command(home, &home.join("control"))
                .args(["list", "--agent", "grok", "--json"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() > deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("{kind} {suffix} blocked SQLite beyond timeout");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "{kind} {suffix}");
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
                serde_json::json!([])
            );
            let warning = String::from_utf8(output.stderr).unwrap();
            assert_eq!(warning.lines().count(), 1, "{kind} {suffix}: {warning}");
            assert!(
                warning.contains(sidecar.to_str().unwrap())
                    && warning.contains("not a regular file"),
                "{kind} {suffix}: {warning}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn issue108_database_symlinks_and_special_files_are_rejected() {
    use std::os::unix::fs::symlink;
    for kind in ["fifo", "directory", "socket", "dangling", "symlink"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("db");
        match kind {
            "fifo" => assert!(
                Command::new("mkfifo")
                    .arg(&path)
                    .status()
                    .unwrap()
                    .success()
            ),
            "directory" => fs::create_dir(&path).unwrap(),
            "socket" => assert!(
                Command::new("python3")
                    .args([
                        "-c",
                        "import socket; s=socket.socket(socket.AF_UNIX); s.bind('db')"
                    ])
                    .current_dir(temp.path())
                    .status()
                    .unwrap()
                    .success()
            ),
            "dangling" => symlink(temp.path().join("missing"), &path).unwrap(),
            "symlink" => {
                let target = temp.path().join("target");
                database(&target);
                symlink(target, &path).unwrap();
            }
            _ => unreachable!(),
        }
        let scan = Grok {
            path: path.clone(),
            now: NOW,
        }
        .scan(&Config::default());
        assert!(scan.sessions.is_empty());
        assert_eq!(scan.skipped_stores, 1);
        assert_eq!(scan.warnings.len(), 1);
        assert!(
            scan.warnings[0].contains("not a regular file")
                && scan.warnings[0].contains(path.to_str().unwrap()),
            "{kind}: {:?}",
            scan.warnings
        );
    }
    let scan = Grok {
        path: "/dev/null".into(),
        now: NOW,
    }
    .scan(&Config::default());
    assert_eq!(scan.skipped_stores, 1);
    assert_eq!(scan.warnings.len(), 1);
    assert!(scan.warnings[0].contains("not a regular file"));
}

#[test]
#[ignore = "explicit 20-concurrent-lists WAL writer stress check"]
fn twenty_concurrent_lists_during_wal_commits_create_no_new_sidecar() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let store = home.join(".grok/sessions");
    let path = store.join("session_search.sqlite");
    database(&path);
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0")
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    writer
        .execute(
            "UPDATE session_docs SET title='commit-0', updated_at=?1",
            [now],
        )
        .unwrap();
    writer.busy_timeout(Duration::from_millis(200)).unwrap();
    let names = || {
        let mut entries: Vec<_> = fs::read_dir(&store)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        entries.sort();
        entries
    };
    let before = names();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let (ready, started) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut commits = 0;
        while !stopped.load(Ordering::Relaxed) {
            commits += 1;
            writer
                .execute(
                    "UPDATE session_docs SET title=?1",
                    [format!("commit-{commits}")],
                )
                .unwrap();
            if commits == 1 {
                ready.send(()).unwrap();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        (writer, commits)
    });
    started.recv_timeout(Duration::from_secs(3)).unwrap();
    let mut children: Vec<_> = (0..20)
        .map(|i| {
            command(home, &home.join(format!("control-{i}")))
                .args(["list", "--agent", "grok", "--json"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut polls = 0;
    let mut problem = None;
    loop {
        if names() != before {
            problem = Some("new sidecar during concurrent reads");
            break;
        }
        polls += 1;
        if children
            .iter_mut()
            .all(|child| child.try_wait().unwrap().is_some())
        {
            break;
        }
        if Instant::now() > deadline {
            problem = Some("concurrent reader timed out");
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    stop.store(true, Ordering::Relaxed);
    let (_writer, commits) = worker.join().unwrap();
    if let Some(problem) = problem {
        for child in &mut children {
            let _ = child.kill();
            let _ = child.wait();
        }
        panic!("{problem}");
    }
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let projects: Vec<session_board::model::Project> =
            serde_json::from_slice(&output.stdout).unwrap();
        let sessions: Vec<_> = projects.iter().flat_map(|p| &p.sessions).collect();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().all(|s| s.title.starts_with("commit-")));
    }
    assert_eq!(names(), before);
    assert!(commits > 1);
    println!(
        "20 concurrent lists passed during {commits} WAL commits; {polls} directory polls; no new sidecar"
    );
}
