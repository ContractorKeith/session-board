use session_board::{
    config::Config,
    model::{Agent, Project, Session, Status},
    project::ProjectRoots,
    scan,
    source::{Registry, Source, SourceScan},
    state::State,
};
use std::{collections::BTreeMap, fs, path::Path, process::Command};

fn session(id: &str, cwd: &str) -> Session {
    Session {
        id: id.into(),
        agent: Agent::Codex,
        cwd: cwd.into(),
        project_root: cwd.into(),
        title: "placeholder".into(),
        started_at: 100,
        last_activity_at: 200,
        status: Status::Done,
        last_user_text: "user placeholder".into(),
        last_assistant_text: "assistant placeholder".into(),
        transcript_path: "fixture.jsonl".into(),
        resume_hint: format!("codex resume {id}"),
        pane: None,
    }
}

struct FakeSource {
    agent: Agent,
    sessions: Vec<Session>,
}
impl Source for FakeSource {
    fn agent(&self) -> Agent {
        self.agent
    }
    fn scan(&self, _: &Config) -> SourceScan {
        SourceScan {
            sessions: self.sessions.clone(),
            skipped_lines: 2,
            skipped_stores: 1,
            warnings: vec!["fixture warning".into()],
        }
    }
}

#[test]
fn model_json_shape_and_round_trip() {
    let expected = serde_json::json!({
        "id": "one", "agent": "codex", "cwd": "/project", "project_root": "/project",
        "title": "placeholder", "started_at": 100, "last_activity_at": 200, "status": "done",
        "last_user_text": "user placeholder", "last_assistant_text": "assistant placeholder",
        "transcript_path": "fixture.jsonl", "resume_hint": "codex resume one", "pane": null
    });
    let session = session("one", "/project");
    assert_eq!(serde_json::to_value(&session).unwrap(), expected);
    assert_eq!(
        serde_json::from_value::<Session>(expected).unwrap(),
        session
    );
    let expected = serde_json::json!({
        "root": "/project", "name": "project", "sessions": [session], "pinned": false,
        "tags": [], "archived": false, "unread_count": 1
    });
    let project: Project = serde_json::from_value(expected.clone()).unwrap();
    assert_eq!(serde_json::to_value(project).unwrap(), expected);
    for (status, name) in [
        (Status::Working, "working"),
        (Status::Waiting, "waiting"),
        (Status::Idle, "idle"),
        (Status::Error, "error"),
        (Status::Done, "done"),
    ] {
        assert_eq!(serde_json::to_value(status).unwrap(), name);
        assert_eq!(
            serde_json::from_value::<Status>(serde_json::json!(name)).unwrap(),
            status
        );
    }
    for (agent, name) in [
        (Agent::ClaudeCode, "claude_code"),
        (Agent::Codex, "codex"),
        (Agent::Grok, "grok"),
        (Agent::Opencode, "opencode"),
    ] {
        assert_eq!(serde_json::to_value(agent).unwrap(), name);
        assert_eq!(
            serde_json::from_value::<Agent>(serde_json::json!(name)).unwrap(),
            agent
        );
    }
}

#[test]
fn scan_runs_all_enabled_sources_resolves_groups_and_applies_state() {
    let registry: Registry = vec![
        Box::new(FakeSource {
            agent: Agent::Codex,
            sessions: vec![session("one", "/project/subdir"), session("two", "/other")],
        }),
        Box::new(FakeSource {
            agent: Agent::ClaudeCode,
            sessions: vec![session("three", "/project")],
        }),
    ];
    let state = State {
        pins: vec!["/project".into()],
        tags: BTreeMap::from([("/project".into(), vec!["client".into()])]),
        archived: vec!["/other".into()],
        read: BTreeMap::from([("one".into(), 200)]),
    };
    let config = Config {
        roots: vec!["/empty".into()],
        ..Config::default()
    };
    let result = scan::scan(&registry, &config, &state, |cwd| {
        if cwd == Path::new("/project/subdir") {
            "/project".into()
        } else {
            cwd.into()
        }
    });
    assert_eq!(result.skipped_lines, 4);
    assert_eq!(result.skipped_stores, 2);
    assert_eq!(result.warnings.len(), 2);
    assert_eq!(result.projects.len(), 3);
    assert!(result.projects[0].sessions.is_empty());
    assert!(result.projects[1].archived);
    let project = &result.projects[2];
    assert_eq!(project.name, "project");
    assert!(project.pinned);
    assert_eq!(project.tags, ["client"]);
    assert_eq!(project.unread_count, 1);
    assert_eq!(
        project
            .sessions
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["one", "three"]
    );
    assert!(
        project
            .sessions
            .iter()
            .all(|s| s.project_root == Path::new("/project"))
    );
    let mut disabled = config.clone();
    disabled.agents.codex = false;
    let result = scan::scan(&registry, &disabled, &state, Path::to_path_buf);
    assert_eq!(result.skipped_lines, 2);
    assert_eq!(
        result
            .projects
            .iter()
            .map(|p| p.sessions.len())
            .sum::<usize>(),
        1
    );
}

