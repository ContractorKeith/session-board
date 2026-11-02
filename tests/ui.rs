#[path = "../src/ui/mod.rs"]
mod ui;
#[path = "../src/watch.rs"]
mod watch;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use session_board::{
    cli,
    config::Config,
    scan,
    source::{Registry, claude::Claude, codex::Codex},
    state::State,
};
use std::{
    fs,
    time::{Duration, Instant},
};
use ui::{Action, App};

const NOW: i64 = 1_793_534_400;
fn fixture_app() -> App {
    let registry: Registry = vec![
        Box::new(Claude {
            root: "fixtures/claude_code".into(),
            now: NOW,
        }),
        Box::new(Codex {
            root: "fixtures/codex/sessions".into(),
            index: "fixtures/codex/session_index.jsonl".into(),
            now: NOW,
        }),
    ];
    let state = State::default();
    let mut result = scan::scan(&registry, &Config::default(), &state, |cwd| cwd.to_owned());
    cli::filter(&mut result.projects, &state, None, None, None, false);
    let mut app = App::new(false);
    app.summary = format!("{} projects", result.projects.len());
    app.replace(result.projects);
    app
}
fn key(app: &mut App, code: KeyCode) -> Action {
    app.key(KeyEvent::new(code, KeyModifiers::NONE))
}
fn frame(app: &App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.render(frame, NOW)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
                + "\n"
        })
        .collect()
}
#[test]
fn fixture_tree_snapshot() {
    let mut app = fixture_app();
    key(&mut app, KeyCode::Down);
    let actual = frame(&app, 100, 30);
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        fs::write("tests/snapshots/tree.txt", &actual).unwrap();
    }
    assert_eq!(
        actual,
        fs::read_to_string("tests/snapshots/tree.txt").unwrap()
    );
}
#[test]
fn navigation_fold_and_rescan_identity() {
    let mut app = fixture_app();
    let count = app.rows().len();
    key(&mut app, KeyCode::Char('j'));
    let id = app.session().unwrap().id.clone();
    let mut projects = app.projects.clone();
    projects.reverse();
    for project in &mut projects {
        project.sessions.reverse();
    }
    app.replace(projects);
    assert_eq!(app.session().unwrap().id, id);
    key(&mut app, KeyCode::Left);
    assert!(app.rows().len() < count);
    assert!(app.session().is_none());
    key(&mut app, KeyCode::Right);
    assert_eq!(app.rows().len(), count);
    key(&mut app, KeyCode::Down);
    assert!(app.session().is_some());
    key(&mut app, KeyCode::Up);
    assert!(app.session().is_none());
    assert_eq!(key(&mut app, KeyCode::Enter), Action::Open);
    assert_eq!(key(&mut app, KeyCode::Char('r')), Action::Rescan);
    key(&mut app, KeyCode::Char('?'));
    assert!(app.help);
    let selected = app.selected;
    key(&mut app, KeyCode::Down);
    assert_eq!(app.selected, selected);
    assert!(frame(&app, 100, 30).contains("toggle help"));
    assert_eq!(key(&mut app, KeyCode::Char('q')), Action::Quit);
    assert_eq!(
        app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Action::Quit
    );
    app.replace(vec![]);
    assert_eq!(app.selected, 0);
}
#[test]
fn tiny_empty_and_wide_unicode_frames() {
    let mut app = fixture_app();
    let text = "界😀e\u{301}".repeat(100);
    let s = &mut app.projects[0].sessions[0];
    s.title.clone_from(&text);
    s.last_user_text = format!("{text}\n").repeat(100);
    s.last_assistant_text.clone_from(&text);
    key(&mut app, KeyCode::Down);
    for (w, h) in [(0, 0), (1, 1), (20, 10), (100, 30)] {
        frame(&app, w, h);
    }
    let rendered = frame(&app, 100, 30);
    assert!(rendered.contains("Last user:"));
    assert!(rendered.contains("Last assistant:"));
    assert!(rendered.contains("Resume:"));
    app.replace(vec![]);
    for (w, h) in [(0, 0), (1, 1), (100, 30)] {
        frame(&app, w, h);
    }
    assert_eq!(key(&mut app, KeyCode::Down), Action::None);
    assert_eq!(app.selected, 0);
}
#[test]
fn no_color_and_palette_statuses() {
    let mut app = fixture_app();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| app.render(f, NOW)).unwrap();
    assert!(
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .all(|cell| cell.fg == ratatui::style::Color::Reset)
    );
    app.color = true;
    terminal.draw(|f| app.render(f, NOW)).unwrap();
    assert!(
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .any(|cell| cell.fg != ratatui::style::Color::Reset)
    );
}
// FSEvents can deliver a coalesced batch after the first debounce window.
fn drain_requests(requests: &std::sync::mpsc::Receiver<()>) -> usize {
    let deadline = Instant::now() + watch::DEBOUNCE + Duration::from_secs(2);
    let mut count = 0;
    while requests
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .is_ok()
    {
        count += 1;
    }
    count
}

#[test]
fn recursive_watcher_debounces_single_and_burst_changes() {
    for burst in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let (_watcher, requests) = watch::StoreWatcher::new(&[temp.path().to_owned()]).unwrap();
        let started = Instant::now();
        fs::write(nested.join("session.jsonl"), "one").unwrap();
        if burst {
            std::thread::sleep(Duration::from_millis(100));
            fs::write(nested.join("session.jsonl"), "two").unwrap();
        }
        requests
            .recv_timeout(Duration::from_secs(10).saturating_sub(started.elapsed()))
            .expect("recursive_watcher_debounces_single_and_burst_changes: waiting for debounced write request");
        assert!(started.elapsed() >= watch::DEBOUNCE);
        let count = 1 + drain_requests(&requests);
        if burst {
            assert!((1..=2).contains(&count), "burst produced {count} requests");
        } else {
            assert_eq!(count, 1, "isolated touch must yield exactly one request");
        }
    }
}

