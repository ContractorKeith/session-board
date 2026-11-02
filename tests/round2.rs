mod common;
use serde_json::{Value, json};
use session_board::{
    config::Config,
    model::Status,
    source::{Source, SourceScan, claude::Claude, codex::Codex},
};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};
const NOW: i64 = 1793534400;
const RECENT: &str = "2026-11-01T11:59:55Z";
fn claude_record(kind: &str, content: Value) -> Value {
    json!({"type":kind,"sessionId":"session","cwd":"/home/user/project","timestamp":RECENT,"message":{"content":content}})
}
fn meta() -> Value {
    json!({"timestamp":RECENT,"type":"session_meta","payload":{"id":"session","cwd":"/home/user/project","thread_source":"user","source":"cli"}})
}
fn event(kind: &str) -> Value {
    json!({"timestamp":RECENT,"type":"event_msg","payload":{"type":kind}})
}
fn response(kind: &str) -> Value {
    json!({"timestamp":RECENT,"type":"response_item","payload":{"type":kind,"call_id":"call"}})
}
fn encode(rows: &[Value]) -> String {
    rows.iter().map(|v| format!("{v}\n")).collect()
}
fn claude_text(text: &str) -> SourceScan {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("project")).unwrap();
    fs::write(temp.path().join("project/session.jsonl"), text).unwrap();
    Claude {
        root: temp.path().into(),
        now: NOW,
    }
    .scan(&Config::default())
}
fn claude(rows: &[Value]) -> SourceScan {
    claude_text(&encode(rows))
}
fn codex_text(text: &str) -> SourceScan {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("rollout-session.jsonl"), text).unwrap();
    Codex {
        root: temp.path().into(),
        index: temp.path().join("index"),
        now: NOW,
    }
    .scan(&Config::default())
}
fn codex(rows: &[Value]) -> SourceScan {
    codex_text(&encode(rows))
}
fn ended(text: &str) -> Value {
    let mut v = claude_record("assistant", json!([{"type":"text","text":text}]));
    v["message"]["stop_reason"] = json!("end_turn");
    v
}
fn tool() -> Value {
    let mut v = claude_record(
        "assistant",
        json!([{"type":"tool_use","id":"call","name":"Read","input":{}}]),
    );
    v["message"]["stop_reason"] = json!("tool_use");
    v
}
fn result(error: bool) -> Value {
    claude_record(
        "user",
        json!([{"type":"tool_result","tool_use_id":"call","is_error":error,"content":"Result placeholder"}]),
    )
}
fn command(temp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sboard"));
    cmd.env("SBOARD_STATE_DIR", temp.join("state"))
        .env("SBOARD_CONFIG_DIR", temp.join("config"))
        .env("CLAUDE_CONFIG_DIR", temp.join("claude"))
        .env("CODEX_HOME", temp.join("codex"));
    cmd
}
#[test]
fn issue21_codex_provenance_and_only_first_meta() {
    let mut first = meta();
    first["payload"]
        .as_object_mut()
        .unwrap()
        .remove("thread_source");
    first["payload"]["source"] = json!("vscode");
    assert_eq!(codex(&[first.clone()]).sessions.len(), 1);
    let mut second = meta();
    second["payload"]["id"] = json!("parent");
    second["payload"]["cwd"] = json!("/home/user/parent");
    second["payload"]["thread_source"] = json!("guardian_review");
    let scanned = codex(&[first.clone(), second]);
    assert_eq!(scanned.sessions[0].id, "codex:session");
    assert_eq!(scanned.sessions[0].cwd, Path::new("/home/user/project"));
    first["payload"]["source"] = json!({"subagent":{}});
    assert_eq!(codex(&[first.clone(), meta()]).skipped_sessions, 1);
    first["payload"]["thread_source"] = json!("user");
    assert!(
        codex(&[first.clone()]).sessions.is_empty(),
        "object source is never owner provenance"
    );
    first["payload"]["source"] = json!("cli");
    first["payload"]["thread_source"] = json!("guardian_review");
    assert!(codex(&[first]).sessions.is_empty());
}
#[test]
fn issue22_task_state_survives_gap_until_task_event() {
    let mut first = meta();
    first["payload"]["instructions"] = json!("placeholder ".repeat(6000));
    let padding = json!({"type":"ignored","padding":"p".repeat(400_000)});
    let rows = [
        first,
        event("task_started"),
        padding,
        response("custom_tool_call"),
        response("custom_tool_call_output"),
    ];
    let text = encode(&rows).replace(RECENT, "2026-11-01T11:59:40Z");
    let scanned = codex_text(&text);
    assert_eq!(scanned.sessions[0].status, Status::Working);
    let stale = text.replace("2026-11-01T11:59:40Z", "2026-11-01T11:20:00Z");
    assert_eq!(codex_text(&stale).sessions[0].status, Status::Error);
    for (event_name, expected) in [
        ("task_complete", Status::Done),
        ("turn_aborted", Status::Idle),
    ] {
        assert_eq!(
            codex_text(&(text.clone() + &encode(&[event(event_name)]))).sessions[0].status,
            expected
        );
    }
}
#[test]
fn issue23_large_final_record_and_exact_tail_boundary() {
    let mut old = claude_record("user", json!("User placeholder"));
    old["timestamp"] = json!("2026-10-24T12:00:00Z");
    let prefix = encode(&[
        old,
        json!({"type":"attachment","padding":"p".repeat(400_000)}),
    ]);
    for size in [65535, 65536, 65537, 100_000, 131071, 131072, 131073] {
        let mut last = tool();
        last["padding"] = json!("");
        let base = format!("{last}\n").len();
        last["padding"] = json!("p".repeat(size - base));
        let text = prefix.clone() + &format!("{last}\n");
        let scanned = claude_text(&text);
        assert_eq!(scanned.sessions.len(), 1, "last record size {size}");
        assert_eq!(
            scanned.sessions[0].status,
            Status::Working,
            "last record size {size}"
        );
        assert_eq!(scanned.sessions[0].last_activity_at, NOW - 5);
    }
    // First complete tail line is significant even when followed by ignored lines.
    let complete = format!("{}\n", tool());
    let padding_len = 65536 - complete.len();
    let ignored = json!({"type":"attachment","padding":""});
    let base = format!("{ignored}\n").len();
    let tail = complete
        + &format!(
            "{}\n",
            json!({"type":"attachment","padding":"p".repeat(padding_len-base)})
        );
    assert_eq!(tail.len(), 65536);
    assert_eq!(
        claude_text(&(prefix + &tail)).sessions[0].status,
        Status::Working
    );
}
#[test]
fn issue24_claude_user_records_mean_generating_except_interruption() {
    for rows in [
        vec![claude_record("user", json!("User placeholder"))],
        vec![tool(), result(false)],
    ] {
        assert_eq!(claude(&rows).sessions[0].status, Status::Working);
    }
    assert_eq!(
        claude(&[
            tool(),
            claude_record("user", json!("[Request interrupted by user for tool use]"))
        ])
        .sessions[0]
            .status,
        Status::Idle
    );
}
#[test]
fn issue25_only_api_errors_and_codex_error_clears_on_new_task() {
    assert_eq!(
        claude(&[tool(), result(true), tool(), result(false)]).sessions[0].status,
        Status::Working
    );
    let mut error = claude_record("assistant", json!("API error placeholder"));
    error["isApiErrorMessage"] = json!(true);
    assert_eq!(claude(&[error.clone()]).sessions[0].status, Status::Error);
    assert_eq!(
        claude(&[
            error.clone(),
            claude_record("user", json!("[Request interrupted by user]"))
        ])
        .sessions[0]
            .status,
        Status::Idle
    );
    error.as_object_mut().unwrap().remove("isApiErrorMessage");
    error["error"] = json!({"message":"Error placeholder"});
    assert_eq!(claude(&[error]).sessions[0].status, Status::Error);
    assert_eq!(
        codex(&[meta(), event("error")]).sessions[0].status,
        Status::Error
    );
    assert_eq!(
        codex(&[
            meta(),
            event("error"),
            event("task_started"),
            response("custom_tool_call_output")
        ])
        .sessions[0]
            .status,
        Status::Working
    );
    let mut output = response("custom_tool_call_output");
    output["payload"]["output"] = json!("Process exited with code 1");
    assert_eq!(codex(&[meta(), output]).sessions[0].status, Status::Idle);
}
#[test]
fn issue26_list_survives_closed_stderr_with_many_warnings() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("claude/projects")).unwrap();
    fs::create_dir(temp.path().join("config")).unwrap();
    let config = (0..3000)
        .map(|i| format!("unknown_{i} = true\n"))
        .collect::<String>();
    fs::write(temp.path().join("config/config.toml"), config).unwrap();
    let mut child = command(temp.path())
        .args(["list", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stderr.take());
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{:?}", out.status);
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!([])
    );
}
#[test]
fn issue27_codex_aborted_turn_clears_task_and_calls() {
    let rows = vec![
        meta(),
        event("task_started"),
        response("custom_tool_call"),
        event("turn_aborted"),
    ];
    assert_eq!(codex(&rows).sessions[0].status, Status::Idle);
    let mut with_text = rows;
    with_text.last_mut().unwrap()["payload"]["last_agent_message"] = json!("Assistant placeholder");
    assert_eq!(codex(&with_text).sessions[0].status, Status::Done);
}
#[test]
fn issue28_titles_prioritize_owner_text_and_strip_terminal_controls() {
    let mut boiler = response("message");
    boiler["payload"]["role"] = json!("user");
    boiler["payload"]["content"] = json!([{"type":"input_text","text":"# AGENTS.md instructions"}]);
    let mut fallback = boiler.clone();
    fallback["payload"]["content"][0]["text"] = json!("Fallback placeholder");
    let mut real = event("user_message");
    real["payload"]["message"] = json!("\u{1b}]0;x\u{7}\nsecond");
    let scanned = codex(&[meta(), boiler, fallback, real]);
    assert_eq!(scanned.sessions[0].title, "x second");
    assert_eq!(
        scanned.sessions[0].last_user_text,
        "\u{1b}]0;x\u{7}\nsecond"
    );
    let mut injected = claude_record("user", json!("Injected placeholder"));
    injected["isMeta"] = json!(true);
    let rows = [
        injected,
        claude_record("user", json!("<local-command-caveat>placeholder")),
        claude_record("user", json!("User\nplaceholder\u{7}")),
    ];
    let scanned = claude(&rows);
    assert_eq!(scanned.sessions[0].title, "User placeholder");
    assert_eq!(scanned.sessions[0].last_user_text, "User\nplaceholder\u{7}");
    let mut projects = session_board::scan::scan(
        &vec![Box::new(Claude {
            root: "fixtures/claude_code".into(),
            now: NOW,
        })],
        &Config::default(),
        &Default::default(),
        Path::to_path_buf,
    )
    .projects;
    projects[0].sessions = vec![scanned.sessions[0].clone()];
    let mut out = vec![];
    session_board::cli::list(&mut out, &projects, false).unwrap();
    assert_eq!(String::from_utf8(out).unwrap().lines().count(), 2);
}
#[test]
fn issue29_thinking_only_ended_turn_does_not_reuse_previous_question() {
    let mut thinking = claude_record(
        "assistant",
        json!([{"type":"thinking","thinking":"Thinking placeholder"}]),
    );
    thinking["message"]["stop_reason"] = json!("end_turn");
    assert_eq!(
        claude(&[
            ended("Decision placeholder?"),
            claude_record("user", json!("Answer placeholder")),
            thinking
        ])
        .sessions[0]
            .status,
        Status::Done
    );
}
#[test]
fn issue31_unimplemented_agents_and_no_store_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    for agent in ["grok", "opencode"] {
        let out = command(temp.path())
            .args(["list", "--agent", agent])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8(out.stderr).unwrap().contains("M6"));
    }
    fs::create_dir(temp.path().join("config")).unwrap();
    fs::write(
        temp.path().join("config/config.toml"),
        "[agents]\nclaude_code = false\n",
    )
    .unwrap();
    let out = command(temp.path())
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8(out.stderr).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(
        text.contains("claude_code")
            && text.contains("disabled")
            && text.contains("codex/sessions")
    );
}
#[test]
fn issue32_first_cwd_wins_for_both_agents() {
    let mut later = ended("Assistant placeholder");
    later["cwd"] = json!("/home/user/other");
    let s = claude(&[claude_record("user", json!("User placeholder")), later]);
    assert_eq!(s.sessions[0].cwd, Path::new("/home/user/project"));
    let context =
        json!({"timestamp":RECENT,"type":"turn_context","payload":{"cwd":"/home/user/other"}});
    assert_eq!(
        codex(&[meta(), context]).sessions[0].cwd,
        Path::new("/home/user/project")
    );
}
#[test]
fn round2_ended_reasons_and_thirty_minute_threshold() {
    for reason in ["max_tokens", "stop_sequence"] {
        let mut v = ended("Assistant placeholder");
        v["message"]["stop_reason"] = json!(reason);
        assert_eq!(claude(&[v]).sessions[0].status, Status::Done);
    }
    assert_eq!(session_board::status::WORKING_SECONDS, 30 * 60);
    let text = encode(&[tool()]).replace(RECENT, "2026-11-01T11:40:00Z");
    assert_eq!(claude_text(&text).sessions[0].status, Status::Working);
}

