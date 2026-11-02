#[allow(
    dead_code,
    reason = "each integration suite uses a subset of common helpers"
)]
mod common;
use session_board::{
    config::Config,
    model::Status,
    source::{Source, claude::Claude},
};
use std::{fs, process::Command};
const NOW: i64 = 1793534400;

#[test]
fn local_slash_commands_do_not_start_assistant_turns() {
    let source = Claude {
        root: "fixtures/release-blockers/claude".into(),
        now: NOW,
    };
    let scan = source.scan(&Config::default());
    assert_eq!(scan.sessions.len(), 5);
    assert!(scan.status_reasons.is_empty());
    for (id, status, title) in [
        ("exit", Status::Idle, "Real prompt."),
        ("login", Status::Idle, "login"),
        ("clear", Status::Done, "Real prompt."),
        ("compact", Status::Waiting, "Real prompt."),
        ("command-prompt", Status::Working, "Actual prompt."),
    ] {
        let s = scan
            .sessions
            .iter()
            .find(|s| s.id == format!("claude_code:{id}"))
            .unwrap();
        assert_eq!(s.status, status, "{id}");
        assert_eq!(s.title, title, "{id}");
        if id == "clear" || id == "compact" {
            assert_eq!(s.last_activity_at, NOW - 300);
        }
    }
}

fn command(root: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_sboard"));
    common::no_tmux(&mut c, root);
    c.env("HOME", root)
        .env("SBOARD_CONFIG_DIR", root.join("config"))
        .env("SBOARD_STATE_DIR", root.join("state"))
        .env("CLAUDE_CONFIG_DIR", root.join("claude"))
        .env("CODEX_HOME", root.join("codex"))
        .env("GROK_HOME", root.join("grok"))
        .env("OPENCODE_DB", root.join("opencode.db"));
    c
}

#[test]
fn doctor_explains_codex_skipped_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("codex/sessions");
    fs::create_dir_all(&store).unwrap();
    for n in 0..2 {
        fs::write(store.join(format!("rollout-subagent-{n}.jsonl")), r#"{"type":"session_meta","timestamp":"2026-11-01T11:59:00Z","payload":{"id":"subagent","cwd":"/home/user/project","source":{"subagent":{}},"thread_source":"user"}}"#).unwrap();
    }
    fs::write(store.join("rollout-empty.jsonl"), "").unwrap();
    let out = command(temp.path()).arg("doctor").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("excluded non-user provenance: 2"), "{text}");
    assert!(text.contains("empty transcript: 1"), "{text}");
}

