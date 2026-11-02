//! One-line launch prompts; process execution stays in launch and tmux.
use super::{Action, App, Row};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;
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
pub(super) struct Open {
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
    pub fn key(
        &mut self,
        app: &mut App,
        key: KeyEvent,
        terminal: &mut DefaultTerminal,
    ) -> io::Result<bool> {
        let Some(prompt) = self.prompt.take() else {
            return Ok(false);
        };
        if key.code == KeyCode::Esc
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            app.message = "launch cancelled".into();
            return Ok(true);
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
                        self.launch(app, terminal, project, agent, None)?;
                        None
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
                        self.launch(app, terminal, project, agent, Some(&branch))?;
                        return Ok(true);
                    }
                    KeyCode::Backspace => {
                        branch.pop();
                    }
                    KeyCode::Char(c) if !c.is_control() && branch.len() < 256 => branch.push(c),
                    _ => {}
                }
                Some(Prompt::Branch {
                    project,
                    agent,
                    branch,
                })
            }
        };
        Ok(true)
    }
    pub fn action(
        &mut self,
        app: &mut App,
        action: Action,
        terminal: &mut DefaultTerminal,
    ) -> io::Result<()> {
        match action {
            Action::Open => {
                let Some(mut session) = app.session().cloned() else {
                    app.message = "select a session to open".into();
                    return Ok(());
                };
                let result = (|| {
                    let (config, _) = Config::load(&Config::path()?)?;
                    app.scan_warnings
                        .extend(tmux::refresh(&mut app.projects, &config.tmux.socket));
                    session = app.session().cloned().unwrap_or(session);
                    if session.pane.is_none() {
                        // Validate before leaving the TUI.
                        launch::resume_command(&session)?;
                        app.message = format!("no pane; resuming in {}", session.cwd.display());
                        terminal.draw(|f| app.render(f, 0))?;
                    }
                    // A pane can vanish after discovery; fallback resume needs suspension too.
                    app.suspend(terminal, || launch::open(&session, &config.tmux.socket))
                })();
                app.message = result.unwrap_or_else(|e| e.to_string());
            }
            Action::New | Action::Worktree => {
                let root = match app.rows().get(app.selected) {
                    Some(Row::Project(root)) => Some(root.clone()),
                    Some(Row::Session(_)) => app.session().map(|s| s.project_root.clone()),
                    _ => None,
                };
                if let Some(project) = root {
                    self.prompt = Some(Prompt::Agent {
                        project,
                        worktree: action == Action::Worktree,
                    });
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
