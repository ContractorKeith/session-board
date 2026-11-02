use session_board::{
    attention::attention,
    cli::{self, StatusFilter},
    config::Config,
    notify::{Notifier, Recording, Transitions},
    scan,
    source::{Registry, claude::Claude},
    state::State,
};
const NOW: i64 = 1_793_534_400;
fn projects(root: &std::path::Path) -> Vec<session_board::model::Project> {
    let registry: Registry = vec![Box::new(Claude {
        root: root.into(),
        now: NOW,
    })];
    scan::scan(&registry, &Config::default(), &State::default(), |cwd| {
        cwd.to_owned()
    })
    .projects
}
#[test]
fn attention_matches_cli_fixture_order_and_read_marks() {
    let projects = projects(std::path::Path::new("fixtures/claude_code"));
    let mut state = State::default();
    for project in &projects {
        for session in &project.sessions {
            if session.status == session_board::model::Status::Done {
                state
                    .read
                    .insert(session.id.clone(), session.last_activity_at);
            }
        }
    }
    let expected = [
        "claude_code:waiting",
        "claude_code:error",
        "claude_code:stuck",
    ]
    .map(str::to_owned);
    assert_eq!(
        attention(&projects, &state)
            .iter()
            .map(|i| i.session.id.clone())
            .collect::<Vec<_>>(),
        expected
    );
    let mut filtered = projects.clone();
    cli::filter(
        &mut filtered,
        &state,
        None,
        Some(StatusFilter::Attention),
        None,
        false,
    );
    let actual: Vec<_> = filtered
        .iter()
        .flat_map(|p| &p.sessions)
        .map(|s| s.id.clone())
        .collect();
    assert_eq!(actual, expected);
    assert!(
        attention(&projects, &state)
            .iter()
            .all(|i| i.session.status != session_board::model::Status::Done)
    );
}
#[test]
fn edited_fixtures_notify_once_per_event_and_never_on_startup_or_disabled() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("project")).unwrap();
    let mut transitions = Transitions::default();
    let mut disabled = Transitions::default();
    let mut recording = Recording::default();
    let mut muted = Recording::default();
    for (fixture, count) in [
        ("working", 0),
        ("waiting", 1),
        ("waiting", 1),
        ("done", 2),
        ("error", 3),
        ("working", 3),
        ("waiting", 3),
    ] {
        std::fs::copy(
            format!("fixtures/claude_code/project/{fixture}.jsonl"),
            temp.path().join("project/session.jsonl"),
        )
        .unwrap();
        let mut projects = projects(temp.path());
        for p in &mut projects {
            for s in &mut p.sessions {
                s.id = "claude_code:stable".into();
            }
        }
        transitions.scan(&projects, true, &mut recording);
        disabled.scan(&projects, false, &mut muted);
        assert_eq!(recording.calls.len(), count, "{fixture}");
    }
    assert_eq!(recording.calls.len(), 3);
    assert!(recording.calls[0].0.ends_with("waiting"));
    assert!(recording.calls[1].0.ends_with("done"));
    assert!(recording.calls[2].0.ends_with("error"));
    assert!(muted.calls.is_empty());
}
#[test]
#[ignore]
fn command_probe() {
    let title = "title \"quoted\"; $(touch BAD)";
    let body = "body\nsecond line \\";
    session_board::notify::NotifySend
        .notify(title, body)
        .unwrap();
    session_board::notify::Osascript
        .notify(title, body)
        .unwrap();
    let mut desktop = session_board::notify::Delivery::desktop().unwrap();
    // Missing command falls back to Noop and warns once.
    std::fs::remove_file(std::env::var_os("PROBE_REMOVE").unwrap()).unwrap();
    desktop.notify(title, body).unwrap();
    desktop.notify(title, body).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut warnings = vec![];
    while warnings.is_empty() && std::time::Instant::now() < deadline {
        warnings.extend(desktop.take_warnings());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("desktop notification:"));
    drop(desktop);
}
#[test]
#[cfg(unix)]
fn fixed_commands_preserve_arguments_and_missing_command_warns_once() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("commands");
    for name in ["notify-send", "osascript"] {
        let path = temp.path().join(name);
        std::fs::write(
            &path,
            "#!/bin/sh\nprintf '%s\\n' \"$0\" \"$@\" >> \"$PROBE_LOG\"\n",
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(target_os = "macos")]
    let removed = "osascript";
    #[cfg(not(target_os = "macos"))]
    let removed = "notify-send";
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "command_probe", "--nocapture"])
        .env("PATH", temp.path())
        .env("PROBE_LOG", &log)
        .env("PROBE_REMOVE", temp.path().join(removed))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = std::fs::read_to_string(log).unwrap();
    assert!(
        actual.contains(
            "--app-name\nsboard\n--\ntitle \"quoted\"; $(touch BAD)\nbody second line \\\n"
        )
    );
    assert!(actual.contains("on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run\n--\ntitle"));
}

