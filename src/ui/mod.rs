mod detail;
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
    collections::HashSet,
    io::{self, IsTerminal},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Project(PathBuf),
    Session(String),
}

pub struct App {
    pub projects: Vec<Project>,
    pub folded: HashSet<PathBuf>,
    pub selected: usize,
    pub help: bool,
    pub message: String,
    pub color: bool,
}
#[derive(PartialEq, Debug)]
pub enum Action {
    None,
    Rescan,
    Quit,
}
impl App {
    pub fn new(color: bool) -> Self {
        Self {
            projects: vec![],
            folded: HashSet::new(),
            selected: 0,
            help: false,
            message: "scanning…".into(),
            color,
        }
    }
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = vec![];
        for project in &self.projects {
            rows.push(Row::Project(project.root.clone()));
            if !self.folded.contains(&project.root) {
                rows.extend(project.sessions.iter().map(|s| Row::Session(s.id.clone())));
            }
        }
        rows
    }
    pub fn session(&self) -> Option<&Session> {
        match self.rows().get(self.selected)? {
            Row::Session(id) => self
                .projects
                .iter()
                .flat_map(|p| &p.sessions)
                .find(|s| &s.id == id),
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
            KeyCode::Char('k') | KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('h' | 'l') | KeyCode::Left | KeyCode::Right => {
                if let Some(row) = rows.get(self.selected) {
                    let root = match row {
                        Row::Project(root) => Some(root.clone()),
                        Row::Session(id) => self
                            .projects
                            .iter()
                            .find(|p| p.sessions.iter().any(|s| &s.id == id))
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
            KeyCode::Enter => self.message = "open lands in M3".into(),
            KeyCode::Char('r') => {
                self.message = "scanning…".into();
                return Action::Rescan;
            }
            _ => {}
        }
        Action::None
    }
    pub fn render(&self, frame: &mut Frame, now: i64) {
        let vertical =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(frame.area());
        if self.help {
            frame.render_widget(Paragraph::new("j / k / ↑ / ↓  move\nh / l / ← / →  fold / unfold project\nEnter  open lands in M3\nr  rescan\n?  toggle help\nq / Ctrl-C  quit").block(Block::default().borders(Borders::ALL).title("Help")), vertical[0]);
        } else {
            let panes =
                Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                    .split(vertical[0]);
            tree::render(self, frame, panes[0], now);
            detail::render(self, frame, panes[1]);
        }
        frame.render_widget(
            Paragraph::new(format!("{} | ? help · r rescan · q quit", self.message)),
            vertical[1],
        );
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
    let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
}
pub fn run() -> io::Result<i32> {
    if !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "stdout is not a terminal; run sboard list for script output.",
        ));
    }
    let scans = Scans::start()?;
    terminal_session(|terminal| {
        let mut app = App::new(std::env::var_os("NO_COLOR").is_none());
        if let Some(warning) = &scans.warning {
            app.message.clone_from(warning);
        }
        event_loop(terminal, &mut app, &scans)
    })?;
    Ok(0)
}

pub(crate) fn terminal_session(
    operation: impl FnOnce(&mut DefaultTerminal) -> io::Result<()>,
) -> io::Result<()> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
    let _guard = TerminalGuard;
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))?;
    operation(&mut terminal)
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App, scans: &Scans) -> io::Result<()> {
    loop {
        while scans.changes.try_recv().is_ok() {
            let _ = scans.requests.try_send(());
        }
        while let Ok(result) = scans.results.try_recv() {
            match result {
                Ok(result) => {
                    app.message = result
                        .warnings
                        .first()
                        .cloned()
                        .unwrap_or_else(|| format!("{} projects", result.projects.len()));
                    if let Some(warning) = &scans.warning {
                        app.message.clone_from(warning);
                    }
                    app.replace(result.projects);
                }
                Err(error) => app.message = format!("scan: {error}"),
            }
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        terminal.draw(|frame| app.render(frame, now))?;
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            match app.key(key) {
                Action::Quit => return Ok(()),
                Action::Rescan => {
                    let _ = scans.requests.try_send(());
                }
                Action::None => {}
            }
        }
    }
}