#[test]
fn issue30_source_git_test_preserves_unrelated_repository() {
    let temp = tempfile::tempdir().unwrap();
    let other = temp.path().join("other");
    fs::create_dir(&other).unwrap();
    common::git(&other, &["init", "-q"]);
    let config = other.join(".git/config");
    let before = (
        fs::read(&config).unwrap(),
        fs::metadata(&config).unwrap().modified().unwrap(),
    );
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "issue30_git_probe"])
        .env("GIT_DIR", other.join(".git"))
        .env("SBOARD_TEST_GIT_PROJECT", &project)
        .output()
        .unwrap();
    assert!(project.join(".git").is_dir());
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        (
            fs::read(&config).unwrap(),
            fs::metadata(&config).unwrap().modified().unwrap()
        ),
        before,
        "test git init mutated an unrelated repository"
    );
}

#[test]
#[ignore = "executed by issue30_source_git_test_preserves_unrelated_repository"]
fn issue30_git_probe() {
    common::git(
        Path::new(&std::env::var_os("SBOARD_TEST_GIT_PROJECT").unwrap()),
        &["init", "-q"],
    );
}

#[test]
fn project_filter_accepts_subdirectory_and_invalid_roots_warn() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("subdir")).unwrap();
    common::git(&repo, &["init", "-q"]);
    fs::create_dir_all(temp.path().join("claude/projects/project")).unwrap();
    let mut row = ended("Assistant placeholder");
    row["cwd"] = json!(repo);
    fs::write(
        temp.path().join("claude/projects/project/session.jsonl"),
        encode(&[row]),
    )
    .unwrap();
    let out = command(temp.path())
        .args(["list", "--json", "--all", "--project"])
        .arg(repo.join("subdir"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let p: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(p[0]["sessions"].as_array().unwrap().len(), 1);
    fs::create_dir(temp.path().join("config")).unwrap();
    fs::write(
        temp.path().join("config/config.toml"),
        "roots = ['~user/x', 'relative', '']
",
    )
    .unwrap();
    let out = command(temp.path())
        .args(["list", "--json", "--all"])
        .output()
        .unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(
        stderr
            .lines()
            .filter(|l| l.contains("invalid config root"))
            .count(),
        3,
        "{stderr}"
    );
}

#[test]
fn issue28_head_reaches_owner_prompt_and_index_titles_are_sanitized() {
    let mut first = meta();
    first["payload"]["instructions"] = json!("p".repeat(100_000));
    let mut user = event("user_message");
    user["payload"]["message"] = json!("Owner placeholder");
    let text = encode(&[
        first,
        user,
        json!({"type":"ignored","padding":"p".repeat(400_000)}),
        event("task_complete"),
    ]);
    assert_eq!(codex_text(&text).sessions[0].title, "Owner placeholder");
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("rollout-session.jsonl"), encode(&[meta()])).unwrap();
    let index = temp.path().join("index");
    fs::write(
        &index,
        format!(
            "{}\n",
            json!({"id":"session","thread_name":"\u{1b}]0;x\u{7}\nsecond\u{7f}"})
        ),
    )
    .unwrap();
    assert_eq!(
        Codex {
            root: temp.path().into(),
            index,
            now: NOW
        }
        .scan(&Config::default())
        .sessions[0]
            .title,
        "x second"
    );
}

