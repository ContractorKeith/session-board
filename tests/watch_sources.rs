#[path = "../src/watch.rs"]
mod watch;
use std::{fs, path::Path, process::Command, time::Duration};

fn probe(mode: &str) {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "watch_round2_probe"])
        .env("SBOARD_WATCH_CASE", mode)
        .env("HOME", temp.path())
        .env("SBOARD_STATE_DIR", temp.path().join("state"))
        .env("SBOARD_CONFIG_DIR", temp.path().join("config"))
        .env("CLAUDE_CONFIG_DIR", temp.path().join("claude"))
        .env("CODEX_HOME", temp.path().join("codex"))
        .env("GROK_HOME", temp.path().join("custom-grok"))
        .env(
            "OPENCODE_DB",
            temp.path().join("custom-code/opencode-beta.db"),
        )
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn issue63_scans_watch_grok_and_use_override() {
    probe("grok");
}
#[test]
fn issue63_scans_watch_opencode_and_use_override() {
    probe("opencode");
}
#[test]
fn issue63_missing_roots_are_named_and_reattach() {
    probe("missing");
}
#[test]
#[ignore = "isolated HOME and overrides supplied by parent tests"]
fn watch_round2_probe() {
    let mode = std::env::var("SBOARD_WATCH_CASE").unwrap();
    let home = std::env::var_os("HOME").unwrap();
    let home = Path::new(&home);
    fs::create_dir_all(home.join("claude/projects")).unwrap();
    fs::create_dir_all(home.join("codex/sessions")).unwrap();
    let grok = home.join("custom-grok/sessions/session_search.sqlite");
    let code = home.join("custom-code/opencode-beta.db");
    if mode != "missing" {
        fs::create_dir_all(grok.parent().unwrap()).unwrap();
        fs::create_dir_all(code.parent().unwrap()).unwrap();
        rusqlite::Connection::open(&grok)
            .unwrap()
            .execute_batch(include_str!("../fixtures/grok/session_search.sql"))
            .unwrap();
        rusqlite::Connection::open(&code)
            .unwrap()
            .execute_batch(include_str!("../fixtures/opencode/opencode.sql"))
            .unwrap();
    }
    let scans = watch::Scans::start().unwrap();
    scans
        .results
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    if mode == "missing" {
        let warnings = scans.status.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("unwatched:") && w.contains("grok") && w.contains("opencode")),
            "{warnings:?}"
        );
        fs::create_dir_all(grok.parent().unwrap()).unwrap();
        fs::create_dir_all(code.parent().unwrap()).unwrap();
        scans
            .results
            .recv_timeout(Duration::from_secs(9))
            .unwrap()
            .unwrap();
        let warnings = scans.status.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
    } else {
        while scans.results.try_recv().is_ok() {}
        let path = if mode == "grok" { &grok } else { &code };
        let connection = rusqlite::Connection::open(path).unwrap();
        connection
            .execute_batch(if mode == "grok" {
                "UPDATE session_docs SET title='Changed by watcher test'"
            } else {
                "UPDATE session SET title='Changed by watcher test'"
            })
            .unwrap();
        drop(connection);
        // Far shorter than the 60-second quiet refresh: only a watcher request can pass.
        let result = scans
            .results
            .recv_timeout(Duration::from_secs(7))
            .unwrap()
            .unwrap();
        assert!(
            result
                .projects
                .iter()
                .flat_map(|p| &p.sessions)
                .any(|s| s.title == "Changed by watcher test")
        );
    }
}