// Invoked in a child with piped output, or under a PTY for manual hygiene checks.
#[test]
#[ignore]
fn terminal_probe() {
    match std::env::var("SBOARD_TERMINAL_PROBE").as_deref() {
        Ok(mode @ ("hook" | "error")) => {
            let hook = mode == "hook";
            std::thread::Builder::new()
                .name("main".into())
                .spawn(move || {
                    if hook {
                        // Exit in the previous hook, before unwinding can run the guard.
                        std::panic::set_hook(Box::new(|_| std::process::exit(86)));
                    }
                    let result = ui::terminal_session(|terminal| {
                        terminal.draw(|frame| fixture_app().render(frame, NOW))?;
                        if hook {
                            panic!("hook probe");
                        }
                        Err(std::io::Error::other("guard probe"))
                    });
                    assert!(result.is_err());
                })
                .unwrap()
                .join()
                .unwrap();
        }
        _ => {
            assert!(ui::run().is_err());
        }
    }
}

#[test]
fn piped_probe_refuses_terminal_setup() {
    let temp = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "terminal_probe"])
        .env_remove("SBOARD_TERMINAL_PROBE")
        .env("HOME", temp.path())
        .env_remove("GROK_HOME")
        .env_remove("OPENCODE_DB")
        .env_remove("XDG_DATA_HOME")
        .env("SBOARD_STATE_DIR", temp.path().join("state"))
        .env("SBOARD_CONFIG_DIR", temp.path().join("config"))
        .env("CLAUDE_CONFIG_DIR", temp.path().join("claude"))
        .env("CODEX_HOME", temp.path().join("codex"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn duplicate_transcripts_keep_details_and_selection() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    for (file, text) in [("first", "first copy"), ("second", "second copy")] {
        let fixture = fs::read_to_string("fixtures/claude_code/project/done.jsonl")
            .unwrap()
            .replace("User placeholder", text);
        fs::write(project.join(format!("{file}.jsonl")), fixture).unwrap();
    }
    let registry: Registry = vec![Box::new(Claude {
        root: temp.path().into(),
        now: NOW,
    })];
    let rescan = || {
        scan::scan(&registry, &Config::default(), &State::default(), |cwd| {
            cwd.to_owned()
        })
        .projects
    };
    let mut app = App::new(false);
    app.replace(rescan());
    app.selected = 1;
    assert_eq!(app.session().unwrap().last_user_text, "first copy");
    assert!(frame(&app, 100, 30).contains("first copy"));
    app.selected = 2;
    assert_eq!(app.session().unwrap().last_user_text, "second copy");
    assert!(frame(&app, 100, 30).contains("second copy"));
    app.replace(rescan());
    assert_eq!(app.selected, 2);
    assert_eq!(app.session().unwrap().last_user_text, "second copy");
}

#[test]
fn detail_shows_last_nonblank_lines() {
    let mut app = fixture_app();
    app.projects[0].sessions[0].last_user_text =
        format!("{}\n\nLAST USER LINE", "obsolete\n".repeat(40));
    app.projects[0].sessions[0].last_assistant_text =
        format!("{}\n\nLAST ASSISTANT LINE", "obsolete\n".repeat(40));
    app.selected = 1;
    let output = frame(&app, 100, 30);
    assert!(output.contains("LAST USER LINE"));
    assert!(output.contains("LAST ASSISTANT LINE"));
}

#[test]
fn steady_writes_cannot_starve_refresh() {
    let temp = tempfile::tempdir().unwrap();
    let (watcher, requests) = watch::StoreWatcher::new(&[temp.path().to_owned()]).unwrap();
    assert!(
        watcher
            .status
            .recv_timeout(Duration::from_secs(10))
            .expect("steady_writes_cannot_starve_refresh: waiting for watch attachment status")
            .is_empty()
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut count = 0;
    let mut writes = 0;
    while count < 3 && Instant::now() < deadline {
        fs::write(temp.path().join("session.jsonl"), writes.to_string()).unwrap();
        watcher
            .observed
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("watcher did not acknowledge a write");
        writes += 1;
        count += requests.try_iter().count();
    }
    assert!(
        count >= 3,
        "steady acknowledged writes starved refresh: {count} requests, {writes} writes"
    );
}

#[test]
fn missing_root_is_attached_later() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let (_watcher, requests) = watch::StoreWatcher::new(std::slice::from_ref(&root)).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join("rollout-new.jsonl"), "{}").unwrap();
    requests
        .recv_timeout(Duration::from_secs(8))
        .expect("root was never attached");
}

#[test]
fn scans_initial_watcher_wiring_and_panic_recovery() {
    use session_board::{
        model::Status,
        source::{Source, claude::Claude},
    };
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("project")).unwrap();
    let path = temp.path().join("project/session.jsonl");
    fs::copy("fixtures/claude_code/project/working.jsonl", &path).unwrap();
    let root = temp.path().to_owned();
    let scans =
        watch::Scans::test_scanner(vec![root.clone()], Duration::from_secs(60), move |first| {
            if first {
                panic!("panic once");
            }
            let sessions = Claude {
                root: root.clone(),
                now: NOW,
            }
            .scan(&Config::default())
            .sessions;
            let mut app = fixture_app();
            app.projects[0].sessions = sessions;
            Ok(scan::Scan {
                projects: app.projects,
                ..Default::default()
            })
        });
    let error = scans
        .results
        .recv_timeout(Duration::from_secs(8))
        .expect("scans_initial_watcher_wiring_and_panic_recovery: waiting for scan worker result")
        .unwrap_err();
    assert!(error.to_string().starts_with("panicked:"));
    scans.requests.try_send(()).unwrap();
    let initial = scans
        .results
        .recv_timeout(Duration::from_secs(8))
        .expect("scans_initial_watcher_wiring_and_panic_recovery: waiting for scan worker result")
        .unwrap();
    assert_eq!(initial.projects[0].sessions[0].status, Status::Working);
    fs::copy("fixtures/claude_code/project/waiting.jsonl", &path).unwrap();
    let refreshed = scans
        .results
        .recv_timeout(Duration::from_secs(10))
        .expect("scans_initial_watcher_wiring_and_panic_recovery: waiting for scan worker result")
        .unwrap();
    assert_eq!(refreshed.projects[0].sessions[0].status, Status::Waiting);
}

