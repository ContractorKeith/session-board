//! One-line launch prompts; process execution stays in launch and tmux.
use super::DefaultTerminal;
use super::{Action, App, Row};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use session_board::{config::Config, launch, model::Agent, tmux};
use std::{io, path::PathBuf};

enum Prompt {
    Agent {
        project: PathBuf,
        worktree: bool,
    },
    Branch {
        project: PathBuf,
        agent: Agent,
        branch: String,
    },
}
#[derive(Default)]
pub struct Open {
    prompt: Option<Prompt>,
}
impl Open {
    pub fn text(&self) -> Option<String> {
        match self.prompt.as_ref()? {
            Prompt::Agent { .. } => Some("Agent: c claude_code · x codex · Esc cancel".into()),
            Prompt::Branch { branch, .. } => {
                Some(format!("Branch: {branch} · Enter launch · Esc cancel"))
            }
        }
    }
    pub fn active(&self) -> bool {
        self.prompt.is_some()
    }
    pub fn accepts_text(&self) -> bool {
        matches!(self.prompt, Some(Prompt::Branch { .. }))
    }
    pub fn begin(&mut self, project: PathBuf, worktree: bool) {
        self.prompt = Some(Prompt::Agent { project, worktree });
    }
    pub fn paste(&mut self, text: &str) {
        if let Some(Prompt::Branch { branch, .. }) = &mut self.prompt {
            for c in text.chars().filter(|c| !c.is_control()) {
                if branch.len() + c.len_utf8() > 256 {
                    break;
                }
                branch.push(c);
            }
        }
    }
    /// Prompts produce launch intent; action() retains M3's process/terminal handling.
    pub fn key(&mut self, app: &mut App, key: KeyEvent) -> Option<Action> {
        let prompt = self.prompt.take()?;
        if (key.code == KeyCode::Esc
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT))
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            app.message = "launch cancelled".into();
            return Some(Action::None);
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            self.prompt = Some(prompt);
            return Some(Action::None);
        }
        self.prompt = match prompt {
            Prompt::Agent { project, worktree } => {
                let agent = match key.code {
                    KeyCode::Char('c') => Some(Agent::ClaudeCode),
                    KeyCode::Char('x') => Some(Agent::Codex),
                    _ => None,
                };
                match agent {
                    Some(agent) if worktree => Some(Prompt::Branch {
                        project,
                        agent,
                        branch: String::new(),
                    }),
                    Some(agent) => {
                        return Some(Action::Launch {
                            project,
                            agent,
                            branch: None,
                        });
                    }
                    None => Some(Prompt::Agent { project, worktree }),
                }
            }
            Prompt::Branch {
                project,
                agent,
                mut branch,
            } => {
                match key.code {
                    KeyCode::Enter => {
                        return Some(Action::Launch {
                            project,
                            agent,
                            branch: Some(branch),
                        });
                    }
                    KeyCode::Backspace => {
                        branch.pop();
                    }
                    KeyCode::Char(c) if !c.is_control() && branch.len() + c.len_utf8() <= 256 => {
                        branch.push(c)
                    }
                    _ => {}
                }
                Some(Prompt::Branch {
                    project,
                    agent,
                    branch,
                })
            }
        };
        Some(Action::None)
    }
    pub fn action(
        &mut self,
        app: &mut App,
        action: Action,
        terminal: &mut DefaultTerminal,
    ) -> io::Result<()> {
        match action {
            Action::Launch {
                project,
                agent,
                branch,
            } => {
                self.launch(app, terminal, project, agent, branch.as_deref())?;
            }
            Action::Open => {
                let Some(mut session) = app.session().cloned() else {
                    app.message = "select a session to open".into();
                    return Ok(());
                };
                app.mark_selected(false);
                app.flush_read_marks(true);
                let save_notice = app.message.clone();
                let result = (|| {
                    let (config, _) = Config::load(&Config::path()?)?;
                    app.scan_warnings.extend(tmux::refresh(
                        &mut app.organize.projects,
                        &config.tmux.socket,
                    ));
                    app.project_organization();
                    // Read marks can shrink the queue; keep the original open target.
                    let known_pane = session.pane.clone();
                    session = app
                        .organize
                        .projects
                        .iter()
                        .flat_map(|p| &p.sessions)
                        .find(|s| {
                            s.id == session.id && s.transcript_path == session.transcript_path
                        })
                        .cloned()
                        .unwrap_or(session);
                    session.pane = session.pane.or(known_pane);
                    if session.pane.is_none() {
                        // Validate before leaving the TUI.
                        launch::resume_command(&session)?;
                        let notice = format!("no pane; resuming in {}", session.cwd.display());
                        app.message = if save_notice.is_empty() {
                            notice
                        } else {
                            format!("{save_notice}; {notice}")
                        };
                        terminal.draw(|f| app.render(f, 0))?;
                    }
                    // Jump failures remain errors even when the target vanished after discovery.
                    app.suspend(terminal, || launch::open(&session, &config.tmux.socket))
                })();
                let outcome = result.unwrap_or_else(|e| format!("error: {e}"));
                app.message = if save_notice.is_empty() {
                    outcome
                } else {
                    format!("{save_notice}; {outcome}")
                };
            }
            Action::New | Action::Worktree => {
                let root = app.session().map(|s| s.project_root.clone()).or_else(|| {
                    match app.rows().get(app.selected) {
                        Some(Row::Project(root))
                            if app.state.group == session_board::scan::Group::Project =>
                        {
                            Some(root.clone())
                        }
                        Some(Row::Session(_, _)) => app.session().map(|s| s.project_root.clone()),
                        _ => None,
                    }
                });
                if let Some(project) = root {
                    self.begin(project, action == Action::Worktree);
                } else {
                    app.message = "select a project to launch".into();
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn launch(
        &self,
        app: &mut App,
        terminal: &mut DefaultTerminal,
        project: PathBuf,
        agent: Agent,
        branch: Option<&str>,
    ) -> io::Result<()> {
        let result = (|| {
            let (config, _) = Config::load(&Config::path()?)?;
            let cwd = match branch {
                Some(branch) => launch::worktree(&project, branch)?,
                None => project,
            };
            launch::directory(&cwd)?;
            if tmux::inside() {
                launch::launch(&cwd, agent, &config.tmux.socket)
            } else {
                app.suspend(terminal, || {
                    launch::launch(&cwd, agent, &config.tmux.socket)
                })
            }
        })();
        app.message = result.unwrap_or_else(|e| e.to_string());
        Ok(())
    }
}
