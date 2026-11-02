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

// Process fixtures run sequentially so the scan budget excludes competing test subprocesses.
static PROCESS_FIXTURES: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn process_fixture_guard() -> std::sync::MutexGuard<'static, ()> {
    PROCESS_FIXTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn pane_record(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|s| format!("{}:{s}", s.len()))
        .collect::<String>()
        + "\n"
}
fn script(path: &Path, text: &str) {
    fs::write(path, format!("#!/bin/sh\n{text}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
struct Sandbox {
    temp: TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}
impl Sandbox {
    fn new() -> Self {
        let s = Self {
            _guard: process_fixture_guard(),
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
            "printf '%s\\n' \"CALL\" \"$@\" >> \"$RECORD_TMUX\"\nif [ \"$1\" = '-L' ] || [ \"$1\" = '-S' ]; then shift 2; fi\nif [ \"$1\" = list-panes ]; then [ -f \"$PANES\" ] && cat \"$PANES\"; elif [ \"$1\" = display ]; then echo %0; else exit 0; fi",
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
        let timestamp = String::from_utf8(
            Command::new("date")
                .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();
        let text = fs::read_to_string("fixtures/claude_code/project/done.jsonl")
            .unwrap()
            .replace("/projects/alpha", self.path("project").to_str().unwrap());
        // Preserve the fixture schema, replacing cwd wherever it occurs.
        let mut rows: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        for row in &mut rows {
            if row.get("timestamp").is_some() {
                row["timestamp"] = timestamp.clone().into();
            }
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
    let mut panes = tmux::parse(&fs::read_to_string("tests/fixtures/list-panes.txt").unwrap());
    for pane in &mut panes {
        pane.cwd = if pane.cwd == Path::new("/PROJECT") {
            link.clone()
        } else {
            s.path("other")
        };
    }
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
            if agent == "codex" {
                format!("resume\n--\n{}\n", id.split_once(':').unwrap().1)
            } else {
                format!("--resume={}\n", id.split_once(':').unwrap().1)
            }
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
            pane_record(&[
                "dev",
                "2",
                "1",
                "claude",
                "999999999",
                s.path("project").to_str().unwrap(),
            ]),
        )
        .unwrap();
        fs::write(
            s.path("config/config.toml"),
            "[tmux]\nsocket = 'test-socket'\n",
        )
        .unwrap();
        let mut cmd = s.cmd();
        if inside {
            cmd.env("TMUX", "/fake/test-socket,1,0");
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
                .all(|c| c.starts_with(if inside {
                    "-S\n/fake/test-socket\n"
                } else {
                    "-L\ntest-socket\n"
                }))
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
            .env("TMUX", "/fake/test-socket,1,0")
            .output()
            .unwrap(),
    );
    let calls = fs::read_to_string(s.path("tmux-args")).unwrap();
    assert!(calls.contains(&format!(
        "new-window\n-c\n{}\n",
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
    assert!(String::from_utf8_lossy(&out.stdout).contains("tmux: "));
    let id = s.session("claude_code");
    script(&s.path("bin/claude"), "exit 9");
    failure(
        s.cmd().args(["open", &id]).output().unwrap(),
        "exited unsuccessfully",
    );
}

#[test]
fn issue47_refuses_option_and_non_id_transcript_ids() {
    for agent in ["claude_code", "codex"] {
        for bad in ["--help", "-x", "a b", "x'\"$(id)`id`;", "a/b"] {
            let s = Sandbox::new();
            let id = s.session(agent);
            let path = s.path(if agent == "codex" {
                "codex/sessions/rollout-test.jsonl"
            } else {
                "claude/projects/project/session.jsonl"
            });
            let text = fs::read_to_string(&path).unwrap();
            let text = text.replace(
                &serde_json::to_string(id.split_once(':').unwrap().1).unwrap(),
                &serde_json::to_string(bad).unwrap(),
            );
            fs::write(path, text).unwrap();
            failure(
                s.cmd()
                    .args(["open", &format!("{agent}:{bad}")])
                    .output()
                    .unwrap(),
                "invalid session id",
            );
            assert!(!s.path("args").exists(), "unsafe id reached agent");
        }
    }
}

#[test]
fn argv_stays_verbatim_for_metacharacters_in_arguments_and_cwd() {
    let s = Sandbox::new();
    let literal = "spaces '\" $(id) `id` ;";
    let cwd = s.path(literal);
    fs::create_dir(&cwd).unwrap();
    let mut command = Command::new(s.path("bin/claude"));
    command
        .args(["--resume", literal, "second argument"])
        .current_dir(&cwd)
        .env("RECORD_CWD", s.path("cwd"))
        .env("RECORD_ARGS", s.path("args"));
    session_board::launch::run(&mut command).unwrap();
    assert_eq!(
        fs::read_to_string(s.path("args")).unwrap(),
        format!("--resume\n{literal}\nsecond argument\n")
    );
    assert_eq!(
        fs::read_to_string(s.path("cwd"))
            .unwrap()
            .strip_suffix('\n')
            .unwrap(),
        cwd.canonicalize().unwrap().to_str().unwrap()
    );
}

#[test]
fn issue54_branch_rules_and_remote_tracking() {
    let s = Sandbox::new();
    s.repo();
    s.git(&["tag", "tag-name"]);
    for name in ["refs/heads/x", "@", "tag-name", &"a".repeat(201)] {
        failure(
            s.cmd()
                .arg("new")
                .arg(s.path("project"))
                .args(["--worktree", name])
                .output()
                .unwrap(),
            "branch",
        );
        assert!(!s.path("cwd").exists());
    }
    s.git(&[
        "remote",
        "add",
        "origin",
        s.path("project").to_str().unwrap(),
    ]);
    s.git(&["update-ref", "refs/remotes/origin/remote-only", "HEAD"]);
    success(
        s.cmd()
            .arg("new")
            .arg(s.path("project"))
            .args(["--worktree", "remote-only"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        common::git(&s.path("project"), &["config", "branch.remote-only.merge"]),
        "refs/heads/remote-only"
    );
    assert_eq!(
        common::git(&s.path("project"), &["config", "branch.remote-only.remote"]),
        "origin"
    );
}

#[test]
fn issue55_failed_hook_rolls_back_new_worktree_and_branch() {
    let s = Sandbox::new();
    s.repo();
    script(
        &s.path("project/.git/hooks/post-checkout"),
        "echo 'hook-denied' >&2\nexit 1",
    );
    failure(
        s.cmd()
            .arg("new")
            .arg(s.path("project"))
            .args(["--worktree", "retry"])
            .output()
            .unwrap(),
        "hook-denied",
    );
    assert!(!s.path("project-wt/retry").exists());
    assert!(
        !Command::new("git")
            .args(["show-ref", "--verify", "--quiet", "refs/heads/retry"])
            .current_dir(s.path("project"))
            .status()
            .unwrap()
            .success()
    );
    fs::remove_file(s.path("project/.git/hooks/post-checkout")).unwrap();
    success(
        s.cmd()
            .arg("new")
            .arg(s.path("project"))
            .args(["--worktree", "retry"])
            .output()
            .unwrap(),
    );
}

#[test]
fn issue56_newline_root_and_linked_worktree_use_main_sibling() {
    let s = Sandbox::new();
    s.repo();
    let newline = s.path("project\n");
    fs::create_dir(&newline).unwrap();
    common::git(&newline, &["init", "-q"]);
    common::git(&newline, &["commit", "--allow-empty", "-qm", "initial"]);
    success(
        s.cmd()
            .arg("new")
            .arg(&newline)
            .args(["--worktree", "from-newline"])
            .output()
            .unwrap(),
    );
    assert!(s.path("project\n-wt/from-newline/.git").exists());
    assert!(!s.path("project-wt/from-newline").exists());
    s.git(&[
        "worktree",
        "add",
        "-b",
        "linked",
        s.path("linked").to_str().unwrap(),
    ]);
    success(
        s.cmd()
            .arg("new")
            .arg(s.path("linked"))
            .args(["--worktree", "from-linked"])
            .output()
            .unwrap(),
    );
    assert!(s.path("project-wt/from-linked/.git").exists());
    assert!(!s.path("linked-wt").exists());
}

#[test]
fn issue52_tab_in_pane_path_is_matched() {
    let s = Sandbox::new();
    let id = s.session("claude_code");
    let cwd = s.path("project\ttab");
    fs::rename(s.path("project"), &cwd).unwrap();
    let transcript = s.path("claude/projects/project/session.jsonl");
    fs::write(
        &transcript,
        fs::read_to_string(&transcript).unwrap().replace(
            &serde_json::to_string(&s.path("project")).unwrap(),
            &serde_json::to_string(&cwd).unwrap(),
        ),
    )
    .unwrap();
    fs::write(
        s.path("panes"),
        pane_record(&[
            "dev",
            "2",
            "1",
            "claude",
            "999999999",
            cwd.to_str().unwrap(),
        ]),
    )
    .unwrap();
    success(s.cmd().args(["open", &id]).output().unwrap());
    assert!(!s.path("args").exists(), "live pane was resumed");
}

#[test]
fn issue53_stale_tmux_launches_in_place() {
    let s = Sandbox::new();
    script(&s.path("bin/tmux"), "exit 1");
    success(
        s.cmd()
            .arg("new")
            .arg(s.path("project"))
            .env("TMUX", "/nonexistent,1,0")
            .output()
            .unwrap(),
    );
    assert!(s.path("cwd").exists());
}

#[test]
fn issue46_tmux_launch_escapes_formats_resolves_executable_and_forwards_stores() {
    let s = Sandbox::new();
    let alias = s.path("alias");
    std::os::unix::fs::symlink(s.temp.path(), &alias).unwrap();
    let cwd = alias.join("evil#(touch PWNED)");
    fs::create_dir(&cwd).unwrap();
    success(
        s.cmd()
            .arg("new")
            .arg(&cwd)
            .env("TMUX", "/fake/live,1,0")
            .output()
            .unwrap(),
    );
    let calls = fs::read_to_string(s.path("tmux-args")).unwrap();
    assert!(
        calls.contains(&format!(
            "-c\n{}\n",
            cwd.canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
                .replace('#', "##")
        )),
        "{calls}"
    );
    assert!(
        calls.contains(&format!("\n{}\n--\n", s.path("bin/claude").display())),
        "{calls}"
    );
    assert!(
        calls.contains(&format!(
            "-e\nCLAUDE_CONFIG_DIR={}\n",
            s.path("claude").display()
        )),
        "{calls}"
    );
    assert!(
        calls.contains(&format!("-e\nCODEX_HOME={}\n", s.path("codex").display())),
        "{calls}"
    );
    assert!(!s.path("PWNED").exists());
}

#[test]
fn issue50_only_newest_session_gets_pane_and_vanished_pane_never_resumes() {
    let s = Sandbox::new();
    let newest = s.session("claude_code");
    let text = fs::read_to_string(s.path("claude/projects/project/session.jsonl")).unwrap();
    let old: String = text
        .lines()
        .map(|line| {
            let mut row: serde_json::Value = serde_json::from_str(line).unwrap();
            if row.get("timestamp").is_some() {
                let mut timestamp = row["timestamp"].as_str().unwrap().to_owned();
                timestamp.replace_range(17..19, "00");
                row["timestamp"] = timestamp.into();
            }
            if row.get("sessionId").is_some() {
                row["sessionId"] = "old-id".into();
            }
            format!("{row}\n")
        })
        .collect();
    fs::write(s.path("claude/projects/project/old.jsonl"), old).unwrap();
    fs::write(
        s.path("panes"),
        pane_record(&[
            "dev",
            "2",
            "1",
            "claude",
            "999999999",
            s.path("project").to_str().unwrap(),
        ]),
    )
    .unwrap();
    let out = s.cmd().args(["list", "--all", "--json"]).output().unwrap();
    assert!(out.status.success());
    let tree: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let sessions = tree[0]["sessions"].as_array().unwrap();
    assert_eq!(sessions.iter().filter(|s| !s["pane"].is_null()).count(), 1);
    assert!(
        sessions
            .iter()
            .find(|s| s["id"] == "claude_code:old-id")
            .unwrap()["pane"]
            .is_null()
    );
    success(
        s.cmd()
            .args(["open", "claude_code:old-id"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        fs::read_to_string(s.path("args")).unwrap(),
        "--resume=old-id\n"
    );
    script(
        &s.path("bin/tmux"),
        "if [ \"$1\" = '-S' ] || [ \"$1\" = '-L' ]; then shift 2; fi\nif [ \"$1\" = list-panes ]; then cat \"$PANES\"; elif [ \"$1\" = display ]; then exit 1; else echo unexpected-jump >> \"$RECORD_TMUX\"; exit 1; fi",
    );
    fs::remove_file(s.path("args")).unwrap();
    failure(
        s.cmd().args(["open", &newest]).output().unwrap(),
        "pane no longer exists",
    );
    assert!(
        !s.path("args").exists(),
        "vanished pane must not start a second copy"
    );
    assert!(
        !fs::read_to_string(s.path("tmux-args"))
            .unwrap()
            .contains("unexpected-jump")
    );
}

#[test]
fn issue57_doctor_distinguishes_version_no_server_missing_and_names_agents() {
    for (script_text, expected) in [
        (
            "if [ \"$1\" = -V ]; then echo 'tmux 3.test'; else exit 1; fi",
            "no server",
        ),
        (
            "if [ \"$1\" = -V ]; then echo 'tmux 3.test'; else exit 0; fi",
            "tmux 3.test",
        ),
        ("exit 127", "not installed"),
    ] {
        let s = Sandbox::new();
        script(&s.path("bin/tmux"), script_text);
        for json in [false, true] {
            let mut cmd = s.cmd();
            cmd.arg("doctor");
            if json {
                cmd.arg("--json");
            }
            let out = cmd.output().unwrap();
            assert!(out.status.success());
            if json {
                let data: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
                assert_eq!(data["tmux"], expected);
                assert!(data["stores"].is_array());
            } else {
                assert!(
                    String::from_utf8_lossy(&out.stdout).contains(&format!("tmux: {expected}"))
                );
            }
        }
    }
    let s = Sandbox::new();
    let only_tmux = s.path("only-tmux");
    fs::create_dir(&only_tmux).unwrap();
    script(&only_tmux.join("tmux"), "exit 1");
    failure(
        s.cmd()
            .arg("new")
            .arg(s.path("project"))
            .env("PATH", &only_tmux)
            .output()
            .unwrap(),
        "claude: not found on PATH",
    );
}

#[test]
fn issue51_fifty_sessions_twenty_panes_scan_fast() {
    let s = Sandbox::new();
    s.session("claude_code");
    let text = fs::read_to_string(s.path("claude/projects/project/session.jsonl")).unwrap();
    for i in 0..50 {
        fs::write(
            s.path(&format!("claude/projects/project/{i}.jsonl")),
            text.replace("\"done\"", &format!("\"session-{i}\"")),
        )
        .unwrap();
    }
    fs::write(
        s.path("panes"),
        (0..20)
            .map(|i| {
                pane_record(&[
                    "dev",
                    "0",
                    &i.to_string(),
                    "bash",
                    "999999999",
                    s.path("project").to_str().unwrap(),
                ])
            })
            .collect::<String>(),
    )
    .unwrap();
    #[cfg(not(target_os = "linux"))]
    script(
        &s.path("bin/ps"),
        "echo consulted >> \"$RECORD_ARGS\"\nprintf '1 0 bash\\n'",
    );
    // Use the best of three cold processes so unrelated parallel test scheduling
    // does not count against the discovery budget. The unit test asserts exactly one process
    // snapshot; this integration budget leaves headroom for concurrent release compilation.
    let mut fastest = std::time::Duration::MAX;
    for _ in 0..3 {
        let started = std::time::Instant::now();
        success(s.cmd().args(["list", "--all", "--json"]).output().unwrap());
        fastest = fastest.min(started.elapsed());
    }
    assert!(
        fastest < std::time::Duration::from_secs(1),
        "fastest scan took {fastest:?}"
    );
    #[cfg(not(target_os = "linux"))]
    assert_eq!(
        fs::read_to_string(s.path("args")).unwrap().lines().count(),
        3
    );
}

#[test]
fn issue46_private_tmux_literal_launch_and_issue58_closed_terminal_exit() {
    let _guard = process_fixture_guard();
    let out = Command::new("python3")
        .arg("tests/tmux.py")
        .arg(env!("CARGO_BIN_EXE_sboard"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn issue56_submodule_worktree_stays_outside_superproject() {
    let s = Sandbox::new();
    s.repo();
    let superproject = s.path("super");
    fs::create_dir(&superproject).unwrap();
    common::git(&superproject, &["init", "-q"]);
    common::git(
        &superproject,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            s.path("project").to_str().unwrap(),
            "sub",
        ],
    );
    common::git(&superproject, &["commit", "-qm", "submodule"]);
    success(
        s.cmd()
            .arg("new")
            .arg(superproject.join("sub"))
            .args(["--worktree", "outside"])
            .output()
            .unwrap(),
    );
    assert!(s.path("sub-wt/outside/.git").exists());
    assert!(!s.path("super/sub-wt").exists());
}

#[test]
fn issue47_cli_resume_preserves_exact_argv_and_metacharacter_cwd() {
    for agent in ["claude_code", "codex"] {
        let s = Sandbox::new();
        let original = s.session(agent);
        let native = "Native.ID_42:part-2";
        let cwd = s.path("spaces '\" $(id) `id` ;");
        fs::rename(s.path("project"), &cwd).unwrap();
        let transcript = s.path(if agent == "codex" {
            "codex/sessions/rollout-test.jsonl"
        } else {
            "claude/projects/project/session.jsonl"
        });
        fs::write(
            &transcript,
            fs::read_to_string(&transcript)
                .unwrap()
                .replace(
                    &serde_json::to_string(&s.path("project")).unwrap(),
                    &serde_json::to_string(&cwd).unwrap(),
                )
                .replace(
                    &serde_json::to_string(original.split_once(':').unwrap().1).unwrap(),
                    &serde_json::to_string(native).unwrap(),
                ),
        )
        .unwrap();
        success(
            s.cmd()
                .args(["open", &format!("{agent}:{native}")])
                .output()
                .unwrap(),
        );
        let expected = if agent == "codex" {
            format!("resume\n--\n{native}\n")
        } else {
            format!("--resume={native}\n")
        };
        assert_eq!(fs::read_to_string(s.path("args")).unwrap(), expected);
        assert_eq!(
            fs::read_to_string(s.path("cwd"))
                .unwrap()
                .strip_suffix('\n')
                .unwrap(),
            cwd.canonicalize().unwrap().to_str().unwrap()
        );
    }
}

#[test]
fn issue78_rollback_keeps_branch_created_after_existence_check() {
    let s = Sandbox::new();
    s.repo();
    let real_git = session_board::launch::executable("git").unwrap();
    script(
        &s.path("bin/git"),
        r#"
if [ "$1" = show-ref ] && [ "$4" = refs/heads/raced ] && [ ! -e "$RACE_MARK" ]; then
    "$REAL_GIT" "$@" >/dev/null 2>&1
    result=$?
    if [ "$result" != 0 ]; then
        "$REAL_GIT" branch raced HEAD || exit 90
        : > "$RACE_MARK"
        exit 1
    fi
fi
if [ "$1" = worktree ] && [ "$2" = add ]; then
    echo add-denied >&2
    exit 1
fi
exec "$REAL_GIT" "$@"
"#,
    );
    failure(
        s.cmd()
            .arg("new")
            .arg(s.path("project"))
            .args(["--worktree", "raced"])
            .env("REAL_GIT", &real_git)
            .env("RACE_MARK", s.path("raced"))
            .output()
            .unwrap(),
        "add-denied",
    );
    assert!(
        s.path("raced").exists(),
        "concurrent branch was never created"
    );
    assert!(
        Command::new(real_git)
            .current_dir(s.path("project"))
            .args(["show-ref", "--verify", "--quiet", "refs/heads/raced"])
            .status()
            .unwrap()
            .success(),
        "rollback deleted the concurrent branch"
    );
}

#[test]
fn issue79_private_tmux_cwd_cannot_forge_pane_records() {
    let _guard = process_fixture_guard();
    let out = Command::new("python3")
        .arg("tests/tmux.py")
        .arg(env!("CARGO_BIN_EXE_sboard"))
        .arg("injection")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn issue80_private_tmux_literal_hash_session_jumps() {
    let _guard = process_fixture_guard();
    let out = Command::new("python3")
        .arg("tests/tmux.py")
        .arg(env!("CARGO_BIN_EXE_sboard"))
        .arg("hash-jump")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn issue81_private_tmux_session_removed_after_display_reports_error() {
    let _guard = process_fixture_guard();
    let out = Command::new("python3")
        .arg("tests/tmux.py")
        .arg(env!("CARGO_BIN_EXE_sboard"))
        .arg("jump-race")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn issue121_private_tmux_without_attached_client_never_resumes() {
    let output = Command::new("python3")
        .args(["tests/tmux.py", env!("CARGO_BIN_EXE_sboard"), "no-client"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