#[test]
fn quiet_store_status_crosses_threshold() {
    use session_board::{
        model::Status,
        source::{Source, claude::Claude},
    };
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("project")).unwrap();
    let text = fs::read_to_string("fixtures/claude_code/project/working.jsonl")
        .unwrap()
        .replace("11:59:00", "11:30:20");
    fs::write(temp.path().join("project/session.jsonl"), text).unwrap();
    let root = temp.path().to_owned();
    let scans = watch::Scans::test_scanner(
        vec![root.clone()],
        Duration::from_millis(100),
        move |first| {
            let sessions = Claude {
                root: root.clone(),
                now: NOW + if first { 0 } else { 60 },
            }
            .scan(&Config::default())
            .sessions;
            let mut app = fixture_app();
            app.projects[0].sessions = sessions;
            Ok(scan::Scan {
                projects: app.projects,
                ..Default::default()
            })
        },
    );
    assert_eq!(
        scans
            .results
            .recv_timeout(Duration::from_secs(5))
            .expect("quiet_store_status_crosses_threshold: waiting for quiet-store refresh result")
            .unwrap()
            .projects[0]
            .sessions[0]
            .status,
        Status::Working
    );
    assert_eq!(
        scans
            .results
            .recv_timeout(Duration::from_secs(5))
            .expect("quiet_store_status_crosses_threshold: waiting for quiet-store refresh result")
            .unwrap()
            .projects[0]
            .sessions[0]
            .status,
        Status::Error
    );
}

#[test]
fn burst_coalesces_and_recreated_root_reattaches() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    fs::create_dir(&root).unwrap();
    // The configured store may be a symlink; events name its canonical target.
    #[cfg(unix)]
    let watched = {
        let link = temp.path().join("store-link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        link
    };
    #[cfg(not(unix))]
    let watched = root.clone();
    let (watcher, requests) = watch::StoreWatcher::new(std::slice::from_ref(&watched)).unwrap();
    assert!(
        watcher
            .status
            .recv_timeout(Duration::from_secs(10))
            .expect("burst_coalesces_and_recreated_root_reattaches: waiting for watcher attachment/removal acknowledgement")
            .is_empty()
    );
    // Keep all 200 real writes within one debounce window, even under build load.
    use std::io::Write;
    let started = Instant::now();
    let mut file = fs::File::create(root.join("session.jsonl")).unwrap();
    for n in 0..200 {
        writeln!(file, "{n}").unwrap();
    }
    drop(file);
    assert!(
        started.elapsed() < watch::DEBOUNCE,
        "producer exceeded debounce window: {:?}",
        started.elapsed()
    );
    requests.recv_timeout(Duration::from_secs(10)).unwrap();
    let count = 1 + drain_requests(&requests);
    assert!(
        (1..=2).contains(&count),
        "200-write burst produced {count} requests"
    );
    fs::remove_dir_all(&root).unwrap();
    // Synchronize with watcher state instead of guessing when backend events arrive.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let warnings = watcher
            .status
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("burst_coalesces_and_recreated_root_reattaches: waiting for watcher attachment/removal acknowledgement");
        if warnings.iter().any(|w| w.contains("unwatched:")) {
            break;
        }
    }
    fs::create_dir(&root).unwrap();
    fs::write(root.join("new.jsonl"), "{}").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if watcher
            .status
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("burst_coalesces_and_recreated_root_reattaches: waiting for watcher attachment/removal acknowledgement")
            .is_empty()
        {
            break;
        }
    }
    // Drain attachment/deletion requests before proving the replacement is watched.
    drain_requests(&requests);
    fs::write(root.join("new.jsonl"), "changed").unwrap();
    requests.recv_timeout(Duration::from_secs(10)).unwrap();
}

#[test]
fn release_keys_color_warnings_and_scroll_memory() {
    use crossterm::event::KeyEventKind;
    use std::ffi::OsStr;
    assert!(ui::color_enabled(None));
    assert!(ui::color_enabled(Some(OsStr::new(""))));
    assert!(!ui::color_enabled(Some(OsStr::new("1"))));
    let mut app = fixture_app();
    for code in [
        KeyCode::Down,
        KeyCode::Char('q'),
        KeyCode::Char('?'),
        KeyCode::Char('r'),
    ] {
        assert_eq!(
            app.key(KeyEvent::new_with_kind(
                code,
                KeyModifiers::NONE,
                KeyEventKind::Release
            )),
            Action::None
        );
    }
    assert_eq!(app.selected, 0);
    assert!(!app.help);
    app.scan_warnings = vec!["warning ".repeat(100), "second".into()];
    let rendered = frame(&app, 100, 30);
    assert!(rendered.contains("2 warnings: warning"));
    assert!(
        rendered
            .lines()
            .last()
            .unwrap()
            .ends_with("? help · r rescan · q quit")
    );
    frame(&app, 100, 20);
    key(&mut app, KeyCode::End);
    frame(&app, 100, 20);
    let offset = app.list.borrow().offset();
    assert!(offset > 0);
    key(&mut app, KeyCode::Up);
    frame(&app, 100, 20);
    assert_eq!(app.list.borrow().offset(), offset);
    key(&mut app, KeyCode::PageUp);
    assert!(app.selected < app.rows().len() - 2);
    key(&mut app, KeyCode::Home);
    assert_eq!(app.selected, 0);
    key(&mut app, KeyCode::PageDown);
    assert!(app.selected > 0);
}

#[test]
fn dead_worker_is_reported() {
    let mut scans = watch::Scans::test_scanner(vec![], Duration::from_secs(60), |_| {
        Ok(scan::Scan::default())
    });
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    drop(receiver);
    scans.requests = sender;
    let mut app = App::new(false);
    app.request_scan(&scans);
    assert_eq!(app.status_text(), "scan worker died; restart sboard");
}

