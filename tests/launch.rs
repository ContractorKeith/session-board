mod common;
use session_board::{
    config::Config,
    model::Agent,
    scan,
    source::{Registry, claude::Claude},
    state::State,
    tmux,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

fn script(path: &Path, text: &str) {
    fs::write(path, format!("#!/bin/sh\n{text}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
struct Sandbox {
    temp: TempDir,
}
impl Sandbox {
    fn new() -> Self {
        let s = Self {
            temp: tempfile::tempdir().unwrap(),
        };
        for dir in [
            "bin",
            "project",
            "claude/projects/project",
            "codex/sessions",
            "config",
        ] {
            fs::create_dir_all(s.path(dir)).unwrap();
        }
        script(
            &s.path("bin/tmux"),
            "printf '%s\\n' \"CALL\" \"$@\" >> \"$RECORD_TMUX\"\nif [ \"$1\" = '-L' ]; then shift 2; fi\nif [ \"$1\" = list-panes ]; then [ -f \"$PANES\" ] && cat \"$PANES\"; else exit 0; fi",
        );
        for agent in ["claude", "codex"] {
            script(
                &s.path(&format!("bin/{agent}")),
                "pwd > \"$RECORD_CWD\"\nprintf '%s\\n' \"$@\" > \"$RECORD_ARGS\"",
            );
        }
        s
    }
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.temp.path().join(name)
    }
    fn cmd(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sboard"));
        common::no_tmux(&mut cmd, self.temp.path());
        cmd.env(
            "PATH",
            format!(
                "{}:{}",
                self.path("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env_remove("TMUX")
        .env("RECORD_TMUX", self.path("tmux-args"))
        .env("PANES", self.path("panes"))
        .env("RECORD_CWD", self.path("cwd"))
        .env("RECORD_ARGS", self.path("args"))
        .env("SBOARD_CONFIG_DIR", self.path("config"))
        .env("SBOARD_STATE_DIR", self.path("state"))
        .env("CLAUDE_CONFIG_DIR", self.path("claude"))
        .env("CODEX_HOME", self.path("codex"));
        cmd
    }
    fn git(&self, args: &[&str]) {
        common::git(&self.path("project"), args);
    }
    fn repo(&self) {
        self.git(&["init", "-q"]);
        self.git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ]);
    }
    fn session(&self, agent: &str) -> String {
        let text = fs::read_to_string("fixtures/claude_code/project/done.jsonl")
            .unwrap()
            .replace("/projects/alpha", self.path("project").to_str().unwrap());
        // Preserve the fixture schema, replacing cwd wherever it occurs.
        let mut rows: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        for row in &mut rows {
            if row.get("cwd").is_some() {
                row["cwd"] = self.path("project").to_str().unwrap().into();
            }
        }
        fs::write(
            self.path("claude/projects/project/session.jsonl"),
            rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
        )
        .unwrap();
        if agent == "codex" {
            fs::remove_file(self.path("claude/projects/project/session.jsonl")).unwrap();
            fs::write(self.path("codex/sessions/rollout-test.jsonl"), format!("{{\"timestamp\":\"2026-11-01T12:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"native-id\",\"cwd\":{},\"source\":\"cli\"}}}}\n", serde_json::to_string(&self.path("project")).unwrap())).unwrap();
            "codex:native-id".into()
        } else {
            format!(
                "claude_code:{}",
                rows.iter().find_map(|r| r["sessionId"].as_str()).unwrap()
            )
        }
    }
}
fn success(out: Output) {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn failure(out: Output, message: &str) {
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(err.contains(message), "{err}");
}
#[test]
fn capture_matches_canonical_cwd_and_agent() {
    let s = Sandbox::new();
    let link = s.path("link");
    std::os::unix::fs::symlink(s.path("project"), &link).unwrap();
    let panes = tmux::parse(
        &fs::read_to_string("tests/fixtures/list-panes.txt")
            .unwrap()
            .replace("PROJECT", link.to_str().unwrap())
            .replace("OTHER", s.path("other").to_str().unwrap()),
    );
    let registry: Registry = vec![Box::new(Claude {
        root: "fixtures/claude_code".into(),
        now: 1793534400,
    })];
    let mut session = scan::scan(
        &registry,
        &Config::default(),
        &State::default(),
        Path::to_path_buf,
    )
    .projects
    .remove(0)
    .sessions
    .remove(0);
    session.cwd = s.path("project");
    assert_eq!(
        tmux::matching(&session, &panes).unwrap().target(),
        "dev:2.1"
    );
    session.agent = Agent::Codex;
    assert!(tmux::matching(&session, &panes).is_none());
    assert!(tmux::parse("malformed").is_empty());
}
#[test]
fn cli_resume_uses_native_id_and_directory_without_tmux_server() {
    for agent in ["claude_code", "codex"] {
        let s = Sandbox::new();
        let id = s.session(agent);
        success(s.cmd().args(["open", &id]).output().unwrap());
        assert_eq!(
            fs::read_to_string(s.path("cwd")).unwrap().trim(),
            s.path("project").canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(
            fs::read_to_string(s.path("args")).unwrap(),
            format!(
                "{}\n{}\n",
                if agent == "codex" {
                    "resume"
                } else {
                    "--resume"
                },
                id.split_once(':').unwrap().1
            )
        );
        let calls = fs::read_to_string(s.path("tmux-args")).unwrap();
        assert_eq!(calls.matches("list-panes").count(), 1);
        assert!(!calls.contains("attach"));
    }
}
#[test]
fn cli_jump_honours_socket_inside_and_outside_tmux() {
    for inside in [false, true] {
        let s = Sandbox::new();
        let id = s.session("claude_code");
        fs::write(
            s.path("panes"),
            format!(
                "dev\t2\t1\t{}\tclaude\t999999999\n",
                s.path("project").display()
            ),
        )
        .unwrap();
        fs::write(
            s.path("config/config.toml"),
            "[tmux]\nsocket = 'test-socket'\n",
        )
        .unwrap();
        let mut cmd = s.cmd();
        if inside {
            cmd.env("TMUX", "fake");
        }
        success(cmd.args(["open", &id]).output().unwrap());
        let calls = fs::read_to_string(s.path("tmux-args")).unwrap();
        assert_eq!(calls.matches("list-panes").count(), 1);
        assert!(calls.contains("select-window\n-t\n=dev:2"));
        assert!(calls.contains("select-pane\n-t\n=dev:2.1"));
        assert_eq!(calls.contains("switch-client\n-t\n=dev"), inside);
        assert_eq!(calls.contains("attach\n-t\n=dev"), !inside);
        assert!(
            calls
                .split("CALL\n")
                .skip(1)
                .all(|c| c.starts_with("-L\ntest-socket\n"))
        );
        assert!(!s.path("cwd").exists());
    }
}
#[test]
fn cli_worktree_creates_branch_runs_agent_and_refuses_invalid_inputs() {
    let s = Sandbox::new();
    s.repo();
    success(
        s.cmd()
            .arg("new")
            .arg(s.path("project"))
            .args(["--worktree", "feat/x", "--agent", "claude_code"])
            .output()
            .unwrap(),
    );
    let worktree = s.path("project-wt/feat/x");
    assert!(worktree.join(".git").exists());
    assert_eq!(
        fs::read_to_string(s.path("cwd")).unwrap().trim(),
        worktree.canonicalize().unwrap().to_str().unwrap()
    );
    assert!(
        Command::new("git")
            .args(["show-ref", "--verify", "refs/heads/feat/x"])
            .current_dir(s.path("project"))
            .output()
            .unwrap()
            .status
            .success()
    );
    for (branch, error) in [
        ("feat/x", "worktree exists"),
        ("feat/../x", "invalid branch name"),
        ("bad name", "invalid branch name"),
        ("-bad", "invalid branch name"),
    ] {
        failure(
            s.cmd()
                .arg("new")
                .arg(s.path("project"))
                .arg(format!("--worktree={branch}"))
                .output()
                .unwrap(),
            error,
        );
    }
    fs::create_dir(s.path("nongit")).unwrap();
    failure(
        s.cmd()
            .arg("new")
            .arg(s.path("nongit"))
            .args(["--worktree", "feat/y"])
            .output()
            .unwrap(),
        "not a git repo",
    );
    failure(
        s.cmd().args(["new", "/nonexistent"]).output().unwrap(),
        "directory no longer exists",
    );
    failure(
        s.cmd().args(["open", "nope"]).output().unwrap(),
        "session not found",
    );
}
#[test]
fn existing_branch_and_tmux_launch() {
    let s = Sandbox::new();
    s.repo();
    s.git(&["branch", "existing"]);
    success(
        s.cmd()
            .arg("new")
            .arg(s.path("project"))
            .args(["--worktree", "existing", "--agent", "codex"])
            .env("TMUX", "fake")
            .output()
            .unwrap(),
    );
    let calls = fs::read_to_string(s.path("tmux-args")).unwrap();
    assert!(calls.contains(&format!(
        "new-window\n-c\n{}\ncodex",
        s.path("project-wt/existing").canonicalize().unwrap().display()
    )));
    assert!(!s.path("cwd").exists());
}
#[test]
fn missing_cwd_refuses_resume_and_help_lists_commands() {
    let s = Sandbox::new();
    let id = s.session("claude_code");
    fs::remove_dir(s.path("project")).unwrap();
    failure(
        s.cmd().args(["open", &id]).output().unwrap(),
        "directory no longer exists",
    );
    assert!(!s.path("cwd").exists());
    let out = s.cmd().arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("open") && help.contains("new"));
}

#[test]
fn doctor_reports_absent_tmux_and_failed_agents_report_one_sentence() {
    let s = Sandbox::new();
    let out = s.cmd().arg("doctor").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("tmux: absent"));
    let id = s.session("claude_code");
    script(&s.path("bin/claude"), "exit 9");
    failure(
        s.cmd().args(["open", &id]).output().unwrap(),
        "exited unsuccessfully",
    );
}
