use super::{Action, App, Row};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use session_board::{
    cli,
    scan::{self, Group, Sort},
    state::State,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Organize {
    pub paste_burst: bool,
    pub terminator: Vec<KeyEvent>,
    pub terminator_progress: usize,
    pub terminator_deadline: Option<Instant>,
    pub snapshot: State,
    pub dirty: bool,
    pub read_deadline: Option<std::time::Instant>,
    pub path: Option<PathBuf>,
    pub projects: Vec<session_board::model::Project>,
    pub prompt: Option<TagPrompt>,
    pub missing_projects: std::collections::HashSet<PathBuf>,
}
pub struct TagPrompt {
    pub root: PathBuf,
    pub text: String,
    pub tags: Vec<String>,
}
impl App {
    /// All terminal events pass here, including fake streams in the regression tests.
    pub fn event(&mut self, event: Event, now: Instant) -> Action {
        match event {
            Event::Paste(text) => {
                self.flush_terminator();
                self.organize.paste_burst = true;
                self.paste(&text);
                Action::None
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if self.organize.prompt.is_none()
                    && !self.open.accepts_text()
                    && self.organize.paste_burst
                {
                    self.organize.terminator.clear();
                    self.organize.terminator_progress = 0;
                    self.organize.terminator_deadline = None;
                    return Action::None;
                }
                const END: [KeyCode; 6] = [
                    KeyCode::Esc,
                    KeyCode::Char('['),
                    KeyCode::Char('2'),
                    KeyCode::Char('0'),
                    KeyCode::Char('1'),
                    KeyCode::Char('~'),
                ];
                if self.organize.terminator_progress > 0 {
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && key.code == END[self.organize.terminator_progress]
                    {
                        self.organize.terminator.push(key);
                        self.organize.terminator_progress += 1;
                        if self.organize.terminator_progress == END.len() {
                            self.organize.terminator.clear();
                            self.organize.terminator_progress = 0;
                            self.organize.terminator_deadline = None;
                        }
                        return Action::None;
                    }
                    self.flush_terminator();
                    // Escape may have closed the prompt; recheck the paste quarantine.
                    return self.event(Event::Key(key), now);
                }
                let progress = if key.code == KeyCode::Esc && key.modifiers.is_empty() {
                    1
                } else if key.code == KeyCode::Char('[') && key.modifiers == KeyModifiers::ALT {
                    2
                } else {
                    0
                };
                if progress > 0 {
                    self.organize.terminator.push(key);
                    self.organize.terminator_progress = progress;
                    self.organize.terminator_deadline = Some(now + Duration::from_millis(50));
                    Action::None
                } else {
                    self.key(key)
                }
            }
            _ => Action::None,
        }
    }
    pub fn input_idle(&mut self, received_at: Instant) -> bool {
        let changed = self.expire_input(received_at);
        self.organize.paste_burst = false;
        changed
    }
    pub fn discard_input(&mut self) {
        self.organize.paste_burst = false;
        self.organize.terminator.clear();
        self.organize.terminator_progress = 0;
        self.organize.terminator_deadline = None;
    }
    pub fn expire_input(&mut self, now: Instant) -> bool {
        if self
            .organize
            .terminator_deadline
            .is_some_and(|until| now >= until)
        {
            self.flush_terminator();
            true
        } else {
            false
        }
    }
    fn flush_terminator(&mut self) {
        self.organize.terminator_progress = 0;
        self.organize.terminator_deadline = None;
        // A partial terminator contains only Escape, [, digits and ~: no action keys.
        for key in std::mem::take(&mut self.organize.terminator) {
            self.key(key);
        }
    }
    pub fn load_state(&mut self, path: PathBuf) {
        if self.organize.path.is_none() {
            self.scan_warnings.extend(State::cleanup(&path));
        }
        match State::load(&path) {
            Ok((state, warnings)) => {
                self.state = if self.organize.dirty {
                    self.state.merge(&self.organize.snapshot, state.clone())
                } else {
                    state.clone()
                };
                self.organize.snapshot = state;
                self.scan_warnings.extend(warnings);
            }
            Err(error) => self.message = format!("state: {error}"),
        }
        self.organize.path = Some(path);
    }
    fn save_organization(&mut self) {
        self.organize.dirty = true;
        if let Some(path) = self.organize.path.clone() {
            match self
                .state
                .save_changes(&path, Some(&self.organize.snapshot))
            {
                Ok(warnings) => {
                    self.organize.dirty = false;
                    self.organize.read_deadline = None;
                    self.load_state(path);
                    if !warnings.is_empty() {
                        self.message = warnings.join("; ");
                    }
                }
                Err(error) => {
                    self.message = format!("state: {error}");
                    self.organize.read_deadline =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
                }
            }
        }
        self.project_organization();
    }
    pub fn flush_read_marks(&mut self, force: bool) {
        if self.organize.dirty
            && (force
                || self
                    .organize
                    .read_deadline
                    .is_some_and(|due| std::time::Instant::now() >= due))
        {
            self.save_organization();
        }
    }
    fn paste(&mut self, text: &str) {
        if let Some(prompt) = &mut self.organize.prompt {
            prompt.text.extend(text.chars().filter(|c| !c.is_control()));
        } else {
            self.open.paste(text);
        }
    }
    pub fn project_organization(&mut self) {
        let mut projects = self.organize.projects.clone();
        let state = &self.state;
        for project in &mut projects {
            project.pinned = state.pins.contains(&project.root);
            project.tags = state.tags.get(&project.root).cloned().unwrap_or_default();
            project.archived = state
                .archived
                .contains(&project.root.to_string_lossy().into_owned());
        }
        cli::filter(
            &mut projects,
            state,
            None,
            None,
            None,
            self.state.show_archived,
        );
        scan::organize(&mut projects, state, state.sort, state.group);
        let selected = self.rows().get(self.selected).cloned();
        self.projects = projects;
        let rows = self.rows();
        self.selected = selected
            .and_then(|row| rows.iter().position(|r| r == &row))
            .unwrap_or(self.selected.min(rows.len().saturating_sub(1)));
        self.reconcile_attention();
    }
    fn selected_root(&self) -> Option<PathBuf> {
        if let Some(session) = self.session() {
            return Some(session.project_root.clone());
        }
        match self.rows().get(self.selected)? {
            Row::Project(root) if self.state.group == Group::Project => Some(root.clone()),
            _ => None,
        }
    }
    pub fn mark_selected(&mut self, project: bool) {
        let sessions = if let Some(session) = self.session() {
            vec![session.clone()]
        } else if project {
            self.projects
                .iter()
                .find(|p| Some(&Row::Project(p.root.clone())) == self.rows().get(self.selected))
                .map(|p| p.sessions.clone())
                .unwrap_or_default()
        } else {
            vec![]
        };
        let mut changed = false;
        for session in sessions {
            if self.state.read.get(&session.id).is_none_or(|seen| {
                *seen < session.last_activity_at
                    || *seen
                        > std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64
            }) {
                self.state.read.insert(session.id, session.last_activity_at);
                changed = true;
            }
        }
        if changed {
            self.organize.dirty = true;
            self.organize.read_deadline.get_or_insert_with(|| {
                std::time::Instant::now() + std::time::Duration::from_secs(2)
            });
            self.project_organization();
            if project {
                self.save_organization();
            }
        }
    }
    pub fn organize_key(&mut self, code: KeyCode) -> Option<Action> {
        if let Some(mut prompt) = self.organize.prompt.take() {
            match code {
                KeyCode::Esc => {
                    self.message.clear();
                    return Some(Action::None);
                }
                KeyCode::Enter => {
                    let mut tags: Vec<String> = prompt
                        .text
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect();
                    tags.sort();
                    tags.dedup();
                    let current = self.state.tags.entry(prompt.root).or_default();
                    current.retain(|tag| !prompt.tags.contains(tag) || tags.contains(tag));
                    current.extend(tags.into_iter().filter(|tag| !prompt.tags.contains(tag)));
                    current.sort();
                    current.dedup();
                    self.message = "tags saved".into();
                    self.save_organization();
                    return Some(Action::Rescan);
                }
                KeyCode::Backspace => {
                    prompt.text.pop();
                }
                KeyCode::Char(c) if !c.is_control() => prompt.text.push(c),
                _ => {}
            }
            self.organize.prompt = Some(prompt);
            return Some(Action::None);
        }
        match code {
            KeyCode::Char('p') => {
                if let Some(root) = self.selected_root() {
                    if self.state.pins.contains(&root) {
                        self.state.pins.retain(|p| p != &root);
                    } else {
                        self.state.pins.push(root);
                    }
                } else {
                    return Some(Action::None);
                }
            }
            KeyCode::Char('t') => {
                if let Some(root) = self.selected_root() {
                    let tags = self.state.tags.get(&root).cloned().unwrap_or_default();
                    self.organize.prompt = Some(TagPrompt {
                        root,
                        text: tags.join(", "),
                        tags,
                    });
                }
                return Some(Action::None);
            }
            KeyCode::Char('m') => {
                self.mark_selected(true);
                self.flush_read_marks(true);
                return Some(Action::Rescan);
            }
            KeyCode::Char('a') => {
                let entry = self.session().map(|s| s.id.clone()).or_else(|| {
                    self.selected_root()
                        .map(|p| p.to_string_lossy().into_owned())
                });
                if let Some(entry) = entry {
                    if self.state.archived.contains(&entry) {
                        self.state.archived.retain(|a| a != &entry);
                    } else {
                        self.state.archived.push(entry);
                    }
                } else {
                    return Some(Action::None);
                }
            }
            KeyCode::Char('A') => {
                self.state.show_archived = !self.state.show_archived;
            }
            KeyCode::Char('s') => {
                self.state.sort = match self.state.sort {
                    Sort::Activity => Sort::Attention,
                    Sort::Attention => Sort::Activity,
                }
            }
            KeyCode::Char('g') => {
                self.state.group = match self.state.group {
                    Group::Project => Group::Tag,
                    Group::Tag => Group::Project,
                }
            }
            _ => return None,
        }
        self.save_organization();
        Some(Action::Rescan)
    }
}
