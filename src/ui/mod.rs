mod detail;
mod open;
mod tree;

use crate::watch::Scans;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout},
    widgets::{Block, Borders, Paragraph},
};
use session_board::model::{Project, Session};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    io::{self, IsTerminal},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Project(PathBuf),
    Session(PathBuf),
}

pub struct App {
    pub projects: Vec<Project>,
    pub folded: HashSet<PathBuf>,
    pub selected: usize,
    pub help: bool,
    pub message: String,
    pub summary: String,
    pub color: bool,
    pub list: RefCell<ratatui::widgets::ListState>,
    pub page_size: Cell<usize>,
    pub scan_warnings: Vec<String>,
    pub watch_warnings: Vec<String>,
}
#[derive(PartialEq, Debug)]
pub enum Action {
    None,
    Rescan,
    Open,
    New,
    Worktree,
    Quit,
}
impl App {
    pub fn new(color: bool) -> Self {
        Self {
            projects: vec![],
            folded: HashSet::new(),
            selected: 0,
            help: false,
            message: String::new(),
            summary: "scanning…".into(),
            color,
            list: RefCell::new(ratatui::widgets::ListState::default()),
            page_size: Cell::new(10),
            scan_warnings: vec![],
            watch_warnings: vec![],
        }
    }
    pub(super) fn suspend<T>(
        &mut self,
        terminal: &mut DefaultTerminal,
        f: impl FnOnce() -> io::Result<T>,
    ) -> io::Result<T> {
        restore();
        let result = f();
        session_board::launch::restore_terminal();
        terminal::enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        execute!(
            io::stdout(),
            terminal::Clear(terminal::ClearType::All),
            crossterm::cursor::MoveTo(0, 0)
        )?;
        terminal.resize(terminal.size()?.into())?;
        result
    }
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = vec![];
        for project in &self.projects {
            rows.push(Row::Project(project.root.clone()));
            if !self.folded.contains(&project.root) {
                rows.extend(
                    project
                        .sessions
                        .iter()
                        .map(|s| Row::Session(s.transcript_path.clone())),
                );
            }
        }
        rows
    }
    pub fn session(&self) -> Option<&Session> {
        match self.rows().get(self.selected)? {
            Row::Session(path) => self
                .projects
                .iter()
                .flat_map(|p| &p.sessions)
                .find(|s| &s.transcript_path == path),
            _ => None,
        }
    }
    pub fn replace(&mut self, projects: Vec<Project>) {
        let selected = self.rows().get(self.selected).cloned();
        self.projects = projects;
        let rows = self.rows();
        self.selected = selected
            .and_then(|row| rows.iter().position(|r| r == &row))
            .unwrap_or(self.selected.min(rows.len().saturating_sub(1)));
    }
    pub fn key(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        if key.code == KeyCode::Char('q')
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            return Action::Quit;
        }
        if key.code == KeyCode::Char('?') {
            self.help = !self.help;
            return Action::None;
        }
        if self.help {
            return Action::None;
        }
        let rows = self.rows();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.selected = (self.selected + 1).min(rows.len().saturating_sub(1))
            }
            KeyCode::PageDown => {
                self.selected = self
                    .selected
                    .saturating_add(self.page_size.get())
                    .min(rows.len().saturating_sub(1))
            }
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(self.page_size.get()),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = rows.len().saturating_sub(1),
            KeyCode::Char('k') | KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('h' | 'l') | KeyCode::Left | KeyCode::Right => {
                if let Some(row) = rows.get(self.selected) {
                    let root = match row {
                        Row::Project(root) => Some(root.clone()),
                        Row::Session(path) => self
                            .projects
                            .iter()
                            .find(|p| p.sessions.iter().any(|s| &s.transcript_path == path))
                            .map(|p| p.root.clone()),
                    };
                    if let Some(root) = root {
                        if matches!(key.code, KeyCode::Char('h') | KeyCode::Left) {
                            self.folded.insert(root.clone());
                            self.selected = self
                                .rows()
                                .iter()
                                .position(|r| r == &Row::Project(root.clone()))
                                .unwrap_or(0);
                        } else {
                            self.folded.remove(&root);
                        }
                    }
                }
            }
            KeyCode::Enter => return Action::Open,
            KeyCode::Char('n') => return Action::New,
            KeyCode::Char('w') => return Action::Worktree,
            KeyCode::Char('r') => {
                self.summary = "scanning…".into();
                return Action::Rescan;
            }
            _ => {}
        }
        Action::None
    }
    pub fn status_text(&self) -> String {
        let message = if self.message.is_empty() {
            &self.summary
        } else {
            &self.message
        };
        let warnings: Vec<_> = self
            .watch_warnings
            .iter()
            .chain(&self.scan_warnings)
            .collect();
        match warnings.first() {
            Some(first) => format!("{} | {} warnings: {first}", message, warnings.len()),
            None => message.clone(),
        }
    }
    pub fn request_scan(&mut self, scans: &Scans) {
        if let Err(std::sync::mpsc::TrySendError::Disconnected(_)) = scans.requests.try_send(()) {
            self.message = "scan worker died; restart sboard".into();
            self.scan_warnings.clear();
        }
    }
    pub fn render(&self, frame: &mut Frame, now: i64) {
        let vertical =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(frame.area());
        if self.help {
            frame.render_widget(Paragraph::new("j / k / ↑ / ↓  move\nPgUp / PgDn / Home / End  page / ends\nh / l / ← / →  fold / unfold project\nEnter open · n new · w new in worktree\nr  rescan\n?  toggle help\nq / Ctrl-C  quit").block(Block::default().borders(Borders::ALL).title("Help")), vertical[0]);
        } else {
            let panes =
                Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                    .split(vertical[0]);
            tree::render(self, frame, panes[0], now);
            detail::render(self, frame, panes[1]);
        }
        let hint = "? help · r rescan · q quit";
        let bar = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(hint.chars().count() as u16),
        ])
        .split(vertical[1]);
        frame.render_widget(Paragraph::new(self.status_text()), bar[0]);
        frame.render_widget(Paragraph::new(hint), bar[1]);
    }
}