#[test]
fn state_and_config_round_trip_atomic_replacement_and_corrupt_state() {
    let temp = tempfile::tempdir().unwrap();
    let state_path = temp.path().join("state/state.toml");
    let config_path = temp.path().join("config/config.toml");
    assert_eq!(
        State::load(&state_path).unwrap(),
        (State::default(), vec![])
    );
    assert_eq!(
        Config::load(&config_path).unwrap(),
        (Config::default(), vec![])
    );
    let state = State {
        pins: vec!["/project".into()],
        tags: BTreeMap::from([("/project".into(), vec!["client".into()])]),
        archived: vec!["/project".into(), "session-id".into()],
        read: BTreeMap::from([("session-id".into(), 200)]),
    };
    state.save(&state_path).unwrap();
    assert_eq!(State::load(&state_path).unwrap(), (state.clone(), vec![]));
    State::default().save(&state_path).unwrap();
    assert_eq!(State::load(&state_path).unwrap().0, State::default());
    assert_eq!(
        fs::read_dir(state_path.parent().unwrap()).unwrap().count(),
        1
    );
    let config = Config {
        roots: vec!["/extra".into()],
        window_days: 14,
        notify: true,
        ..Config::default()
    };
    config.save(&config_path).unwrap();
    assert_eq!(Config::load(&config_path).unwrap(), (config, vec![]));
    Config::default().save(&config_path).unwrap();
    assert_eq!(Config::load(&config_path).unwrap().0, Config::default());
    fs::write(&state_path, "[broken").unwrap();
    let (loaded, warnings) = State::load(&state_path).unwrap();
    assert_eq!(loaded, State::default());
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("corrupt state ignored"));
}

#[test]
fn config_every_key_optional_and_unknown_keys_warn() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    fs::write(&path, "window_days = 3\nfuture = true\n[agents]\ncodex = false\nfuture = true\n[tmux]\nfuture = 1\n").unwrap();
    let (config, warnings) = Config::load(&path).unwrap();
    assert_eq!(config.window_days, 3);
    assert!(!config.agents.codex);
    assert!(config.agents.claude_code && config.agents.grok && config.agents.opencode);
    assert_eq!(config.tmux.socket, "");
    assert_eq!(warnings.len(), 3);
    assert!(warnings.iter().any(|s| s.ends_with("agents.future")));
    fs::write(&path, "window_days = 'bad'").unwrap();
    assert!(Config::load(&path).is_err());
}

fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end_matches('\n')
        .into()
}

#[test]
fn project_roots_git_nested_worktree_missing_and_cached() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    git(&project, &["init", "-q"]);
    git(
        &project,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
    );
    let nested = project.join("nested");
    fs::create_dir(&nested).unwrap();
    let worktree = temp.path().join("worktree");
    git(
        &project,
        &[
            "worktree",
            "add",
            "-qb",
            "fixture",
            worktree.to_str().unwrap(),
        ],
    );
    let expected = git(&project, &["rev-parse", "--show-toplevel"]);
    let mut roots = ProjectRoots::default();
    assert_eq!(roots.resolve(&nested), Path::new(&expected));
    assert_eq!(
        roots.resolve(&worktree),
        Path::new(&git(&worktree, &["rev-parse", "--show-toplevel"]))
    );
    fs::remove_dir(&nested).unwrap();
    assert_eq!(roots.resolve(&nested), Path::new(&expected));
    let missing = temp.path().join("missing");
    assert_eq!(roots.resolve(&missing), missing);
    assert_eq!(roots.resolve(temp.path()), temp.path());
}

/// All subprocesses receive both overrides, never touching the owner's sboard files.
fn command(temp: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sboard"));
    command
        .env("SBOARD_STATE_DIR", temp.join("state"))
        .env("SBOARD_CONFIG_DIR", temp.join("config"));
    command
}

#[test]
fn empty_fixture_list_matches_golden_and_cli_contract() {
    let temp = tempfile::tempdir().unwrap();
    let fixture_dir = temp.path().join("fixtures");
    fs::create_dir(&fixture_dir).unwrap();
    let output = command(temp.path())
        .current_dir(&fixture_dir)
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        include_str!("../fixtures/list.json")
    );
    assert!(output.stderr.is_empty());
    let output = command(temp.path()).arg("version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        env!("CARGO_PKG_VERSION")
    );
    let output = command(temp.path()).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"TUI lands in M2\n");
    let output = command(temp.path()).arg("unknown").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(!temp.path().join("state").exists());
    assert!(!temp.path().join("config").exists());
}

#[test]
fn cli_uses_directory_overrides_and_reports_warnings() {
    let temp = tempfile::tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let config_dir = temp.path().join("config");
    fs::create_dir(&state_dir).unwrap();
    fs::create_dir(&config_dir).unwrap();
    fs::write(state_dir.join("state.toml"), "[corrupt").unwrap();
    fs::write(
        config_dir.join("config.toml"),
        "roots = ['/extra']\nfuture = 1\n",
    )
    .unwrap();
    let output = command(temp.path())
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let projects: Vec<Project> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].root, Path::new("/extra"));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("corrupt state ignored"));
    assert!(stderr.contains("unknown config key: future"));
}

#[test]
fn doctor_reports_known_paths_without_reading_stores() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join(".claude/projects")).unwrap();
    let output = command(temp.path())
        .env("HOME", temp.path())
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stores: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let stores = stores.as_array().unwrap();
    assert_eq!(stores.len(), 4);
    for (store, agent, suffix, exists) in [
        (&stores[0], "claude_code", ".claude/projects", true),
        (&stores[1], "codex", ".codex/sessions", false),
        (
            &stores[2],
            "grok",
            ".grok/sessions/session_search.sqlite",
            false,
        ),
        (
            &stores[3],
            "opencode",
            ".local/share/opencode/opencode.db",
            false,
        ),
    ] {
        assert_eq!(store["agent"], agent);
        assert_eq!(store["path"], temp.path().join(suffix).to_str().unwrap());
        assert_eq!(store["exists"], exists);
    }
}
