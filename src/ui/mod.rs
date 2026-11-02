mod attention;
mod detail;
mod input;
mod open;
mod organize;
mod output;
mod tree;

use crate::watch::Scans;
use crossterm::{
    event::{
        DisableBracketedPaste, EnableBracketedPaste, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Frame,
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

type DefaultTerminal = ratatui::Terminal<ratatui::backend::CrosstermBackend<output::Output>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Project(PathBuf),
    Session(PathBuf, PathBuf),
}

pub struct App {
    pub state: session_board::state::State,
    pub open: open::Open,
    pub organize: organize::Organize,
    pub attention_focus: bool,
    pub attention_selected: usize,
    pub attention_path: Option<PathBuf>,
    pub attention_collapsed: bool,
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
    pub notify_warnings: Vec<String>,
}
#[derive(PartialEq, Debug)]
pub enum Action {
    None,
    Rescan,
    Open,
    New,
    Worktree,
    Launch {
        project: PathBuf,
        agent: session_board::model::Agent,
        branch: Option<String>,
    },
    Quit,
}
impl App {
    pub fn new(color: bool) -> Self {
        Self {
            state: Default::default(),
            open: open::Open::default(),
            organize: organize::Organize::default(),
            attention_focus: false,
            attention_selected: 0,
            attention_path: None,
            attention_collapsed: false,
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
            notify_warnings: vec![],
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
        execute!(output::Output, EnterAlternateScreen, EnableBracketedPaste)?;
        execute!(
            output::Output,
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
                        .map(|s| Row::Session(project.root.clone(), s.transcript_path.clone())),
                );
            }
        }
        rows
    }
    pub fn session(&self) -> Option<&Session> {
        if self.attention_focus && !self.attention_collapsed {
            return self.attention_path.as_ref().and_then(|path| {
                self.organize
                    .projects
                    .iter()
                    .flat_map(|p| &p.sessions)
                    .find(|s| &s.transcript_path == path)
            });
        }
        match self.rows().get(self.selected)? {
            Row::Session(root, path) => self
                .projects
                .iter()
                .find(|p| &p.root == root)
                .and_then(|p| p.sessions.iter().find(|s| &s.transcript_path == path)),
            _ => None,
        }
    }
    pub fn replace(&mut self, projects: Vec<Project>) {
        let selected = self.rows().get(self.selected).cloned();
        if let Some(path) = self.organize.path.clone() {
            self.load_state(path);
        }
        self.organize.projects = projects;
        self.project_organization();
        let rows = self.rows();
        self.selected = selected
            .and_then(|row| rows.iter().position(|r| r == &row))
            .unwrap_or(self.selected.min(rows.len().saturating_sub(1)));
        self.reconcile_attention();
        self.mark_selected(false);
    }

    pub fn key(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release || key.modifiers.contains(KeyModifiers::ALT) {
            return Action::None;
        }
        self.message.clear();
        if self.open.active() {
            let mut open = std::mem::take(&mut self.open);
            let action = open.key(self, key).unwrap_or(Action::None);
            self.open = open;
            return action;
        }
        if self.organize.prompt.is_some() {
            if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.organize.prompt = None;
                }
                return Action::None;
            }
            return self.organize_key(key.code).unwrap_or(Action::None);
        }
        if key.code == KeyCode::Char('q')
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            self.flush_read_marks(true);
            return Action::Quit;
        }
        if key.code == KeyCode::Char('?') {
            self.help = !self.help;
            return Action::None;
        }
        if self.help {
            return Action::None;
        }
        if let Some(action) = self.attention_key(key.code) {
            self.mark_selected(false);
            self.reconcile_attention();
            return action;
        }
        if let Some(action) = self.organize_key(key.code) {
            self.mark_selected(false);
            return action;
        }
        let before = self.rows().get(self.selected).cloned();
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
                        Row::Session(root, _) => Some(root.clone()),
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
        if before != self.rows().get(self.selected).cloned() {
            self.mark_selected(false);
        }
        Action::None
    }
    pub fn status_text(&self) -> String {
        if let Some(prompt) = &self.organize.prompt {
            return format!("Tags: {} (Enter save, Esc cancel)", prompt.text);
        }
        if let Some(text) = self.open.text() {
            return text;
        }
        let warnings: Vec<_> = self
            .watch_warnings
            .iter()
            .chain(&self.scan_warnings)
            .chain(&self.notify_warnings)
            .collect();
        let message = if self.message.is_empty() {
            &self.summary
        } else {
            &self.message
        };
        match warnings.first() {
            Some(first) => format!("{message} | {} warnings: {first}", warnings.len()),
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
        let items = self.attention_items();
        let vertical =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(frame.area());
        if self.help {
            frame.render_widget(Paragraph::new("Tab  attention / tree\nAttention focus: h / l collapse / expand; j / k move\nAttention focus: Enter opens selected session\nj / k / ↑ / ↓  move\nPgUp / PgDn / Home / End  page / ends\nTree focus: h / l / ← / → fold / unfold project\nEnter open · n new · w new in worktree\np pin · t tags · m read · a archive · A archived\ns sort · g group by tag / project\nr  rescan\n?  toggle help\nq / Ctrl-C  quit").block(Block::default().borders(Borders::ALL).title("Help")), vertical[0]);
        } else {
            let content = Layout::vertical([
                Constraint::Length(attention::height(
                    items.len(),
                    self.attention_collapsed,
                    frame.area().height,
                )),
                Constraint::Min(0),
            ])
            .split(vertical[0]);
            attention::render(self, &items, frame, content[0], now);
            let panes =
                Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                    .split(content[1]);
            tree::render(self, frame, panes[0], now);
            detail::render(self, frame, panes[1]);
        }
        let hint = "? help · r rescan · q quit";
        let bar = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(hint.chars().count() as u16),
        ])
        .split(vertical[1]);
        frame.render_widget(
            Paragraph::new({
                let count = items.len();
                if count > 0 {
                    format!("{count} need you · {}", self.status_text())
                } else {
                    self.status_text()
                }
            }),
            bar[0],
        );
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
    let _ = execute!(
        output::Output,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
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
        app.load_state(session_board::state::State::path()?);
        event_loop(terminal, &mut app, &scans, &signal)
    });
    for id in registrations {
        signal_hook::low_level::unregister(id);
    }
    let signal = signal.load(Ordering::Relaxed);
    if signal != 0 {
        return Ok(128 + signal as i32);
    }
    if output::closed() {
        return Ok(128 + signal_hook::consts::SIGHUP);
    }
    result?;
    Ok(0)
}