#[test]
fn script_pty_terminal_hygiene_and_worker_recovery() {
    let output = std::process::Command::new("python3")
        .arg("tests/terminal.py")
        .arg(env!("CARGO_BIN_EXE_sboard"))
        .arg(std::env::current_exe().unwrap())
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
fn organization_keys_persist_and_selection_clears_unread() {
    use session_board::scan::{Group, Sort};
    let temp = tempfile::tempdir().unwrap();
    let mut app = fixture_app();
    app.load_state(temp.path().join("state.toml"));
    let root = app.projects[0].root.clone();
    assert_eq!(key(&mut app, KeyCode::Char('p')), Action::Rescan);
    assert!(
        State::load(app.organize.path.as_ref().unwrap())
            .unwrap()
            .0
            .pins
            .contains(&root)
    );
    key(&mut app, KeyCode::Char('p'));
    assert!(
        !State::load(app.organize.path.as_ref().unwrap())
            .unwrap()
            .0
            .pins
            .contains(&root)
    );
    key(&mut app, KeyCode::Char('p'));
    key(&mut app, KeyCode::Char('t'));
    for c in "client, urgent".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    assert!(frame(&app, 100, 30).contains("Tags: client, urgent"));
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        State::load(app.organize.path.as_ref().unwrap())
            .unwrap()
            .0
            .tags[&root],
        ["client", "urgent"]
    );
    key(&mut app, KeyCode::Char('m'));
    let session = app.projects[0].sessions[0].clone();
    assert_eq!(app.state.read[&session.id], session.last_activity_at);
    key(&mut app, KeyCode::Char('a'));
    assert!(!app.projects.iter().any(|p| p.root == root));
    key(&mut app, KeyCode::Char('A'));
    assert!(app.projects.iter().any(|p| p.root == root && p.archived));
    assert!(frame(&app, 100, 30).contains("[archived]"));
    app.selected = app
        .rows()
        .iter()
        .position(|r| r == &ui::Row::Project(root.clone()))
        .unwrap();
    key(&mut app, KeyCode::Char('a'));
    key(&mut app, KeyCode::Down);
    let id = app.session().unwrap().id.clone();
    key(&mut app, KeyCode::Char('a'));
    assert!(app.state.archived.contains(&id));
    key(&mut app, KeyCode::Char('a'));
    key(&mut app, KeyCode::Char('s'));
    assert_eq!(app.state.sort, Sort::Attention);
    key(&mut app, KeyCode::Char('g'));
    assert_eq!(app.state.group, Group::Tag);
    assert!(app.projects.iter().any(|p| p.name == "client"));
    key(&mut app, KeyCode::Char('g'));
    let mut raw = app.organize.projects.clone();
    for p in &mut raw {
        for s in &mut p.sessions {
            s.last_activity_at += 1;
        }
    }
    app.selected = 0;
    app.replace(raw.clone());
    assert!(app.projects.iter().any(|p| p.unread_count > 0));
    app.selected = app
        .rows()
        .iter()
        .position(|r| matches!(r, ui::Row::Session(_, _)))
        .unwrap()
        - 1;
    key(&mut app, KeyCode::Down);
    let session = app.session().unwrap().clone();
    assert!(!app.state.unread(&session));
    app.replace(raw);
    assert!(!app.state.unread(app.session().unwrap()));
    let mut restarted = App::new(false);
    restarted.load_state(temp.path().join("state.toml"));
    assert!(restarted.state.pins.contains(&root));
    assert_eq!(restarted.state.tags[&root], ["client", "urgent"]);
    assert_eq!(restarted.state.sort, Sort::Attention);
    assert!(restarted.state.show_archived);
}

#[test]
fn organized_tree_snapshot() {
    let mut app = fixture_app();
    let root = app.projects[0].root.clone();
    app.state.first_seen = Some(0);
    app.state.pins.push(root.clone());
    app.state
        .tags
        .insert(root, vec!["client".into(), "urgent".into()]);
    app.state
        .archived
        .push(app.projects[0].sessions[1].id.clone());
    app.state.show_archived = true;
    app.project_organization();
    let actual = frame(&app, 160, 22);
    assert!(actual.contains("[pinned] #client #urgent"));
    assert!(actual.contains("[archived]"));
    assert!(actual.contains("•13"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        fs::write("tests/snapshots/organized.txt", &actual).unwrap();
    }
    assert_eq!(
        actual,
        fs::read_to_string("tests/snapshots/organized.txt").unwrap()
    );
}

#[test]
fn missing_pinned_project_with_historical_sessions_is_grey() {
    let mut app = fixture_app();
    app.color = true;
    let root = app.projects[0].root.clone();
    app.state.pins.push(root.clone());
    app.organize.missing_projects.insert(root);
    app.project_organization();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| app.render(f, NOW)).unwrap();
    let tree_y = frame(&app, 100, 30)
        .lines()
        .position(|line| line.contains("Projects"))
        .unwrap() as u16;
    assert_eq!(
        terminal.backend().buffer()[(4, tree_y + 1)].fg,
        ratatui::style::Color::DarkGray
    );
    assert_eq!(
        terminal.backend().buffer()[(4, tree_y + 2)].fg,
        ratatui::style::Color::DarkGray
    );
}

#[test]
fn issue69_two_apps_keep_remote_removals() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    let mut a = fixture_app();
    a.load_state(path.clone());
    key(&mut a, KeyCode::Char('p'));
    let root = a.projects[0].root.clone();
    let mut b = fixture_app();
    b.load_state(path.clone());
    b.project_organization();
    key(&mut a, KeyCode::Char('p'));
    key(&mut b, KeyCode::Char('t'));
    for c in "client".chars() {
        key(&mut b, KeyCode::Char(c));
    }
    key(&mut b, KeyCode::Enter);
    let saved = State::load(&path).unwrap().0;
    assert!(!saved.pins.contains(&root));
    assert_eq!(saved.tags[&root], ["client"]);
}

#[cfg(unix)]
#[test]
fn issue71_failed_save_survives_rescan_with_error_message() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("state");
    fs::create_dir(&dir).unwrap();
    let path = dir.join("state.toml");
    let mut app = fixture_app();
    app.load_state(path);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    key(&mut app, KeyCode::Char('p'));
    let projects = app.organize.projects.clone();
    app.replace(projects);
    assert!(app.projects[0].pinned);
    assert!(
        app.status_text().contains("state:"),
        "{}",
        app.status_text()
    );
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn issue72_paste_and_chords_never_dispatch_organizing_keys() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = fixture_app();
    app.load_state(temp.path().join("state.toml"));
    key(&mut app, KeyCode::Char('t'));
    app.event(
        crossterm::event::Event::Paste("foo,bar\np\nq".into()),
        Instant::now(),
    );
    app.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    app.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT));
    assert_eq!(key(&mut app, KeyCode::Enter), Action::Rescan);
    let saved = State::load(app.organize.path.as_ref().unwrap()).unwrap().0;
    assert_eq!(saved.tags[&app.projects[0].root], ["barpq", "foo"]);
    assert!(saved.pins.is_empty());
    assert!(saved.archived.is_empty());
    key(&mut app, KeyCode::Char('t'));
    assert_eq!(
        app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Action::None
    );
    assert!(app.organize.prompt.is_none());
}