#[test]
fn issue23_fallback_limit_skips_instead_of_listing_old_head() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("project")).unwrap();
    let path = temp.path().join("project/session.jsonl");
    fs::write(&path, encode(&[tool()])).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(70 * 1024 * 1024)
        .unwrap();
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    std::io::Write::write_all(&mut file, b"\n{\"type\":\"attachment\"}\n").unwrap();
    let scanned = Claude {
        root: temp.path().into(),
        now: NOW,
    }
    .scan(&Config::default());
    assert!(scanned.sessions.is_empty());
    assert_eq!(scanned.skipped_sessions, 1);
    assert!(scanned.warnings[0].contains("64 MiB"));
}

#[test]
fn issue23_total_file_sizes_around_head_and_tail_budgets() {
    let prefix = encode(&[claude_record("user", json!("User placeholder"))]);
    for size in [
        65535, 65536, 65537, 131071, 131072, 131073, 262143, 262144, 262145,
    ] {
        let mut last = tool();
        last["padding"] = json!("");
        let remaining = size - prefix.len() - format!("{last}\n").len();
        last["padding"] = json!("p".repeat(remaining));
        let text = prefix.clone() + &format!("{last}\n");
        assert_eq!(text.len(), size);
        assert_eq!(
            claude_text(&text).sessions[0].status,
            Status::Working,
            "file size {size}"
        );
    }
}

