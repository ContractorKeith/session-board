mod common;
use common::git;
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
            ..SourceScan::default()
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
    let canonical = fs::canonicalize(temp.path()).unwrap();
    assert_eq!(roots.resolve(temp.path()), canonical);
    #[cfg(unix)]
    {
        // macOS temp roots have a symlinked prefix; exercise that case on Linux too.
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(temp.path(), &alias).unwrap();
        assert_eq!(roots.resolve(&alias), canonical);
    }
}

/// All subprocesses receive both overrides, never touching the owner's sboard files.
fn command(temp: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sboard"));
    command
        .env("CLAUDE_CONFIG_DIR", temp.join(".claude"))
        .env("CODEX_HOME", temp.join(".codex"))
        .env("SBOARD_STATE_DIR", temp.join("state"))
        .env("SBOARD_CONFIG_DIR", temp.join("config"));
    command
}

#[test]
fn empty_stores_list_and_cli_contract() {
    let temp = tempfile::tempdir().unwrap();
    let fixture_dir = temp.path().join("fixtures");
    fs::create_dir(&fixture_dir).unwrap();
    let output = command(temp.path())
        .current_dir(&fixture_dir)
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "[]\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("no enabled stores found"));
    let output = command(temp.path()).arg("version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        env!("CARGO_PKG_VERSION")
    );
    let output = command(temp.path()).arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for name in ["list", "doctor", "version"] {
        assert!(help.contains(name));
    }
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
    assert_eq!(output.status.code(), Some(2));
    let projects: Vec<Project> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].root, Path::new("/extra"));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("corrupt state ignored"));
    assert!(stderr.contains("unknown config key: future"));
}

#[test]
fn doctor_reports_known_paths_and_missing_stores() {
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

/// #11: anything that keeps state.toml from loading is a warning plus empty state.
#[test]
fn unreadable_state_is_warned_about_and_treated_as_empty() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    fs::write(&path, b"tags = { \"/p\" = [\"caf\xc3").unwrap();
    let (state, warnings) = State::load(&path).unwrap();
    assert_eq!(state, State::default());
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].starts_with(&format!("{}: corrupt state ignored", path.display())));
    let dir = temp.path().join("dir");
    fs::create_dir(&dir).unwrap();
    let (state, warnings) = State::load(&dir).unwrap();
    assert_eq!(state, State::default());
    assert_eq!(warnings.len(), 1);
}

/// #12: an exported-but-empty override falls back to the default under HOME.
#[test]
fn empty_overrides_count_as_unset() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(home.join(".config/sboard")).unwrap();
    fs::write(
        home.join(".config/sboard/config.toml"),
        "roots = ['/from-home']\n",
    )
    .unwrap();
    let cwd = temp.path().join("cwd");
    fs::create_dir(&cwd).unwrap();
    fs::write(cwd.join("config.toml"), "roots = ['/from-cwd']\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_sboard"))
        .env("HOME", &home)
        .env("CLAUDE_CONFIG_DIR", "")
        .env("CODEX_HOME", "")
        .env("SBOARD_STATE_DIR", "")
        .env("SBOARD_CONFIG_DIR", "")
        .current_dir(&cwd)
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let projects: Vec<Project> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].root, Path::new("/from-home"));
    let output = command(temp.path())
        .env("HOME", "")
        .arg("doctor")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("HOME is not set")
    );
}

/// #13: saving through a symlink updates its target and keeps the mode.
#[cfg(unix)]
#[test]
fn save_writes_through_symlink_and_keeps_mode() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("dot/state.toml");
    fs::create_dir(temp.path().join("dot")).unwrap();
    fs::write(&target, "pins = ['/old']\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    let link = temp.path().join("state.toml");
    symlink(&target, &link).unwrap();
    let state = State {
        pins: vec!["/new".into()],
        ..State::default()
    };
    state.save(&link).unwrap();
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(State::load(&target).unwrap().0, state);
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert_eq!(fs::read_dir(temp.path().join("dot")).unwrap().count(), 1);
}

