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
    assert_eq!(key(&mut app, KeyCode::Enter), Action::None);
    assert_eq!(app.message, "open lands in M3");
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
        assert!(requests.recv_timeout(Duration::from_millis(300)).is_err());
    }
}

// Invoked in a child with piped output, or under a PTY for manual hygiene checks.
#[test]
#[ignore]
fn terminal_probe() {
    match std::env::var("SBOARD_TERMINAL_PROBE").as_deref() {
        Ok("panic") => {
            let _ = ui::terminal_session(|_| panic!("terminal probe"));
        }
        Ok("error") => {
            assert!(
                ui::terminal_session(|_| Err(std::io::Error::other("terminal probe"))).is_err()
            );
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