#[test]
fn round3_straddling_message_is_kept_before_complete_ignored_records() {
    let mut old = claude_record("user", json!("Old placeholder"));
    old["timestamp"] = json!("2026-10-24T12:00:00Z");
    let text = encode(&[
        old,
        json!({"type":"attachment","padding":"p".repeat(400_000)}),
        claude_record("user", json!("p".repeat(100_000))),
        json!({"type":"attachment"}),
    ]);
    let scanned = claude_text(&text);
    assert_eq!(
        scanned.sessions.len(),
        1,
        "fresh message must remain inside default window"
    );
    assert_eq!(scanned.sessions[0].status, Status::Working);
    assert_eq!(scanned.sessions[0].last_activity_at, NOW - 5);
    assert_eq!(
        scanned.skipped_lines, 0,
        "backward alignment must not parse a fragment"
    );
}

#[test]
fn round3_native_ids_are_printable_and_empty_ids_are_counted() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("claude/projects/project")).unwrap();
    fs::create_dir_all(temp.path().join("codex/sessions")).unwrap();
    let claude_path = temp.path().join("claude/projects/project/session.jsonl");
    let codex_path = temp.path().join("codex/sessions/rollout-session.jsonl");
    for id in ["evil\nFORGED\u{1b}[2J", "\u{1}\u{7}\u{1b}[31m"] {
        let mut c = ended("Assistant placeholder");
        c["sessionId"] = json!(id);
        let mut m = meta();
        m["payload"]["id"] = json!(id);
        fs::write(&claude_path, encode(&[c])).unwrap();
        fs::write(&codex_path, encode(&[m])).unwrap();
        if id.starts_with("evil") {
            let output = command(temp.path())
                .args(["list", "--json", "--all"])
                .output()
                .unwrap();
            assert!(output.status.success());
            let projects: Value = serde_json::from_slice(&output.stdout).unwrap();
            let sessions = projects[0]["sessions"].as_array().unwrap();
            assert_eq!(sessions.len(), 2);
            for session in sessions {
                let agent = session["agent"].as_str().unwrap();
                assert_eq!(session["id"], format!("{agent}:evil FORGED"));
            }
            let output = command(temp.path())
                .args(["list", "--all"])
                .output()
                .unwrap();
            let text = String::from_utf8(output.stdout).unwrap();
            assert_eq!(
                text.lines().count(),
                3,
                "one header and two session rows: {text:?}"
            );
            assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
        } else {
            let c = Claude {
                root: temp.path().join("claude/projects"),
                now: NOW,
            }
            .scan(&Config::default());
            assert!(c.sessions.is_empty());
            assert_eq!(c.skipped_sessions, 1);
            let c = Codex {
                root: temp.path().join("codex/sessions"),
                index: temp.path().join("missing"),
                now: NOW,
            }
            .scan(&Config::default());
            assert!(c.sessions.is_empty());
            assert_eq!(c.skipped_sessions, 1);
        }
    }
}