// The guard restores the terminal on setup errors, event-loop errors and unwinding.
struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}
fn restore() {
    let _ = terminal::disable_raw_mode();
    session_board::launch::restore_terminal();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
}
pub fn color_enabled(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_none_or(std::ffi::OsStr::is_empty)
}
pub fn run() -> io::Result<i32> {
    if !io::stdout().is_terminal()
        || !io::stdin().is_terminal()
        || std::env::var("TERM").as_deref() == Ok("dumb")
    {
        return Err(io::Error::other(
            "interactive terminal required; run sboard list for script output.",
        ));
    }
    let signal = Arc::new(AtomicUsize::new(0));
    let mut registrations = vec![];
    for sig in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        registrations.push(signal_hook::flag::register_usize(
            sig,
            signal.clone(),
            sig as usize,
        )?);
    }
    let result = terminal_session(|terminal| {
        let scans = Scans::start()?;
        let mut app = App::new(color_enabled(std::env::var_os("NO_COLOR").as_deref()));
        event_loop(terminal, &mut app, &scans, &signal)
    });
    for id in registrations {
        signal_hook::low_level::unregister(id);
    }
    let signal = signal.load(Ordering::Relaxed);
    if signal != 0 {
        return Ok(128 + signal as i32);
    }
    result?;
    Ok(0)
}

pub(crate) fn terminal_session(
    operation: impl FnOnce(&mut DefaultTerminal) -> io::Result<()>,
) -> io::Result<()> {
    session_board::launch::capture_terminal()?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("main") {
            restore();
        }
        previous(info);
    }));
    let _guard = TerminalGuard;
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))?;
    operation(&mut terminal)
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    scans: &Scans,
    signal: &AtomicUsize,
) -> io::Result<()> {
    // Crossterm 0.29 can spin internally on tty EOF/EIO; keep signals and tty
    // closure observable on main, with no outstanding read while a child runs.
    let (poll, requests) = std::sync::mpsc::channel();
    let (events, input) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while requests.recv().is_ok() {
            let result = event::poll(Duration::from_millis(50)).and_then(|ready| {
                if ready {
                    event::read().map(Some)
                } else {
                    Ok(None)
                }
            });
            let failed = result.is_err();
            if events.send(result).is_err() || failed {
                break;
            }
        }
    });
    let mut pending = false;
    let mut open = open::Open::default();
    let mut dirty = true;
    let mut worker_dead = false;
    let mut next_age = Instant::now();
    loop {
        if signal.load(Ordering::Relaxed) != 0 {
            return Ok(());
        }
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(io::Error::other("terminal closed"));
        }
        while let Ok(warnings) = scans.status.try_recv() {
            app.watch_warnings = warnings;
            dirty = true;
        }
        loop {
            let result = match scans.results.try_recv() {
                Ok(result) => result,
                Err(std::sync::mpsc::TryRecvError::Disconnected) if !worker_dead => {
                    app.summary = "scan worker died; restart sboard".into();
                    app.scan_warnings.clear();
                    worker_dead = true;
                    dirty = true;
                    break;
                }
                Err(_) => break,
            };
            match result {
                Ok(result) => {
                    app.summary = format!("{} projects", result.projects.len());
                    app.scan_warnings = result.warnings;
                    app.replace(result.projects);
                }
                Err(error) => {
                    // The forwarded panic hook may have scrolled stderr through the frame.
                    // Reset the fullscreen buffers without querying the cursor position.
                    terminal.resize(terminal.size()?.into())?;
                    app.scan_warnings.clear();
                    app.summary = format!("scan: {error}");
                }
            }
            dirty = true;
        }
        if Instant::now() >= next_age {
            dirty = true;
            next_age = Instant::now() + Duration::from_secs(1);
        }
        if dirty {
            if let Some(text) = open.text() {
                app.message = text;
            }
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            terminal.draw(|frame| app.render(frame, now))?;
            dirty = false;
        }
        // One outstanding read: the reader waits while a child owns the terminal.
        if !pending {
            poll.send(())
                .map_err(|_| io::Error::other("terminal reader stopped"))?;
            pending = true;
        }
        let event = match input.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => {
                pending = false;
                result?
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return Err(io::Error::other("terminal reader stopped")),
        };
        match event {
            Some(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                app.message.clear();
                if !open.key(app, key, terminal)? {
                    match app.key(key) {
                        Action::Quit => return Ok(()),
                        Action::Rescan => app.request_scan(scans),
                        Action::None => {}
                        action => open.action(app, action, terminal)?,
                    }
                }
                if session_board::launch::take_child_interrupt() {
                    let _ = signal.compare_exchange(
                        signal_hook::consts::SIGINT as usize,
                        0,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    );
                }
                dirty = true;
            }
            Some(Event::Resize(_, _)) => dirty = true,
            _ => {}
        }
    }
}