#[test]
fn empty_agent_window_gives_the_all_hint() {
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("grok/sessions");
    fs::create_dir_all(&store).unwrap();
    let db = rusqlite::Connection::open(store.join("session_search.sqlite")).unwrap();
    db.execute_batch(include_str!("../fixtures/grok/session_search.sql"))
        .unwrap();
    db.execute("UPDATE session_docs SET updated_at=1", [])
        .unwrap();
    drop(db);
    let out = command(temp.path())
        .args(["list", "--agent", "grok"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no sessions in the last 7 days; use --all")
    );
    let out = command(temp.path())
        .args(["list", "--agent", "grok", "--all"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(!out.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&out.stderr).contains("no sessions in the last"));
}

#[test]
fn local_commands_preserve_real_stuck_turns_and_recover_sampled_completions() {
    let rows: Vec<serde_json::Value> =
        fs::read_to_string("fixtures/release-blockers/claude/clear.jsonl")
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.jsonl");
    let source = Claude {
        root: temp.path().into(),
        now: NOW,
    };
    // A local command must not erase a real pending turn or make its clock recent.
    fs::write(&path, format!("{}\n{}\n", rows[0], rows[2])).unwrap();
    let scan = source.scan(&Config::default());
    assert_eq!(scan.sessions[0].status, Status::Error);
    assert_eq!(scan.sessions[0].last_activity_at, NOW - 3600);
    assert_eq!(scan.status_reasons.len(), 1);
    // A local command at the sampled tail must not hide the previous completion.
    let ignored = |size| serde_json::json!({"type":"attachment", "text":"x".repeat(size)});
    fs::write(
        &path,
        format!(
            "{}\n{}\n{}\n{}\n{}\n",
            rows[0],
            ignored(270_000),
            rows[1],
            ignored(140_000),
            rows[2]
        ),
    )
    .unwrap();
    let scan = source.scan(&Config::default());
    assert_eq!(scan.sessions[0].status, Status::Done);
    assert_eq!(scan.sessions[0].last_assistant_text, "Ready.");
    assert_eq!(scan.sessions[0].last_activity_at, NOW - 300);
}

#[test]
fn command_message_exit_ends_turn_but_quoted_command_text_is_a_real_prompt() {
    let mut row: serde_json::Value = serde_json::from_str(
        fs::read_to_string("fixtures/release-blockers/claude/exit.jsonl")
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.jsonl");
    let source = Claude {
        root: temp.path().into(),
        now: NOW,
    };
    row["message"]["content"] = serde_json::json!("<command-message>exit</command-message>");
    fs::write(&path, format!("{row}\n")).unwrap();
    let scan = source.scan(&Config::default());
    assert_eq!(scan.sessions[0].status, Status::Idle);
    assert_eq!(scan.sessions[0].title, "exit");
    row["timestamp"] = serde_json::json!("2026-11-01T11:59:00Z");
    row["message"]["content"] =
        serde_json::json!("Explain <command-name>/exit</command-name> to me.");
    fs::write(&path, format!("{row}\n")).unwrap();
    let scan = source.scan(&Config::default());
    assert_eq!(scan.sessions[0].status, Status::Working);
    assert_eq!(
        scan.sessions[0].last_user_text,
        "Explain <command-name>/exit</command-name> to me."
    );
}

fn command_envelope_status(content: serde_json::Value, pending: bool) -> Status {
    let mut row: serde_json::Value = serde_json::from_str(
        fs::read_to_string("fixtures/release-blockers/claude/exit.jsonl")
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    row["timestamp"] = serde_json::json!("2026-11-01T11:59:00Z");
    let prefix = if pending {
        format!("{row}\n")
    } else {
        String::new()
    };
    row["message"]["content"] = content;
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("session.jsonl"),
        format!("{prefix}{row}\n"),
    )
    .unwrap();
    Claude {
        root: temp.path().into(),
        now: NOW,
    }
    .scan(&Config::default())
    .sessions[0]
        .status
}

#[test]
fn literal_command_tags_followed_by_prose_are_pending_prompts() {
    for text in [
        "<command-name>/exit</command-name> is an XML sample. Explain this tag.",
        "<command-message>exit</command-message> is an XML sample.",
        "<local-command-stdout>output</local-command-stdout> explain this output.",
    ] {
        assert_eq!(
            command_envelope_status(serde_json::json!(text), false),
            Status::Working,
            "{text}"
        );
    }
}

#[test]
fn command_tags_inside_stdout_are_opaque_and_preserve_pending_work() {
    assert_eq!(
        command_envelope_status(
            serde_json::json!(
                "<local-command-stdout>Documentation: <command-name>/exit</command-name></local-command-stdout>"
            ),
            true
        ),
        Status::Working
    );
}

#[test]
fn tool_result_records_never_count_as_local_commands() {
    for text in [
        "<command-name>/exit</command-name>",
        "<local-command-stdout><command-name>/exit</command-name></local-command-stdout>",
    ] {
        assert_eq!(
            command_envelope_status(
                serde_json::json!([
                    {"type":"text", "text":text},
                    {"type":"tool_result", "tool_use_id":"tool", "content":"result"}
                ]),
                true
            ),
            Status::Working,
            "{text}"
        );
    }
}

#[test]
fn full_native_command_envelopes_preserve_wrappers_and_args() {
    for text in [
        "<command-name>/exit</command-name><command-message>exit</command-message><command-args></command-args>",
        "<command-message>exit</command-message>\n<command-name>/exit</command-name>",
        "<command-message>exit</command-message>",
    ] {
        assert_eq!(
            command_envelope_status(serde_json::json!(text), true),
            Status::Idle,
            "{text}"
        );
    }
    assert_eq!(
        command_envelope_status(
            serde_json::json!(
                "<command-name>/compact</command-name><command-message>compact</command-message><command-args>retain <command-name>/exit</command-name></command-args>"
            ),
            true
        ),
        Status::Working
    );
}