#[test]
fn issue74_repeated_session_retains_group_identity() {
    let mut app = fixture_app();
    let root = app.projects[0].root.clone();
    app.state
        .tags
        .insert(root, vec!["alpha".into(), "beta".into()]);
    key(&mut app, KeyCode::Char('g'));
    let beta = app
        .projects
        .iter()
        .find(|p| p.name == "beta")
        .unwrap()
        .root
        .clone();
    app.selected = app
        .rows()
        .iter()
        .position(|r| matches!(r,ui::Row::Project(p) if p==&beta))
        .unwrap()
        + 1;
    let selected = app.rows()[app.selected].clone();
    key(&mut app, KeyCode::Char('p'));
    assert_eq!(app.rows()[app.selected], selected);
    key(&mut app, KeyCode::Left);
    assert!(app.folded.contains(&beta));
}

#[test]
fn issue76_selection_read_marks_are_debounced_and_quit_flushes() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = fixture_app();
    let path = temp.path().join("state.toml");
    app.load_state(path.clone());
    key(&mut app, KeyCode::Down);
    let first = app.session().unwrap().id.clone();
    assert!(app.state.read.contains_key(&first));
    assert!(!path.exists());
    key(&mut app, KeyCode::Down);
    app.flush_read_marks(false);
    assert!(!path.exists());
    key(&mut app, KeyCode::Char('q'));
    assert!(State::load(&path).unwrap().0.read.contains_key(&first));
}

#[cfg(unix)]
#[test]
fn issue70_pin_key_removes_tilde_dotdot_and_symlink_spellings() {
    let home = fs::canonicalize(session_board::cli::home().unwrap()).unwrap();
    let temp = tempfile::tempdir_in(&home).unwrap();
    let root = temp.path().join("project");
    fs::create_dir(&root).unwrap();
    fs::create_dir(temp.path().join("other")).unwrap();
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    let tilde = std::path::PathBuf::from("~").join(root.strip_prefix(&home).unwrap());
    let seed = State {
        pins: vec![tilde, alias, temp.path().join("other/../project/")],
        ..Default::default()
    };
    let path = temp.path().join("state.toml");
    fs::write(&path, toml::to_string(&seed).unwrap()).unwrap();
    let mut app = fixture_app();
    let mut raw = app.organize.projects.clone();
    for p in &mut raw {
        p.root = root.clone();
        for s in &mut p.sessions {
            s.project_root = root.clone();
        }
    }
    app.load_state(path.clone());
    app.replace(raw);
    assert!(app.projects[0].pinned);
    key(&mut app, KeyCode::Char('p'));
    assert!(!app.projects[0].pinned);
    assert!(State::load(&path).unwrap().0.pins.is_empty());
}

#[test]
fn issue71_save_warnings_and_outcome_survive_scans() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    fs::write(&path, "[broken").unwrap();
    let mut app = fixture_app();
    app.load_state(path);
    key(&mut app, KeyCode::Char('p'));
    app.summary = "scan finished".into();
    app.replace(app.organize.projects.clone());
    assert!(app.status_text().contains("corrupt state preserved"));
    key(&mut app, KeyCode::Char('?'));
    assert!(!app.status_text().contains("corrupt state preserved"));
}

#[test]
fn issue76_modes_suffix_order_and_empty_existing_root_style() {
    let mut app = fixture_app();
    app.color = true;
    let root = app.projects[0].root.clone();
    app.state.first_seen = Some(0);
    app.state.pins.push(root.clone());
    app.state.tags.insert(root, vec!["long".repeat(100)]);
    app.state.sort = session_board::scan::Sort::Attention;
    app.state.show_archived = true;
    app.project_organization();
    let text = frame(&app, 160, 22);
    assert!(text.contains("Projects · attention"));
    assert!(text.contains("[archived shown]"));
    assert!(text.contains("•14 [pinned] #"));
    assert!(text.contains("User placeholder •"));
    app.organize.projects[0].sessions.clear();
    app.project_organization();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| app.render(f, NOW)).unwrap();
    assert_eq!(
        terminal.backend().buffer()[(4, 1)].fg,
        ratatui::style::Color::Reset
    );
}

#[test]
fn issue77_selected_session_update_stays_read_through_rescan() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = fixture_app();
    app.load_state(temp.path().join("state.toml"));
    State {
        first_seen: Some(0),
        ..Default::default()
    }
    .save(app.organize.path.as_ref().unwrap())
    .unwrap();
    app.load_state(app.organize.path.clone().unwrap());
    key(&mut app, KeyCode::Down);
    let selected = app.session().unwrap().id.clone();
    let mut raw = app.organize.projects.clone();
    for p in &mut raw {
        for s in &mut p.sessions {
            s.last_activity_at += 1;
        }
    }
    app.replace(raw);
    assert_eq!(app.session().unwrap().id, selected);
    assert!(!app.state.unread(app.session().unwrap()));
    assert!(
        app.projects
            .iter()
            .flat_map(|p| &p.sessions)
            .any(|s| s.id != selected && app.state.unread(s))
    );
}