#[test]
fn existing_attention_states_are_silent_and_disabled_scans_update_baseline() {
    let projects = projects(std::path::Path::new("fixtures/claude_code"));
    let mut recording = Recording::default();
    let mut transitions = Transitions::default();
    transitions.scan(&projects, true, &mut recording);
    transitions.scan(&projects, true, &mut recording);
    assert!(recording.calls.is_empty());
    let mut changed = projects.clone();
    for p in &mut changed {
        for s in &mut p.sessions {
            s.status = session_board::model::Status::Waiting;
        }
    }
    transitions.scan(&changed, false, &mut recording);
    transitions.scan(&changed, true, &mut recording);
    assert!(recording.calls.is_empty());
}

#[test]
fn sessions_sharing_a_store_path_keep_independent_membership() {
    let mut projects = projects(std::path::Path::new("fixtures/claude_code"));
    let mut waiting = projects[0].sessions[0].clone();
    waiting.id = "opencode:waiting".into();
    waiting.status = session_board::model::Status::Waiting;
    waiting.transcript_path = "shared.db".into();
    let mut idle = waiting.clone();
    idle.id = "opencode:idle".into();
    idle.status = session_board::model::Status::Idle;
    projects[0].sessions = vec![waiting, idle];
    cli::filter(
        &mut projects,
        &State::default(),
        None,
        Some(StatusFilter::Attention),
        None,
        false,
    );
    assert_eq!(projects[0].sessions.len(), 1);
    assert_eq!(projects[0].sessions[0].id, "opencode:waiting");
}

#[test]
fn old_first_seen_same_status_activity_and_duplicate_ids() {
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    let mut s = p[0].sessions[0].clone();
    s.status = session_board::model::Status::Waiting;
    s.last_activity_at = NOW - 3 * 86400;
    p[0].sessions = vec![s.clone()];
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    tracker.scan(&[], true, &mut record);
    tracker.scan(&p, true, &mut record);
    assert!(
        record.calls.is_empty(),
        "old sessions first seen after startup must stay silent"
    );
    p[0].sessions[0].status = session_board::model::Status::Done;
    tracker.scan(&p, true, &mut record);
    record.calls.clear();
    p[0].sessions[0].last_activity_at += 1;
    tracker.scan(&p, true, &mut record);
    assert_eq!(record.calls.len(), 1, "new done turn must notify");
    let mut duplicate = s.clone();
    duplicate.transcript_path = "other-copy.jsonl".into();
    duplicate.status = session_board::model::Status::Error;
    p[0].sessions = vec![s, duplicate];
    let mut tracker = Transitions::default();
    record.calls.clear();
    tracker.scan(&p, true, &mut record);
    tracker.scan(&p, true, &mut record);
    assert!(
        record.calls.is_empty(),
        "unchanged copies sharing an id must not notify"
    );
}