#[test]
fn round3_project_filter_accepts_a_file_inside_the_project() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("nested")).unwrap();
    common::git(&repo, &["init", "-q"]);
    let file = repo.join("nested/file.txt");
    fs::write(&file, "placeholder").unwrap();
    fs::create_dir_all(temp.path().join("claude/projects/project")).unwrap();
    let mut row = ended("Assistant placeholder");
    row["cwd"] = json!(repo);
    fs::write(
        temp.path().join("claude/projects/project/session.jsonl"),
        encode(&[row]),
    )
    .unwrap();
    let output = command(temp.path())
        .args(["list", "--json", "--all", "--project"])
        .arg(file)
        .output()
        .unwrap();
    assert!(output.status.success());
    let projects: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(projects.as_array().unwrap().len(), 1);
}

#[test]
fn round3_signed_timestamp_fields_are_skipped_lines() {
    let invalid = [
        "2026-11-01T+1:00:00Z",
        "2026-+1-01T12:00:00Z",
        "2026-11-01T12:00:00+00:-1",
    ];
    let mut c = vec![ended("Assistant placeholder")];
    let mut x = vec![meta()];
    for time in invalid {
        let mut row = claude_record("user", json!("Placeholder"));
        row["timestamp"] = json!(time);
        c.push(row);
        let mut row = event("task_started");
        row["timestamp"] = json!(time);
        x.push(row);
    }
    let scanned = claude(&c);
    assert_eq!(scanned.skipped_lines, 3);
    assert_eq!(scanned.sessions[0].last_activity_at, NOW - 5);
    assert_eq!(scanned.sessions[0].status, Status::Done);
    let scanned = codex(&x);
    assert_eq!(scanned.skipped_lines, 3);
    assert_eq!(scanned.sessions[0].last_activity_at, NOW - 5);
    assert_eq!(scanned.sessions[0].status, Status::Idle);
}