#[test]
fn issue76_explicit_read_corrects_a_future_mark() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = fixture_app();
    let path = temp.path().join("state.toml");
    let id = app.projects[0].sessions[0].id.clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    fs::write(
        &path,
        toml::to_string(&State {
            read: std::collections::BTreeMap::from([(id.clone(), now + 3600)]),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
    app.load_state(path.clone());
    app.selected = 1;
    key(&mut app, KeyCode::Char('m'));
    assert_eq!(
        State::load(&path).unwrap().0.read[&id],
        app.session().unwrap().last_activity_at
    );
}

#[cfg(unix)]
#[test]
fn issue96_pin_key_removes_alias_after_its_target_disappears() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    let path = temp.path().join("state.toml");
    fs::write(
        &path,
        toml::to_string(&State {
            pins: vec![alias],
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
    let mut app = fixture_app();
    let mut raw = app.organize.projects.clone();
    for p in &mut raw {
        p.root = root.clone();
        for s in &mut p.sessions {
            s.project_root = root.clone();
        }
    }
    app.load_state(path.clone());
    app.replace(raw);
    assert!(app.projects[0].pinned);
    fs::remove_dir(&root).unwrap();
    key(&mut app, KeyCode::Char('p'));
    let saved: State = toml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert!(
        saved.pins.is_empty(),
        "dangling alias pin survived: {:?}",
        saved.pins
    );
}

#[test]
fn issue94_paste_burst_uses_the_live_event_router() {
    use crossterm::event::Event;
    let temp = tempfile::tempdir().unwrap();
    let mut app = fixture_app();
    app.load_state(temp.path().join("state.toml"));
    let now = Instant::now();
    assert_eq!(app.event(Event::Paste("foo".into()), now), Action::None);
    for c in ['p', 'q'] {
        assert_eq!(
            app.event(
                Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)),
                now + Duration::from_millis(1)
            ),
            Action::None
        );
    }
    assert!(!app.projects[0].pinned);
    assert!(!app.organize.path.as_ref().unwrap().exists());
}

#[test]
fn issue94_prompt_paste_and_stray_terminators_share_the_live_router() {
    use crossterm::event::Event;
    let temp = tempfile::tempdir().unwrap();
    let mut app = fixture_app();
    app.load_state(temp.path().join("state.toml"));
    let now = Instant::now();
    assert_eq!(
        app.event(
            Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE)),
            now
        ),
        Action::None
    );
    app.event(Event::Paste("foo,bar\np\nq".into()), now);
    for code in [
        KeyCode::Esc,
        KeyCode::Char('['),
        KeyCode::Char('2'),
        KeyCode::Char('0'),
        KeyCode::Char('1'),
        KeyCode::Char('~'),
    ] {
        assert_eq!(
            app.event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)), now),
            Action::None
        );
    }
    assert!(
        app.organize.prompt.is_some(),
        "stray terminator cancelled the prompt"
    );
    assert_eq!(
        app.event(
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            now
        ),
        Action::Rescan
    );
    let saved = State::load(app.organize.path.as_ref().unwrap()).unwrap().0;
    assert_eq!(saved.tags[&app.projects[0].root], ["barpq", "foo"]);
    assert!(saved.pins.is_empty());
    // Suffix keys of the same paste burst cannot act after Enter closes the prompt.
    assert_eq!(
        app.event(
            Event::Key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)),
            now
        ),
        Action::None
    );
    assert!(!app.projects[0].pinned);
    // Only a quiet boundary observed by the reader ends the paste burst.
    app.input_idle(now + Duration::from_millis(51));
    assert_eq!(
        app.event(
            Event::Key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)),
            now + Duration::from_millis(51)
        ),
        Action::Rescan
    );
    assert!(app.projects[0].pinned);
}

#[test]
fn issue94_real_escape_still_cancels_after_sequence_timeout() {
    use crossterm::event::Event;
    let mut app = fixture_app();
    key(&mut app, KeyCode::Char('t'));
    let now = Instant::now();
    app.event(
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        now,
    );
    assert!(!app.expire_input(now + Duration::from_millis(49)));
    assert!(app.expire_input(now + Duration::from_millis(50)));
    assert!(app.organize.prompt.is_none());
}

#[test]
fn issue94_partial_terminator_cannot_dispatch_a_paste_suffix() {
    use crossterm::event::Event;
    let mut app = fixture_app();
    let now = Instant::now();
    key(&mut app, KeyCode::Char('t'));
    app.event(Event::Paste("foo".into()), now);
    app.event(
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        now,
    );
    assert_eq!(
        app.event(
            Event::Key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)),
            now
        ),
        Action::None
    );
    assert!(!app.projects[0].pinned);
}

#[test]
fn rebased_m3_launch_prompts_use_the_shared_paste_router() {
    use crossterm::event::Event;
    use session_board::model::Agent;
    let mut app = fixture_app();
    let root = app.projects[0].root.clone();
    let now = Instant::now();
    assert_eq!(key(&mut app, KeyCode::Char('n')), Action::New);
    assert_eq!(key(&mut app, KeyCode::Char('w')), Action::Worktree);
    app.open.begin(root.clone(), true);
    app.event(
        Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
        now,
    );
    app.event(Event::Paste("feat/\np\nq".into()), now);
    app.event(
        Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)),
        now,
    );
    app.event(
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::ALT)),
        now,
    );
    assert!(
        app.open.active(),
        "Alt-Escape must not cancel the branch prompt"
    );
    assert!(app.open.text().unwrap().contains("feat/pq"));
    for code in [
        KeyCode::Esc,
        KeyCode::Char('['),
        KeyCode::Char('2'),
        KeyCode::Char('0'),
        KeyCode::Char('1'),
        KeyCode::Char('~'),
    ] {
        app.event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)), now);
    }
    assert_eq!(
        app.event(
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            now
        ),
        Action::Launch {
            project: root,
            agent: Agent::Codex,
            branch: Some("feat/pq".into())
        }
    );
    assert!(app.state.pins.is_empty());
}

#[test]
fn rebased_m3_agent_picker_does_not_launch_pasted_choices() {
    use crossterm::event::Event;
    let mut app = fixture_app();
    let now = Instant::now();
    app.open.begin(app.projects[0].root.clone(), false);
    app.event(Event::Paste("x".into()), now);
    assert_eq!(
        app.event(
            Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
            now
        ),
        Action::None
    );
    assert!(app.open.active());
    app.input_idle(now + Duration::from_millis(51));
    app.event(
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        now + Duration::from_millis(51),
    );
    assert!(!app.open.active());
    assert_eq!(app.message, "launch cancelled");
}

#[test]
fn issue101_paste_suffix_stays_inert_without_an_observed_input_gap() {
    use crossterm::event::Event;
    let mut app = fixture_app();
    let now = Instant::now();
    app.event(Event::Paste("foo".into()), now);
    for i in 0..100 {
        assert_eq!(
            app.event(
                Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
                now + Duration::from_millis(i * 10)
            ),
            Action::None
        );
    }
    assert_eq!(
        app.event(
            Event::Key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)),
            now + Duration::from_secs(2)
        ),
        Action::None
    );
    assert!(!app.projects[0].pinned);
}

#[test]
fn issue102_prompt_edits_merge_against_tags_present_when_it_opened() {
    for edited in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.toml");
        let mut app = fixture_app();
        let root = app.projects[0].root.clone();
        State {
            tags: std::collections::BTreeMap::from([(root.clone(), vec!["base".into()])]),
            ..Default::default()
        }
        .save(&path)
        .unwrap();
        app.load_state(path.clone());
        app.project_organization();
        key(&mut app, KeyCode::Char('t'));
        let prior = State::load(&path).unwrap().0;
        let mut external = prior.clone();
        external
            .tags
            .get_mut(&root)
            .unwrap()
            .push("external".into());
        external.save_changes(&path, Some(&prior)).unwrap();
        app.replace(app.organize.projects.clone());
        if edited {
            for c in ", local".chars() {
                key(&mut app, KeyCode::Char(c));
            }
        }
        key(&mut app, KeyCode::Enter);
        let saved = State::load(&path).unwrap().0;
        assert_eq!(
            saved.tags[&root],
            if edited {
                vec!["base", "external", "local"]
            } else {
                vec!["base", "external"]
            }
        );
    }
}