#[test]
fn global_attention_order_is_independent_of_project_grouping() {
    let p: Vec<session_board::model::Project> =
        serde_json::from_str(include_str!("fixtures/attention-projects.json")).unwrap();
    let expected = ["b-wait", "g-wait", "a-wait", "g-error", "b-error", "a-done"];
    let actual: Vec<_> = attention(
        &p,
        &State {
            first_seen: Some(0),
            ..Default::default()
        },
    )
    .iter()
    .map(|i| i.session.id.as_str())
    .collect();
    assert_eq!(actual, expected);
}

#[test]
fn burst_of_300_transitions_sends_one_summary() {
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    let sample = p[0].sessions[0].clone();
    p[0].sessions = (0..300)
        .map(|n| {
            let mut s = sample.clone();
            s.id = format!("burst:{n}");
            s.transcript_path = format!("burst-{n}").into();
            s.status = session_board::model::Status::Working;
            s
        })
        .collect();
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    tracker.scan(&p, true, &mut record);
    for s in &mut p[0].sessions {
        s.status = session_board::model::Status::Waiting;
    }
    tracker.scan(&p, true, &mut record);
    assert_eq!(record.calls.len(), 1);
    assert_eq!(record.calls[0].0, "sboard: 300 sessions need you");
}

#[test]
fn notification_titles_sanitise_project_names_and_limit_text() {
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    p[0].sessions.truncate(1);
    p[0].name = "bad\n\u{1b}[31mproject\u{1b}[0m".into();
    p[0].sessions[0].status = session_board::model::Status::Working;
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    tracker.scan(&p, true, &mut record);
    p[0].sessions[0].status = session_board::model::Status::Waiting;
    tracker.scan(&p, true, &mut record);
    assert_eq!(record.calls[0].0, "sboard: bad project — waiting");
    p[0].name = "界".repeat(300);
    p[0].sessions[0].title = "😀".repeat(300);
    p[0].sessions[0].status = session_board::model::Status::Done;
    tracker.scan(&p, true, &mut record);
    assert_eq!(record.calls[1].0.chars().count(), 200);
    assert_eq!(record.calls[1].1.chars().count(), 200);
}

