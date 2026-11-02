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
    let expected: Vec<_> = attention(&projects, &state)
        .iter()
        .map(|i| i.session.id.clone())
        .collect();
    assert!(!expected.is_empty());
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
fn edited_fixtures_notify_once_per_transition_and_never_on_startup_or_disabled() {
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
        ("waiting", 4),
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
    assert_eq!(recording.calls.len(), 4);
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
    let mut desktop = session_board::notify::Desktop::default();
    // Missing command falls back to Noop and warns once.
    std::fs::remove_file(std::env::var_os("PROBE_REMOVE").unwrap()).unwrap();
    desktop.notify(title, body).unwrap();
    desktop.notify(title, body).unwrap();
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
    assert_eq!(
        String::from_utf8_lossy(&output.stderr)
            .matches("desktop notification:")
            .count(),
        1
    );
    let actual = std::fs::read_to_string(log).unwrap();
    assert!(actual.contains(
        "--app-name\nsboard\n--\ntitle \"quoted\"; $(touch BAD)\nbody\nsecond line \\\n"
    ));
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
