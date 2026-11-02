use super::{Action, App, Row};
use crossterm::event::KeyCode;
use session_board::{
    cli,
    scan::{self, Group, Sort},
    state::State,
};
use std::path::PathBuf;

#[derive(Default)]
pub struct Organize {
    pub state: State,
    pub path: Option<PathBuf>,
    pub projects: Vec<session_board::model::Project>,
    pub prompt: Option<(PathBuf, String)>,
    pub missing_projects: std::collections::HashSet<PathBuf>,
}
impl App {
    pub fn load_state(&mut self, path: PathBuf) {
        match State::load(&path) {
            Ok((mut state, mut warnings)) => {
                if let Ok(home) = cli::home() {
                    warnings.extend(session_board::project::normalize_settings(
                        &mut session_board::config::Config::default(),
                        &mut state,
                        &home,
                    ));
                }
                self.organize.state = state;
                self.scan_warnings.extend(warnings);
            }
            Err(error) => self.message = error.to_string(),
        }
        self.organize.path = Some(path);
    }
    fn save_organization(&mut self, previous: &State) {
        if let Some(path) = &self.organize.path {
            match self.organize.state.save_changes(path, Some(previous)) {
                Ok(()) => match State::load(path) {
                    Ok((state, warnings)) => {
                        self.organize.state = state;
                        self.scan_warnings.extend(warnings);
                    }
                    Err(error) => self.message = format!("state: {error}"),
                },
                Err(error) => self.message = format!("state: {error}"),
            }
        }
        self.project_organization();
    }
    pub fn project_organization(&mut self) {
        let mut projects = self.organize.projects.clone();
        let state = &self.organize.state;
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
            self.organize.state.show_archived,
        );
        scan::organize(&mut projects, state, state.sort, state.group);
        let selected = self.rows().get(self.selected).cloned();
        self.projects = projects;
        let rows = self.rows();
        self.selected = selected
            .and_then(|row| rows.iter().position(|r| r == &row))
            .unwrap_or(self.selected.min(rows.len().saturating_sub(1)));
    }
    fn selected_root(&self) -> Option<PathBuf> {
        if let Some(session) = self.session() {
            return Some(session.project_root.clone());
        }
        match self.rows().get(self.selected)? {
            Row::Project(root) if self.organize.state.group == Group::Project => Some(root.clone()),
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
        let previous = self.organize.state.clone();
        let mut changed = false;
        for session in sessions {
            if self
                .organize
                .state
                .read
                .get(&session.id)
                .is_none_or(|seen| *seen < session.last_activity_at)
            {
                self.organize
                    .state
                    .read
                    .insert(session.id, session.last_activity_at);
                changed = true;
            }
        }
        if changed {
            self.save_organization(&previous);
        }
    }
    pub fn organize_key(&mut self, code: KeyCode) -> Option<Action> {
        if let Some((root, mut text)) = self.organize.prompt.take() {
            match code {
                KeyCode::Esc => {
                    self.message.clear();
                    return Some(Action::None);
                }
                KeyCode::Enter => {
                    let previous = self.organize.state.clone();
                    let mut tags: Vec<String> = text
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect();
                    tags.sort();
                    tags.dedup();
                    self.organize.state.tags.insert(root, tags);
                    self.message = "tags saved".into();
                    self.save_organization(&previous);
                    return Some(Action::Rescan);
                }
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(c) if !c.is_control() => text.push(c),
                _ => {}
            }
            self.organize.prompt = Some((root, text));
            return Some(Action::None);
        }
        let previous = self.organize.state.clone();
        match code {
            KeyCode::Char('p') => {
                if let Some(root) = self.selected_root() {
                    if self.organize.state.pins.contains(&root) {
                        self.organize.state.pins.retain(|p| p != &root);
                    } else {
                        self.organize.state.pins.push(root);
                    }
                } else {
                    return Some(Action::None);
                }
            }
            KeyCode::Char('t') => {
                if let Some(root) = self.selected_root() {
                    let text = self
                        .organize
                        .state
                        .tags
                        .get(&root)
                        .map(|t| t.join(", "))
                        .unwrap_or_default();
                    self.organize.prompt = Some((root, text));
                }
                return Some(Action::None);
            }
            KeyCode::Char('m') => {
                self.mark_selected(true);
                return Some(Action::Rescan);
            }
            KeyCode::Char('a') => {
                let entry = self.session().map(|s| s.id.clone()).or_else(|| {
                    self.selected_root()
                        .map(|p| p.to_string_lossy().into_owned())
                });
                if let Some(entry) = entry {
                    if self.organize.state.archived.contains(&entry) {
                        self.organize.state.archived.retain(|a| a != &entry);
                    } else {
                        self.organize.state.archived.push(entry);
                    }
                } else {
                    return Some(Action::None);
                }
            }
            KeyCode::Char('A') => {
                self.organize.state.show_archived = !self.organize.state.show_archived;
            }
            KeyCode::Char('s') => {
                self.organize.state.sort = match self.organize.state.sort {
                    Sort::Activity => Sort::Attention,
                    Sort::Attention => Sort::Activity,
                }
            }
            KeyCode::Char('g') => {
                self.organize.state.group = match self.organize.state.group {
                    Group::Project => Group::Tag,
                    Group::Tag => Group::Project,
                }
            }
            _ => return None,
        }
        self.save_organization(&previous);
        Some(Action::Rescan)
    }
}