#[test]
fn round3_quoted_interruption_in_user_prose_means_working() {
    assert_eq!(
        claude(&[claude_record(
            "user",
            json!("Explain why the log says [Request interrupted by user] and fix it")
        )])
        .sessions[0]
            .status,
        Status::Working
    );
    assert_eq!(
        claude(&[claude_record(
            "user",
            json!("[Request interrupted by user for tool use]")
        )])
        .sessions[0]
            .status,
        Status::Idle
    );
}

#[cfg(unix)]
fn bounded_output(mut command: Command) -> std::process::Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("command blocked on a special file: {output:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
#[cfg(unix)]
fn fifo(path: &Path) {
    assert!(Command::new("mkfifo").arg(path).status().unwrap().success());
}

#[test]
#[cfg(unix)]
fn round3_fifo_title_index_never_blocks_list_or_doctor() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("codex/sessions")).unwrap();
    let path = temp.path().join("codex/session_index.jsonl");
    fifo(&path);
    for args in [["list", "--json"], ["doctor", "--json"]] {
        let mut cmd = command(temp.path());
        cmd.args(args);
        let output = bounded_output(cmd);
        assert!(output.status.success());
        if args[0] == "list" {
            assert!(
                String::from_utf8(output.stderr)
                    .unwrap()
                    .contains(path.to_str().unwrap())
            );
        } else {
            let stores: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(stores[1]["skipped_stores"], 1);
            assert!(
                stores[1]["warnings"][0]
                    .as_str()
                    .unwrap()
                    .contains(path.to_str().unwrap())
            );
        }
    }
}

