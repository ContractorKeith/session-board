mod common;
use session_board::{
    cli::{self, AgentFilter, StatusFilter},
    config::Config,
    model::{Project, Status},
    project::{ProjectRoots, normalize, normalize_settings},
    scan,
    source::{Registry, Source, claude::Claude, codex::Codex},
    state::State,
    status::{self, DONE_SECONDS, Tail, WORKING_SECONDS},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
const NOW: i64 = 1793534400;
fn fixtures() -> Registry {
    vec![
        Box::new(Claude {
            root: "fixtures/claude_code".into(),
            now: NOW,
        }),
        Box::new(Codex {
            root: "fixtures/codex/sessions".into(),
            index: "fixtures/codex/session_index.jsonl".into(),
            now: NOW,
        }),
    ]
}
fn projects() -> Vec<Project> {
    scan::scan(
        &fixtures(),
        &Config::default(),
        &State::default(),
        Path::to_path_buf,
    )
    .projects
}
#[test]
fn every_status_rule_and_ignored_records() {
    for source in fixtures() {
        let result = source.scan(&Config::default());
        assert_eq!(result.sessions.len(), 7);
        assert_eq!(result.skipped_lines, 1);
        assert_eq!(result.skipped_stores, 0);
        assert_eq!(
            result.skipped_sessions,
            if source.agent() == session_board::model::Agent::Codex {
                3
            } else {
                1
            }
        );
        for (name, expected) in [
            ("working", Status::Working),
            ("stuck", Status::Error),
            ("waiting", Status::Waiting),
            ("done", Status::Done),
            ("idle", Status::Idle),
            ("error", Status::Error),
            (
                "malformed",
                if source.agent() == session_board::model::Agent::ClaudeCode {
                    Status::Working
                } else {
                    Status::Idle
                },
            ),
        ] {
            let s = result
                .sessions
                .iter()
                .find(|s| s.id.ends_with(&format!(":{name}")))
                .unwrap();
            assert_eq!(s.status, expected, "{}", s.id);
            assert_eq!(s.cwd, Path::new("/home/user/project"));
            if name == "done" {
                assert_eq!(s.title, "Title placeholder");
            }
        }
        assert!(
            result
                .status_reasons
                .iter()
                .any(|s| s.contains("30 minutes"))
        );
    }
}
#[test]
fn list_json_golden() {
    let mut projects = projects();
    cli::filter(&mut projects, &State::default(), None, None, None, false);
    let mut output = Vec::new();
    cli::list(&mut output, &projects, true).unwrap();
    let actual = String::from_utf8(output).unwrap();
    assert_eq!(actual, fs::read_to_string("fixtures/list.json").unwrap());
}
#[test]
fn status_thresholds_and_first_match_wins() {
    let mut tail = Tail {
        pending: true,
        ended: true,
        text: "Question?".into(),
        explicit_error: true,
    };
    assert_eq!(
        status::derive(&tail, NOW - WORKING_SECONDS + 1, NOW),
        Status::Working
    );
    assert_eq!(
        status::derive(&tail, NOW - WORKING_SECONDS, NOW),
        Status::Error
    );
    tail.pending = false;
    assert_eq!(status::derive(&tail, 0, NOW), Status::Waiting);
    tail.text = "Done".into();
    assert_eq!(
        status::derive(&tail, NOW - DONE_SECONDS + 1, NOW),
        Status::Done
    );
    assert_eq!(status::derive(&tail, NOW - DONE_SECONDS, NOW), Status::Idle);
    for marker in [
        "<question>Decision placeholder</question>",
        "<request_user_input>placeholder</request_user_input>",
    ] {
        tail.text = marker.into();
        assert_eq!(status::derive(&tail, NOW, NOW), Status::Waiting);
    }
    tail.ended = false;
    assert_eq!(status::derive(&tail, NOW, NOW), Status::Error);
}
#[test]
fn attention_filters_read_marks_archives_and_ordering() {
    let mut state = State::default();
    state.read.insert("codex:done".into(), NOW);
    let mut p = projects();
    cli::filter(
        &mut p,
        &state,
        None,
        Some(StatusFilter::Attention),
        None,
        false,
    );
    let ids: Vec<_> = p[0].sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "claude_code:waiting",
            "codex:waiting",
            "claude_code:error",
            "codex:error",
            "claude_code:stuck",
            "codex:stuck",
            "claude_code:done"
        ]
    );
    // Read keys from the two agents do not collide.
    assert_eq!(p[0].unread_count, 7);
    state.archived.push("claude_code:done".into());
    let mut p = projects();
    cli::filter(
        &mut p,
        &state,
        Some(Path::new("/home/user/project")),
        Some(StatusFilter::Done),
        Some(AgentFilter::ClaudeCode),
        false,
    );
    assert!(p.is_empty());
    let mut p = projects();
    cli::filter(
        &mut p,
        &state,
        None,
        Some(StatusFilter::Done),
        Some(AgentFilter::ClaudeCode),
        true,
    );
    assert_eq!(p[0].sessions.len(), 1);
}
fn command(temp: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sboard"));
    common::no_tmux(&mut command, temp);
    command
        .env("SBOARD_STATE_DIR", temp.join("state"))
        .env("SBOARD_CONFIG_DIR", temp.join("config"))
        .env("CLAUDE_CONFIG_DIR", temp.join("claude"))
        .env("CODEX_HOME", temp.join("codex"));
    command
}
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}
#[test]
fn cli_overrides_doctor_counts_and_regular_file_store() {
    let temp = tempfile::tempdir().unwrap();
    copy_tree(
        Path::new("fixtures/claude_code"),
        &temp.path().join("claude/projects"),
    );
    copy_tree(Path::new("fixtures/codex"), &temp.path().join("codex"));
    let output = command(temp.path())
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stores: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for i in 0..2 {
        assert_eq!(stores["stores"][i]["sessions"], 7);
        assert_eq!(stores["stores"][i]["skipped_lines"], 1);
        assert_eq!(stores["stores"][i]["skipped_stores"], 0);
    }
    let output = command(temp.path())
        .args([
            "list",
            "--json",
            "--all",
            "--agent",
            "codex",
            "--project",
            "/home/user/project/../project/",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let p: Vec<Project> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(p[0].sessions.len(), 7);
    fs::remove_dir_all(temp.path().join("claude/projects")).unwrap();
    fs::write(temp.path().join("claude/projects"), "placeholder").unwrap();
    let output = command(temp.path()).arg("doctor").output().unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("not a directory"));
    assert!(text.contains("skipped stores: 1"));
    assert!(!temp.path().join("state").exists());
}
#[test]
fn normalize_settings_missing_paths_and_git_internals() {
    assert_eq!(
        normalize(Path::new("~/a/../b/"), Some(Path::new("/home/user"))),
        Some("/home/user/b".into())
    );
    assert_eq!(
        normalize(Path::new("/home/user/a/../b/"), None),
        Some("/home/user/b".into())
    );
    assert_eq!(normalize(Path::new("relative"), None), None);
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    common::git(&repo, &["init", "-q"]);
    assert!(
        repo.join(".git").is_dir(),
        "git init must use the test project"
    );
    let mut resolver = ProjectRoots::default();
    assert_eq!(
        resolver.resolve(&repo.join(".git/objects")),
        fs::canonicalize(&repo).unwrap()
    );
    #[cfg(unix)]
    {
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&repo, &alias).unwrap();
        let mut cfg = Config {
            roots: vec![alias.join("."), repo.join(".")],
            ..Default::default()
        };
        let mut state = State {
            pins: vec![alias.clone()],
            tags: [(alias, vec!["tag".into()])].into(),
            ..Default::default()
        };
        normalize_settings(&mut cfg, &mut state, temp.path());
        assert_eq!(cfg.roots, vec![fs::canonicalize(&repo).unwrap()]);
        assert_eq!(state.pins, cfg.roots);
        assert_eq!(state.tags.get(&cfg.roots[0]).unwrap(), &["tag"]);
    }
}
#[test]
fn invalid_cwds_empty_missing_metadata_and_non_utf8_are_counted() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("p")).unwrap();
    for (i, cwd) in ["", "relative", "/home/user/gone/../missing/"]
        .iter()
        .enumerate()
    {
        fs::write(temp.path().join(format!("p/{i}.jsonl")), format!("{{\"type\":\"user\",\"sessionId\":\"{i}\",\"cwd\":\"{cwd}\",\"timestamp\":\"2026-11-01T11:59:00Z\",\"message\":{{\"content\":\"placeholder\"}}}}\n")).unwrap();
    }
    fs::write(temp.path().join("p/bad.jsonl"), [0xff, b'\n']).unwrap();
    let result = Claude {
        root: temp.path().into(),
        now: NOW,
    }
    .scan(&Config::default());
    assert_eq!(result.skipped_lines, 1);
    assert_eq!(result.skipped_sessions, 3);
    assert_eq!(result.sessions.len(), 1);
    assert_eq!(result.sessions[0].cwd, PathBuf::from("/home/user/missing"));
    let result = Codex {
        root: temp.path().join("missing"),
        index: temp.path().join("absent"),
        now: NOW,
    }
    .scan(&Config::default());
    assert_eq!(result.skipped_stores, 1);
}
#[test]
fn large_transcript_uses_tail_and_ignores_corruption_in_omitted_middle() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("p")).unwrap();
    let head = fs::read_to_string("fixtures/claude_code/project/working.jsonl").unwrap();
    let end = fs::read_to_string("fixtures/claude_code/project/done.jsonl").unwrap();
    let text = head
        + &"{\"type\":\"attachment\",\"content\":\"placeholder\"}\n".repeat(10000)
        + "invalid middle line\n"
        + &"{\"type\":\"attachment\"}\n".repeat(10000)
        + &end;
    fs::write(temp.path().join("p/large.jsonl"), text).unwrap();
    let result = Claude {
        root: temp.path().into(),
        now: NOW,
    }
    .scan(&Config::default());
    assert_eq!(result.sessions.len(), 1);
    assert_eq!(result.sessions[0].status, Status::Done);
    assert_eq!(result.skipped_lines, 0);
    let old_error = fs::read_to_string("fixtures/claude_code/project/error.jsonl").unwrap();
    let user = fs::read_to_string("fixtures/claude_code/project/malformed.jsonl")
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned();
    fs::write(
        temp.path().join("p/large.jsonl"),
        old_error + &"{\"type\":\"attachment\"}\n".repeat(9000) + &user + "\n",
    )
    .unwrap();
    let result = Claude {
        root: temp.path().into(),
        now: NOW,
    }
    .scan(&Config::default());
    assert_eq!(
        result.sessions[0].status,
        Status::Working,
        "head error must not leak across omitted history"
    );
}

