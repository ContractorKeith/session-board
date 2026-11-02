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
    app.message = format!("{} projects", result.projects.len());
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
    let deadline = Instant::now() + watch::DEBOUNCE + Duration::from_secs(1);
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
            .recv_timeout(Duration::from_secs(3) - started.elapsed())
            .unwrap();
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
    let (_watcher, requests) = watch::StoreWatcher::new(&[temp.path().to_owned()]).unwrap();
    let mut count = 0;
    for n in 0..8 {
        fs::write(temp.path().join("session.jsonl"), n.to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(1));
        count += requests.try_iter().count();
    }
    count += requests.try_iter().count();
    assert!(
        (3..=5).contains(&count),
        "received {count} requests during steady writes"
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
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().starts_with("panicked:"));
    scans.requests.try_send(()).unwrap();
    let initial = scans
        .results
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert_eq!(initial.projects[0].sessions[0].status, Status::Working);
    fs::copy("fixtures/claude_code/project/waiting.jsonl", &path).unwrap();
    let refreshed = scans
        .results
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
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
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap()
            .projects[0]
            .sessions[0]
            .status,
        Status::Working
    );
    assert_eq!(
        scans
            .results
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
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
    let (_watcher, requests) = watch::StoreWatcher::new(std::slice::from_ref(&watched)).unwrap();
    // Keep one file open so all 200 real writes finish within one second.
    use std::io::Write;
    let started = Instant::now();
    let mut file = fs::File::create(root.join("session.jsonl")).unwrap();
    for n in 0..200 {
        writeln!(file, "{n}").unwrap();
    }
    drop(file);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "producer exceeded the one-second burst budget"
    );
    requests.recv_timeout(Duration::from_secs(3)).unwrap();
    let count = 1 + drain_requests(&requests);
    assert!(
        (1..=2).contains(&count),
        "200-write burst produced {count} requests"
    );
    fs::remove_dir_all(&root).unwrap();
    // Let the backend deliver root removal before recreating it.
    std::thread::sleep(Duration::from_millis(200));
    fs::create_dir(&root).unwrap();
    fs::write(root.join("new.jsonl"), "{}").unwrap();
    requests.recv_timeout(Duration::from_secs(8)).unwrap();
    // Drain deletion/re-attachment scans; then prove the new directory is watched.
    std::thread::sleep(Duration::from_secs(3));
    requests.try_iter().for_each(drop);
    fs::write(root.join("new.jsonl"), "changed").unwrap();
    requests.recv_timeout(Duration::from_secs(3)).unwrap();
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
fn attention_focus_collapse_empty_and_cli_order() {
    let mut app = fixture_app();
    let mut cli_projects = app.projects.clone();
    cli::filter(
        &mut cli_projects,
        &app.state,
        None,
        Some(cli::StatusFilter::Attention),
        None,
        false,
    );
    assert_eq!(
        app.attention_items()
            .iter()
            .map(|i| &i.session.id)
            .collect::<Vec<_>>(),
        cli_projects
            .iter()
            .flat_map(|p| &p.sessions)
            .map(|s| &s.id)
            .collect::<Vec<_>>()
    );
    key(&mut app, KeyCode::Tab);
    assert!(app.attention_focus);
    assert!(app.session().is_some());
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
    let mut app = fixture_app();
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
    let rendered = frame(&app, 100, 30);
    assert!(rendered.contains("+13 more"));
    assert!(rendered.contains("20 need you"));
    assert!(rendered.lines().nth(10).unwrap().contains("Projects"));
    key(&mut app, KeyCode::Tab);
    for _ in 0..30 {
        key(&mut app, KeyCode::Down);
    }
    assert_eq!(app.session().unwrap().id, "sample:19");
    frame(&app, 100, 30);
}

#[test]
fn attention_reflects_read_marks_on_the_next_frame() {
    let mut app = fixture_app();
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