#[test]
#[cfg(unix)]
fn round3_fifo_config_and_state_never_block_list() {
    for which in ["config", "state"] {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("claude/projects")).unwrap();
        fs::create_dir(temp.path().join(which)).unwrap();
        let path = temp.path().join(which).join(format!("{which}.toml"));
        fifo(&path);
        let mut cmd = command(temp.path());
        cmd.args(["list", "--json"]);
        let output = bounded_output(cmd);
        assert_eq!(
            output.status.code(),
            Some(if which == "config" { 1 } else { 0 })
        );
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains(path.to_str().unwrap())
        );
    }
}

#[test]
fn round3_control_only_index_title_falls_through_to_owner_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let mut user = event("user_message");
    user["payload"]["message"] = json!("Owner placeholder");
    fs::write(
        temp.path().join("rollout-session.jsonl"),
        encode(&[meta(), user]),
    )
    .unwrap();
    let index = temp.path().join("session_index.jsonl");
    fs::write(
        &index,
        encode(&[json!({"id":"session","thread_name":"\u{1}\u{7}\u{1b}[31m"})]),
    )
    .unwrap();
    let scanned = Codex {
        root: temp.path().into(),
        index,
        now: NOW,
    }
    .scan(&Config::default());
    assert_eq!(scanned.sessions[0].title, "Owner placeholder");
}

#[test]
fn round4_text_previews_are_bounded_utf8_tails_for_both_sources() {
    let text = format!("prefix{}THE END?", "界😀\t".repeat(3000));
    let mut user = claude_record("user", json!(text));
    let c = claude(&[user.clone(), ended(&text)]);
    let x = codex(&[
        meta(),
        json!({"timestamp": RECENT, "type":"event_msg", "payload":{"type":"user_message", "message":text}}),
        json!({"timestamp": RECENT, "type":"event_msg", "payload":{"type":"task_complete", "last_agent_message":text}}),
    ]);
    for scanned in [c, x] {
        let s = &scanned.sessions[0];
        for preview in [&s.last_user_text, &s.last_assistant_text] {
            assert!(preview.len() <= 8192);
            assert!(preview.ends_with("THE END?"));
            assert!(!preview.starts_with("prefix"));
        }
        assert_eq!(s.status, Status::Waiting);
    }
    user["message"]["content"] = json!("left\u{202e}right");
    assert!(!claude(&[user]).sessions[0].title.contains('\u{202e}'));
}

#[test]
fn round4_doctor_counts_duplicate_ids() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("claude/projects/project");
    fs::create_dir_all(&root).unwrap();
    for (name, text) in [("one", "first"), ("two", "second")] {
        fs::write(root.join(format!("{name}.jsonl")), encode(&[ended(text)])).unwrap();
    }
    let output = command(temp.path())
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stores: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stores[0]["duplicate_ids"], 1);
    assert_eq!(stores[1]["duplicate_ids"], 0);
}

#[cfg(unix)]
#[test]
fn round4_git_fifo_is_killed_reaped_and_warned() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    common::git(&repo, &["init", "-q"]);
    fs::remove_file(repo.join(".git/HEAD")).unwrap();
    fifo(&repo.join(".git/HEAD"));
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", "round4_git_fifo_probe"])
        .env("SBOARD_TEST_FIFO_REPO", &repo);
    let output = bounded_output(command);
    assert!(output.status.success(), "{output:?}");
}

#[cfg(unix)]
#[test]
#[ignore = "run in isolation by round4_git_fifo_is_killed_reaped_and_warned"]
fn round4_git_fifo_probe() {
    let repo = std::path::PathBuf::from(std::env::var_os("SBOARD_TEST_FIFO_REPO").unwrap());
    let started = std::time::Instant::now();
    let mut roots = session_board::project::ProjectRoots::default();
    assert_eq!(roots.resolve(&repo), fs::canonicalize(&repo).unwrap());
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    assert!(
        roots
            .take_warnings()
            .iter()
            .any(|w| w.contains("timed out"))
    );
    let output = Command::new("pgrep")
        .args(["-P", &std::process::id().to_string(), "git"])
        .output()
        .unwrap();
    assert!(output.stdout.is_empty(), "git child survived: {output:?}");
}