#[test]
fn tool_results_clear_pending_calls_without_ending_a_turn() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("p")).unwrap();
    let path = temp.path().join("p/session.jsonl");
    let head = fs::read_to_string("fixtures/claude_code/project/working.jsonl").unwrap();
    let result = serde_json::json!({"type":"user","sessionId":"working","cwd":"/home/user/project","timestamp":"2026-11-01T11:59:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"call","content":"Result placeholder"}]}});
    fs::write(&path, format!("{head}{result}\n")).unwrap();
    let source = Claude {
        root: temp.path().into(),
        now: NOW,
    };
    let scanned = source.scan(&Config::default());
    assert_eq!(scanned.sessions[0].status, Status::Working);
    assert_eq!(scanned.sessions[0].last_user_text, "User placeholder");
    let head =
        fs::read_to_string("fixtures/codex/sessions/2026/11/01/rollout-malformed.jsonl").unwrap();
    let head = head.lines().take(2).collect::<Vec<_>>().join("\n");
    let call = serde_json::json!({"timestamp":"2026-11-01T11:59:01Z","type":"response_item","payload":{"type":"custom_tool_call","call_id":"call","name":"placeholder","input":"placeholder"}});
    let output = serde_json::json!({"timestamp":"2026-11-01T11:59:02Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call","output":"Process exited with code 0"}});
    let path = temp.path().join("rollout-tools.jsonl");
    fs::write(&path, format!("{head}\n{call}\n")).unwrap();
    let source = Codex {
        root: temp.path().into(),
        index: temp.path().join("absent"),
        now: NOW,
    };
    assert_eq!(
        source.scan(&Config::default()).sessions[0].status,
        Status::Working
    );
    fs::write(path, format!("{head}\n{call}\n{output}\n")).unwrap();
    assert_eq!(
        source.scan(&Config::default()).sessions[0].status,
        Status::Idle
    );
}

#[test]
fn scan_window_and_title_unicode_limit() {
    for source in fixtures() {
        // Window filtering follows transcript activity, not the fixture file mtime.
        let config = Config {
            window_days: 0,
            ..Default::default()
        };
        assert!(source.scan(&config).sessions.is_empty());
        let config = Config {
            window_days: u32::MAX,
            ..Default::default()
        };
        assert_eq!(source.scan(&config).sessions.len(), 7);
    }
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("p")).unwrap();
    let v = serde_json::json!({"type":"user","sessionId":"unicode","cwd":"/home/user/project","timestamp":"2026-11-01T11:59:00.123+00:00","message":{"content":"é".repeat(80)}});
    fs::write(temp.path().join("p/session.jsonl"), format!("{v}\n")).unwrap();
    let scanned = Claude {
        root: temp.path().into(),
        now: NOW,
    }
    .scan(&Config::default());
    assert_eq!(scanned.sessions[0].title, "é".repeat(60));
    assert_eq!(scanned.sessions[0].last_activity_at, NOW - 60);
}