pub(crate) fn terminal_session(
    operation: impl FnOnce(&mut DefaultTerminal) -> io::Result<()>,
) -> io::Result<()> {
    output::reset();
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
    execute!(output::Output, EnterAlternateScreen, EnableBracketedPaste)?;
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(output::Output))?;
    operation(&mut terminal)
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    scans: &Scans,
    signal: &AtomicUsize,
) -> io::Result<()> {
    let reader = input::Reader::start();
    let mut transitions = session_board::notify::Transitions::default();
    let mut desktop = session_board::notify::Delivery::desktop()?;
    let mut dirty = true;
    let mut worker_dead = false;
    let mut next_age = Instant::now();
    loop {
        if signal.load(Ordering::Relaxed) != 0 {
            app.flush_read_marks(true);
            return Ok(());
        }
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(io::Error::other("terminal closed"));
        }
        let notification_warnings = desktop.take_warnings();
        if !notification_warnings.is_empty() {
            app.notify_warnings.extend(notification_warnings);
            dirty = true;
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
                    app.organize.missing_projects = result.missing_projects.into_iter().collect();
                    app.replace(result.projects);
                    if let Ok((config, _)) = session_board::config::Config::path()
                        .and_then(|path| session_board::config::Config::load(&path))
                    {
                        transitions.scan_with_state(
                            &app.organize.projects,
                            &app.state,
                            config.notify,
                            &mut desktop,
                        );
                    }
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
        app.flush_read_marks(false);
        if Instant::now() >= next_age {
            dirty = true;
            next_age = Instant::now() + Duration::from_secs(1);
        }
        if dirty {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            terminal.draw(|frame| app.render(frame, now))?;
            dirty = false;
        }
        let received = match reader.events.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => result?,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return Err(io::Error::other("terminal reader stopped")),
        };
        let (event, received_at) = match received {
            input::Input::Event(event, at) => (event, at),
            input::Input::Idle(at) => {
                dirty |= app.input_idle(at);
                continue;
            }
            input::Input::Paused | input::Input::Resumed => continue,
        };
        {
            match app.event(event, received_at) {
                Action::Quit => return Ok(()),
                Action::Rescan => app.request_scan(scans),
                Action::None => {}
                action => {
                    let transfer = matches!(action, Action::Open | Action::Launch { .. });
                    if transfer {
                        reader.pause()?;
                        app.discard_input();
                    }
                    let mut open = std::mem::take(&mut app.open);
                    let result = open.action(app, action, terminal);
                    app.open = open;
                    if transfer {
                        reader.resume()?;
                        app.discard_input();
                    }
                    result?;
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
    }
}