fn attention_fixture_app() -> App {
    let mut app = fixture_app();
    app.state.first_seen = Some(0);
    app.replace(app.organize.projects.clone());
    app
}
#[test]
fn attention_focus_collapse_empty_and_cli_order() {
    let mut app = attention_fixture_app();
    let mut cli_projects = app.projects.clone();
    cli::filter(
        &mut cli_projects,
        &app.state,
        None,
        Some(cli::StatusFilter::Attention),
        None,
        false,
    );
    let expected = [
        "claude_code:waiting",
        "codex:waiting",
        "claude_code:error",
        "codex:error",
        "claude_code:stuck",
        "codex:stuck",
        "claude_code:done",
        "codex:done",
    ];
    assert_eq!(
        app.attention_items()
            .iter()
            .map(|i| i.session.id.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        cli_projects
            .iter()
            .flat_map(|p| &p.sessions)
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    key(&mut app, KeyCode::Tab);
    assert!(app.attention_focus);
    assert!(app.session().is_some());
    assert_eq!(key(&mut app, KeyCode::Char('n')), Action::New);
    assert_eq!(key(&mut app, KeyCode::Char('w')), Action::Worktree);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.attention_selected, 1);
    assert!(frame(&app, 100, 30).contains("Attention [focused]"));
    key(&mut app, KeyCode::Left);
    assert!(frame(&app, 100, 30).contains("to expand"));
    key(&mut app, KeyCode::Right);
    assert!(!app.attention_collapsed);
    key(&mut app, KeyCode::Tab);
    assert!(!app.attention_focus);
    app.replace(vec![]);
    key(&mut app, KeyCode::Tab);
    assert!(!app.attention_focus);
    assert!(!frame(&app, 100, 30).contains("Attention"));
}
#[test]
fn attention_overflow_is_bounded_and_navigable() {
    let mut app = attention_fixture_app();
    let mut sample = app.projects[0].sessions[0].clone();
    sample.status = session_board::model::Status::Waiting;
    app.projects[0].sessions = (0..20)
        .map(|n| {
            let mut s = sample.clone();
            s.id = format!("sample:{n:02}");
            s.transcript_path = format!("sample-{n:02}").into();
            s
        })
        .collect();
    app.replace(app.projects.clone());
    let rendered = frame(&app, 100, 30);
    assert!(rendered.contains("+13 below"));
    assert!(rendered.contains("20 need you"));
    assert!(rendered.lines().nth(10).unwrap().contains("Projects"));
    key(&mut app, KeyCode::Tab);
    for _ in 0..30 {
        key(&mut app, KeyCode::Down);
    }
    assert_eq!(app.session().unwrap().id, "sample:19");
    let output = frame(&app, 100, 30);
    assert!(output.contains("+13 above"));
    assert!(!output.contains("below"));
}

#[test]
fn attention_reflects_read_marks_on_the_next_frame() {
    let mut app = attention_fixture_app();
    let done = app
        .attention_items()
        .iter()
        .find(|item| item.session.status == session_board::model::Status::Done)
        .unwrap()
        .session
        .clone();
    let before = app.attention_items().len();
    app.state
        .read
        .insert(done.id.clone(), done.last_activity_at);
    assert_eq!(app.attention_items().len(), before - 1);
    assert!(
        !app.attention_items()
            .iter()
            .any(|item| item.session.id == done.id)
    );
    assert!(frame(&app, 100, 30).contains(&format!("{} need you", before - 1)));
}

#[test]
fn pty_screen_parser_handles_partial_escape_sequences() {
    let output = std::process::Command::new("python3")
        .arg("tests/terminal_screen.py")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn multi_project_app() -> App {
    let mut app = App::new(false);
    app.state.first_seen = Some(0);
    app.replace(serde_json::from_str(include_str!("fixtures/attention-projects.json")).unwrap());
    app
}
fn pane_highlight(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| app.render(f, NOW)).unwrap();
    let b = terminal.backend().buffer();
    (0..10)
        .find(|y| {
            (0..100).any(|x| {
                b[(x, *y)]
                    .modifier
                    .contains(ratatui::style::Modifier::REVERSED)
            })
        })
        .map(|y| (0..100).map(|x| b[(x, y)].symbol()).collect())
        .unwrap_or_default()
}
#[test]
fn focused_rescan_keeps_highlight_tree_enter_and_next_neighbour_together() {
    let mut app = multi_project_app();
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Down);
    let target = app.session().unwrap().clone();
    let removed = app.attention_items()[2].session.transcript_path.clone();
    let mut p = app.projects.clone();
    for project in &mut p {
        project.sessions.retain(|s| s.transcript_path != removed);
    }
    let mut new = target.clone();
    new.id = "new-top".into();
    new.title = "new-top".into();
    new.transcript_path = "new-top".into();
    new.last_activity_at = 99999;
    p[0].sessions.push(new);
    app.replace(p);
    assert_eq!(
        app.session().unwrap().transcript_path,
        target.transcript_path
    );
    assert!(pane_highlight(&app).contains(&target.title));
    assert_eq!(
        app.rows()[app.selected],
        ui::Row::Session(target.project_root.clone(), target.transcript_path.clone())
    );
    assert_eq!(key(&mut app, KeyCode::Enter), Action::Open);
    assert_eq!(
        app.session().unwrap().transcript_path,
        target.transcript_path
    );
    let path = app
        .attention_items()
        .iter()
        .skip_while(|i| i.session.transcript_path != target.transcript_path)
        .nth(1)
        .unwrap()
        .session
        .transcript_path
        .clone();
    key(&mut app, KeyCode::Down);
    assert_eq!(app.session().unwrap().transcript_path, path);
    assert!(pane_highlight(&app).contains(&app.session().unwrap().title));
}
#[test]
fn small_pane_collapsed_movement_and_folds() {
    let mut app = attention_fixture_app();
    let root = app.projects[0].root.clone();
    app.folded.insert(root.clone());
    key(&mut app, KeyCode::Tab);
    assert!(
        app.folded.contains(&root),
        "focusing attention must not unfold the tree"
    );
    key(&mut app, KeyCode::Down);
    assert!(app.folded.contains(&root));
    key(&mut app, KeyCode::Left);
    let selected = app.selected;
    let path = app.session().map(|s| s.transcript_path.clone());
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Up);
    assert_eq!(app.selected, selected);
    assert_eq!(app.session().map(|s| s.transcript_path.clone()), path);
    key(&mut app, KeyCode::Right);
    let small = frame(&app, 80, 12);
    assert!(
        small.lines().nth(4).unwrap().contains("Projects"),
        "{small}"
    );
    key(&mut app, KeyCode::Enter);
    assert!(!app.folded.contains(&root));
    assert_eq!(app.session().unwrap().id, "codex:waiting");
    assert_eq!(
        app.rows()[app.selected],
        ui::Row::Session(
            app.session().unwrap().project_root.clone(),
            app.session().unwrap().transcript_path.clone()
        )
    );
}