#[test]
fn attention_cli_json_is_global_while_text_keeps_project_groups() {
    let output = std::process::Command::new("python3")
        .args(["tests/attention_cli.py", env!("CARGO_BIN_EXE_sboard")])
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
fn first_seen_recent_and_same_status_waiting_activity() {
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    p[0].sessions.truncate(1);
    let s = &mut p[0].sessions[0];
    s.status = session_board::model::Status::Waiting;
    s.last_activity_at = NOW + 1;
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    tracker.scan_at(&[], true, &mut record, NOW);
    tracker.scan_at(&p, true, &mut record, NOW + 2);
    assert_eq!(record.calls.len(), 1);
    p[0].sessions[0].last_activity_at = NOW + 3;
    tracker.scan_at(&p, true, &mut record, NOW + 4);
    assert_eq!(record.calls.len(), 2);
    tracker.scan_at(&[], true, &mut record, NOW + 5);
    tracker.scan_at(&p, true, &mut record, NOW + 6);
    assert_eq!(
        record.calls.len(),
        2,
        "reappearing old transcript must stay silent"
    );
}

#[test]
fn absent_store_preserves_a_known_transition_at_the_original_timestamp() {
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("store");
    std::fs::create_dir_all(store.join("project")).unwrap();
    let transcript = store.join("project/session.jsonl");
    std::fs::copy("fixtures/claude_code/project/working.jsonl", &transcript).unwrap();
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    tracker.scan_at(&projects(&store), true, &mut record, NOW);
    std::fs::rename(&store, temp.path().join("absent")).unwrap();
    tracker.scan_at(&projects(&store), true, &mut record, NOW + 10);
    std::fs::rename(temp.path().join("absent"), &store).unwrap();
    let waiting = std::fs::read_to_string("fixtures/claude_code/project/waiting.jsonl")
        .unwrap()
        .replace("\"waiting\"", "\"working\"");
    std::fs::write(&transcript, waiting).unwrap();
    tracker.scan_at(&projects(&store), true, &mut record, NOW + 20);
    assert_eq!(
        record.calls.len(),
        1,
        "known working -> waiting was forgotten while the store was absent"
    );
    tracker.scan_at(&projects(&store), true, &mut record, NOW + 30);
    assert_eq!(record.calls.len(), 1);
}

#[test]
fn activity_rollback_then_restore_does_not_notify_the_same_turn_twice() {
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    p[0].sessions.truncate(1);
    p[0].sessions[0].status = session_board::model::Status::Working;
    p[0].sessions[0].last_activity_at = NOW;
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    tracker.scan_at(&p, true, &mut record, NOW);
    p[0].sessions[0].status = session_board::model::Status::Waiting;
    tracker.scan_at(&p, true, &mut record, NOW + 1);
    p[0].sessions[0].last_activity_at = NOW - 5;
    tracker.scan_at(&p, true, &mut record, NOW + 2);
    p[0].sessions[0].last_activity_at = NOW;
    tracker.scan_at(&p, true, &mut record, NOW + 3);
    assert_eq!(
        record.calls.len(),
        1,
        "restoring the already-seen activity notified again"
    );
    p[0].sessions[0].last_activity_at = NOW + 1;
    tracker.scan_at(&p, true, &mut record, NOW + 4);
    assert_eq!(
        record.calls.len(),
        2,
        "a genuinely newer turn must still notify"
    );
}

#[test]
fn five_transitions_are_individual_but_exactly_six_are_a_summary() {
    for count in [5, 6] {
        let mut p = projects(std::path::Path::new("fixtures/claude_code"));
        let sample = p[0].sessions[0].clone();
        p[0].sessions = (0..count)
            .map(|n| {
                let mut s = sample.clone();
                s.id = format!("boundary:{n}");
                s.transcript_path = format!("boundary-{n}").into();
                s.status = session_board::model::Status::Working;
                s
            })
            .collect();
        let mut tracker = Transitions::default();
        let mut record = Recording::default();
        tracker.scan_at(&p, true, &mut record, NOW);
        for s in &mut p[0].sessions {
            s.status = session_board::model::Status::Waiting;
        }
        tracker.scan_at(&p, true, &mut record, NOW + 1);
        assert_eq!(record.calls.len(), if count == 5 { 5 } else { 1 });
        if count == 6 {
            assert_eq!(record.calls[0].0, "sboard: 6 sessions need you");
        }
    }
}

#[test]
fn announced_status_activity_is_not_repeated_after_a_status_rollback() {
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    p[0].sessions.truncate(1);
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    for (status, activity, scan_time) in [
        (session_board::model::Status::Working, NOW, NOW),
        (session_board::model::Status::Waiting, NOW, NOW + 1),
        (session_board::model::Status::Working, NOW - 5, NOW + 2),
        (session_board::model::Status::Waiting, NOW, NOW + 3),
    ] {
        p[0].sessions[0].status = status;
        p[0].sessions[0].last_activity_at = activity;
        tracker.scan_at(&p, true, &mut record, scan_time);
    }
    assert_eq!(
        record.calls.len(),
        1,
        "restored announced status/activity notified twice"
    );
    p[0].sessions[0].last_activity_at = NOW + 4;
    tracker.scan_at(&p, true, &mut record, NOW + 4);
    assert_eq!(record.calls.len(), 2);
}
#[test]
fn future_activity_does_not_poison_later_real_questions() {
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    p[0].sessions.truncate(1);
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    p[0].sessions[0].status = session_board::model::Status::Working;
    p[0].sessions[0].last_activity_at = NOW;
    tracker.scan_at(&p, true, &mut record, NOW);
    p[0].sessions[0].status = session_board::model::Status::Waiting;
    for (activity, time) in [
        (NOW + 86400, NOW + 1),
        (NOW + 86400, NOW + 2),
        (NOW + 2, NOW + 3),
        (NOW + 86400, NOW + 3),
        (NOW + 4, NOW + 4),
    ] {
        p[0].sessions[0].last_activity_at = activity;
        tracker.scan_at(&p, true, &mut record, time);
    }
    assert_eq!(
        record.calls.len(),
        3,
        "future timestamp suppressed new real questions"
    );
}
#[test]
fn newly_seen_session_in_the_previous_scan_second_notifies() {
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    p[0].sessions.truncate(1);
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    tracker.scan_at(&[], true, &mut record, NOW);
    p[0].sessions[0].status = session_board::model::Status::Waiting;
    p[0].sessions[0].last_activity_at = NOW;
    tracker.scan_at(&p, true, &mut record, NOW + 1);
    assert_eq!(record.calls.len(), 1);
}

#[test]
fn a_new_question_after_clock_rollback_notifies_once() {
    use session_board::model::Status;
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    p[0].sessions.truncate(1);
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    for (status, activity, now) in [
        (Status::Working, 1000, 1000),
        (Status::Waiting, 1001, 1001),
        (Status::Waiting, 902, 902),
        (Status::Waiting, 902, 903),
    ] {
        p[0].sessions[0].status = status;
        p[0].sessions[0].last_activity_at = activity;
        tracker.scan_at(&p, true, &mut record, now);
    }
    assert_eq!(
        record.calls.len(),
        2,
        "clock correction lost a new question"
    );
}

#[test]
fn notification_replay_history_keeps_32_events_and_rejects_older_replays() {
    use session_board::model::Status;
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    p[0].sessions.truncate(1);
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    p[0].sessions[0].status = Status::Working;
    tracker.scan_at(&p, true, &mut record, NOW);
    for activity in NOW + 1..=NOW + 32 {
        p[0].sessions[0].status = Status::Waiting;
        p[0].sessions[0].last_activity_at = activity;
        tracker.scan_at(&p, true, &mut record, activity);
    }
    let mut replay = |activity, now| {
        p[0].sessions[0].status = Status::Working;
        tracker.scan_at(&p, true, &mut record, now);
        p[0].sessions[0].status = Status::Waiting;
        p[0].sessions[0].last_activity_at = activity;
        tracker.scan_at(&p, true, &mut record, now);
        record.calls.len()
    };
    assert_eq!(
        replay(NOW + 1, NOW + 33),
        32,
        "oldest of 32 was evicted early"
    );
    assert_eq!(replay(NOW + 33, NOW + 34), 33);
    assert_eq!(replay(NOW + 2, NOW + 35), 33, "a retained event repeated");
    assert_eq!(
        replay(NOW + 1, NOW + 36),
        33,
        "evicted activity older than the retained floor repeated"
    );
}

#[test]
fn an_unchanged_scan_after_clock_rollback_does_not_hide_the_next_question() {
    use session_board::model::Status;
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    p[0].sessions.truncate(1);
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    for (status, activity, now) in [
        (Status::Working, 1000, 1000),
        (Status::Waiting, 1001, 1001),
        (Status::Waiting, 1001, 902),
        (Status::Waiting, 902, 902),
        (Status::Waiting, 902, 903),
    ] {
        p[0].sessions[0].status = status;
        p[0].sessions[0].last_activity_at = activity;
        tracker.scan_at(&p, true, &mut record, now);
    }
    assert_eq!(record.calls.len(), 2);
}

#[test]
fn first_seen_waiting_after_clock_step_uses_current_age() {
    use session_board::model::Status;
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    p[0].sessions.truncate(1);
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    tracker.scan_at(&[], true, &mut record, 1_793_602_000);
    p[0].sessions[0].status = Status::Waiting;
    p[0].sessions[0].last_activity_at = 1_793_601_902;
    tracker.scan_at(&p, true, &mut record, 1_793_601_902);
    tracker.scan_at(&p, true, &mut record, 1_793_601_903);
    assert_eq!(record.calls.len(), 1, "new session lost after clock step");
}

#[test]
fn an_in_scan_question_survives_an_unchanged_result_after_clock_step() {
    use session_board::model::Status;
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    p[0].sessions.truncate(1);
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    for (status, activity, now) in [
        (Status::Working, 1_793_601_999, 1_793_601_999),
        (Status::Waiting, 1_793_602_000, 1_793_602_000),
        (Status::Waiting, 1_793_602_000, 1_793_601_902),
        (Status::Waiting, 1_793_601_901, 1_793_601_903),
        (Status::Waiting, 1_793_601_901, 1_793_601_904),
    ] {
        p[0].sessions[0].status = status;
        p[0].sessions[0].last_activity_at = activity;
        tracker.scan_at(&p, true, &mut record, now);
    }
    assert_eq!(
        record.calls.len(),
        2,
        "in-scan question lost below the old watermark"
    );
}

#[test]
fn forty_announcements_do_not_reannounce_an_evicted_first_event() {
    use session_board::model::Status;
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    p[0].sessions.truncate(1);
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    p[0].sessions[0].status = Status::Working;
    tracker.scan_at(&p, true, &mut record, NOW);
    for activity in NOW + 1..=NOW + 40 {
        p[0].sessions[0].status = Status::Waiting;
        p[0].sessions[0].last_activity_at = activity;
        tracker.scan_at(&p, true, &mut record, activity);
    }
    assert_eq!(record.calls.len(), 40);
    p[0].sessions[0].status = Status::Working;
    tracker.scan_at(&p, true, &mut record, NOW + 41);
    p[0].sessions[0].status = Status::Waiting;
    p[0].sessions[0].last_activity_at = NOW + 1;
    tracker.scan_at(&p, true, &mut record, NOW + 41);
    assert_eq!(
        record.calls.len(),
        40,
        "evicted first event was announced again"
    );
}

#[test]
fn first_seen_age_window_is_inclusive_and_uses_only_the_current_clock() {
    use session_board::model::Status;
    for (age, expected) in [(0, 1), (300, 1), (301, 0), (3 * 86400, 0)] {
        let mut p = projects(std::path::Path::new("fixtures/claude_code"));
        p[0].sessions.truncate(1);
        p[0].sessions[0].status = Status::Waiting;
        p[0].sessions[0].last_activity_at = NOW - age;
        let mut tracker = Transitions::default();
        let mut record = Recording::default();
        tracker.scan_at(&[], true, &mut record, NOW + 86400);
        tracker.scan_at(&p, true, &mut record, NOW);
        assert_eq!(record.calls.len(), expected, "age {age}");
    }
}

#[test]
fn known_error_activity_changes_are_not_gated_by_scan_time_or_age() {
    use session_board::model::Status;
    let mut p = projects(std::path::Path::new("fixtures/claude_code"));
    p[0].sessions.truncate(1);
    p[0].sessions[0].status = Status::Working;
    p[0].sessions[0].last_activity_at = NOW - 3 * 86400;
    let mut tracker = Transitions::default();
    let mut record = Recording::default();
    tracker.scan_at(&p, true, &mut record, NOW);
    p[0].sessions[0].status = Status::Error;
    tracker.scan_at(&p, true, &mut record, NOW + 1);
    p[0].sessions[0].last_activity_at += 1;
    tracker.scan_at(&p, true, &mut record, NOW + 2);
    tracker.scan_at(&p, true, &mut record, NOW + 3);
    assert_eq!(record.calls.len(), 2);
}

#[test]
fn attention_uses_first_seen_and_both_archive_forms() {
    let p = projects(std::path::Path::new("fixtures/claude_code"));
    let mut state = State::default();
    assert!(
        attention(&p, &state)
            .iter()
            .all(|i| i.session.status != session_board::model::Status::Done)
    );
    state.first_seen = Some(0);
    assert!(
        attention(&p, &state)
            .iter()
            .any(|i| i.session.status == session_board::model::Status::Done)
    );
    let selected = attention(&p, &state)[0].session.clone();
    state.archived.push(selected.id.clone());
    assert!(
        attention(&p, &state)
            .iter()
            .all(|i| i.session.id != selected.id)
    );
    state
        .archived
        .push(selected.project_root.to_string_lossy().into_owned());
    assert!(attention(&p, &state).is_empty());
}