/// #14: an inherited GIT_DIR must not redirect root resolution. The probe runs in a child
/// process so the environment change cannot leak into the other git tests.
#[test]
fn project_roots_ignore_inherited_git_environment() {
    let temp = tempfile::tempdir().unwrap();
    let other = temp.path().join("other");
    fs::create_dir(&other).unwrap();
    git(&other, &["init", "-q"]);
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "git_environment_probe"])
        .env("GIT_DIR", other.join(".git"))
        .env("GIT_WORK_TREE", &other)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
#[ignore = "run by project_roots_ignore_inherited_git_environment with GIT_DIR set"]
fn git_environment_probe() {
    assert!(std::env::var_os("GIT_DIR").is_some(), "probe needs GIT_DIR");
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("nested")).unwrap();
    git(&repo, &["init", "-q"]);
    let repo = repo.canonicalize().unwrap();
    let mut roots = ProjectRoots::default();
    assert_eq!(roots.resolve(&repo.join("nested")), repo);
}

/// #15: a config error names the file.
#[test]
fn config_errors_name_the_file() {
    let temp = tempfile::tempdir().unwrap();
    let config_dir = temp.path().join("config");
    fs::create_dir(&config_dir).unwrap();
    fs::write(config_dir.join("config.toml"), "window_days = '7'\n").unwrap();
    let output = command(temp.path()).arg("list").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("sboard: "));
    assert!(stderr.contains("config.toml: "), "{stderr}");
    assert!(stderr.contains("window_days"), "{stderr}");
}

/// #10: a full or closed stdout is a sentence or a quiet exit, never a panic.
#[cfg(target_os = "linux")]
#[test]
fn closed_or_full_stdout_does_not_panic() {
    use std::process::Stdio;
    let temp = tempfile::tempdir().unwrap();
    // An empty but present store keeps the exit code at 0 whether or not the pipe closes first.
    fs::create_dir_all(temp.path().join(".claude/projects")).unwrap();
    let full = fs::File::create("/dev/full").unwrap();
    let output = command(temp.path())
        .arg("version")
        .stdout(Stdio::from(full))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("sboard: "), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
    // Close the reader before spawning: even a tiny response must hit EPIPE.
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let output = command(temp.path())
        .args(["list", "--json"])
        .stdout(Stdio::from(writer))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("panicked"),
        "{output:?}"
    );
}

/// Sessions within a project are newest first, ties by id.
#[test]
fn scan_sorts_sessions_newest_first_then_by_id() {
    let mut a = session("b", "/p");
    a.last_activity_at = 300;
    let mut b = session("a", "/p");
    b.last_activity_at = 300;
    let mut c = session("c", "/p");
    c.last_activity_at = 500;
    let registry: Registry = vec![Box::new(FakeSource {
        agent: Agent::Codex,
        sessions: vec![a, b, c],
    })];
    let result = scan::scan(&registry, &Config::default(), &State::default(), |cwd| {
        cwd.to_owned()
    });
    let ids: Vec<&str> = result.projects[0]
        .sessions
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(ids, ["c", "a", "b"]);
}

/// A symlinked config or state file (dotfiles-managed) loads through the link.
#[cfg(unix)]
#[test]
fn symlinked_config_and_state_files_load() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real");
    let config_dir = temp.path().join("config");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&real).unwrap();
    fs::create_dir_all(&config_dir).unwrap();
    fs::create_dir_all(&state_dir).unwrap();
    // An empty but present store keeps the exit code at 0 (exit 2 means no stores found).
    fs::create_dir_all(temp.path().join(".claude/projects")).unwrap();
    fs::write(real.join("config.toml"), "roots = ['/from-link']\n").unwrap();
    fs::write(real.join("state.toml"), "pins = ['/from-link']\n").unwrap();
    symlink(real.join("config.toml"), config_dir.join("config.toml")).unwrap();
    symlink(real.join("state.toml"), state_dir.join("state.toml")).unwrap();
    let output = command(temp.path())
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let projects: Vec<Project> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].root, Path::new("/from-link"));
    assert!(projects[0].pinned);
}

#[test]
fn default_command_refuses_piped_stdout() {
    let temp = tempfile::tempdir().unwrap();
    let output = command(temp.path()).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "sboard: interactive terminal required; run sboard list for script output.\n"
    );
}