#[test]
fn tui_notifications_use_real_scan_events_and_config_without_blocking_help() {
    use std::{
        io::{Read, Seek},
        process::{Command, Stdio},
    };
    let mut stdout = tempfile::tempfile().unwrap();
    let mut stderr = tempfile::tempfile().unwrap();
    let mut child = Command::new("python3")
        .args(["tests/notify_tui.py", env!("CARGO_BIN_EXE_sboard")])
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .stdout(stdout.try_clone().unwrap())
        .stderr(stderr.try_clone().unwrap())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let timed_out = child.try_wait().unwrap().is_none();
    if timed_out {
        // SIGTERM lets Python's deadline handler run its board/notifier cleanup.
        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
        let cleanup = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() && Instant::now() < cleanup {
            std::thread::sleep(Duration::from_millis(10));
        }
        if child.try_wait().unwrap().is_none() {
            child.kill().unwrap();
        }
    }
    let status = child.wait().unwrap();
    stdout.rewind().unwrap();
    stderr.rewind().unwrap();
    let mut out = String::new();
    let mut err = String::new();
    stdout.read_to_string(&mut out).unwrap();
    stderr.read_to_string(&mut err).unwrap();
    assert!(
        !timed_out && status.success(),
        "tui_notifications_use_real_scan_events_and_config_without_blocking_help: waiting for Python probe exit (90 s limit, timed_out={timed_out})\n{out}\n{err}"
    );
}

#[test]
fn pane_has_handwritten_global_order_and_clean_project_names() {
    let mut app = multi_project_app();
    assert_eq!(
        app.attention_items()
            .iter()
            .map(|i| i.session.id.as_str())
            .collect::<Vec<_>>(),
        ["b-wait", "g-wait", "a-wait", "g-error", "b-error", "a-done"]
    );
    app.projects[0].name = "bad\n\u{1b}[31mproject\u{1b}[0m".into();
    app.replace(app.projects.clone());
    let output = frame(&app, 100, 30);
    let pane = output.lines().take(8).collect::<Vec<_>>().join("\n");
    assert!(pane.contains("bad project"), "{output}");
    assert!(!pane.contains("31m"));
}

#[test]
fn focused_queue_shrink_clamps_the_view_and_target() {
    let mut app = multi_project_app();
    key(&mut app, KeyCode::Tab);
    for _ in 0..20 {
        key(&mut app, KeyCode::Down);
    }
    let mut p = app.projects.clone();
    for project in &mut p {
        project.sessions.retain(|s| s.id == "b-wait");
    }
    app.replace(p);
    assert_eq!(app.session().unwrap().id, "b-wait");
    assert!(pane_highlight(&app).contains("b-wait"));
    assert_eq!(key(&mut app, KeyCode::Enter), Action::Open);
    assert_eq!(app.session().unwrap().id, "b-wait");
    app.replace(vec![]);
    assert!(!app.attention_focus);
    assert!(app.attention_path.is_none());
}

#[test]
fn attention_keeps_real_projects_and_shared_marks_in_tag_view() {
    let mut app = attention_fixture_app();
    let root = app.projects[0].root.clone();
    let count = app.attention_items().len();
    app.state
        .tags
        .insert(root.clone(), vec!["client".into(), "urgent".into()]);
    app.state.group = scan::Group::Tag;
    app.project_organization();
    assert_eq!(app.projects.len(), 2);
    assert_eq!(app.attention_items().len(), count);
    assert!(app.attention_items().iter().all(|i| i.project.root == root));
    let done = app
        .attention_items()
        .into_iter()
        .find(|i| i.session.status == session_board::model::Status::Done)
        .unwrap()
        .session
        .clone();
    app.state
        .read
        .insert(done.id.clone(), done.last_activity_at);
    assert_eq!(app.attention_items().len(), count - 1);
    key(&mut app, KeyCode::Tab);
    let selected = app.session().unwrap().id.clone();
    assert_eq!(key(&mut app, KeyCode::Char('p')), Action::Rescan);
    assert_eq!(app.state.pins, [root]);
    assert_eq!(key(&mut app, KeyCode::Char('a')), Action::Rescan);
    assert!(app.state.archived.contains(&selected));
    assert!(
        app.attention_items()
            .iter()
            .all(|i| i.session.id != selected)
    );
}

#[test]
fn attention_enter_uses_the_router_and_persists_its_selected_read_mark() {
    use crossterm::event::Event;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.toml");
    State {
        first_seen: Some(0),
        ..Default::default()
    }
    .save(&path)
    .unwrap();
    let mut app = attention_fixture_app();
    app.load_state(path.clone());
    let root = app.projects[0].root.clone();
    app.folded.insert(root.clone());
    let now = Instant::now();
    app.event(
        Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        now,
    );
    let selected = app.session().unwrap().clone();
    assert!(app.state.read.contains_key(&selected.id));
    assert_eq!(
        app.event(
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            now
        ),
        Action::Open
    );
    assert_eq!(
        app.session().unwrap().transcript_path,
        selected.transcript_path
    );
    assert!(!app.folded.contains(&root));
    app.event(
        Event::Key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE)),
        now,
    );
    assert_eq!(
        State::load(&path).unwrap().0.read[&selected.id],
        selected.last_activity_at
    );
}
