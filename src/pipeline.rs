//! The common cold-scan pipeline for list and the TUI.
use crate::{
    cli::{self, AgentFilter, StatusFilter},
    config::Config,
    project::{ProjectRoots, normalize, normalize_settings},
    scan::{self, Scan},
    source,
    state::State,
};
use std::{io, path::PathBuf};

#[derive(Default)]
pub struct Options {
    pub project: Option<PathBuf>,
    pub status: Option<StatusFilter>,
    pub agent: Option<AgentFilter>,
    pub all: bool,
}
pub struct Outcome {
    pub scan: Scan,
    pub exit_code: i32,
}

pub fn run(roots: &mut ProjectRoots, options: &Options) -> io::Result<Outcome> {
    if matches!(
        options.agent,
        Some(AgentFilter::Grok | AgentFilter::Opencode)
    ) {
        return Err(io::Error::other(
            "Grok and OpenCode sources are not implemented until M6; use claude_code or codex",
        ));
    }
    let home = cli::home()?;
    let (mut config, mut warnings) = Config::load(&Config::path()?)?;
    let (mut state, state_warnings) = State::load(&State::path()?)?;
    warnings.extend(state_warnings);
    warnings.extend(normalize_settings(&mut config, &mut state, &home));
    let disabled = [
        (!config.agents.claude_code, "claude_code"),
        (!config.agents.codex, "codex"),
    ]
    .into_iter()
    .filter_map(|(off, name)| off.then_some(name))
    .collect::<Vec<_>>();
    if options.all {
        config.window_days = u32::MAX;
    }
    if let Some(agent) = options.agent {
        config.agents.claude_code &= agent.agent() == crate::model::Agent::ClaudeCode;
        config.agents.codex &= agent.agent() == crate::model::Agent::Codex;
    }
    let stores = cli::stores(&home);
    let missing =
        !(config.agents.claude_code && stores[0].exists || config.agents.codex && stores[1].exists);
    if missing {
        warnings.push(format!("no enabled stores found; looked for claude_code at {} and codex at {}; disabled by config: {}", stores[0].path.display(), stores[1].path.display(), if disabled.is_empty() { "none".into() } else { disabled.join(", ") }));
    }
    let project = options
        .project
        .as_ref()
        .map(|p| {
            let absolute = if p.is_relative() && !p.starts_with("~") {
                std::env::current_dir()?.join(p)
            } else {
                p.clone()
            };
            normalize(&absolute, Some(&home))
                .ok_or_else(|| io::Error::other("invalid project path"))
        })
        .transpose()?
        .map(|p| roots.resolve(&p));
    let mut result = scan::scan(&source::registry(), &config, &state, |cwd| {
        roots.resolve(cwd)
    });
    let panes = crate::tmux::discover(&config.tmux.socket);
    for session in result.projects.iter_mut().flat_map(|p| &mut p.sessions) {
        session.pane = crate::tmux::matching(session, &panes).map(crate::tmux::Pane::target);
    }
    cli::filter(
        &mut result.projects,
        &state,
        project.as_deref(),
        options.status,
        options.agent,
        options.all,
    );
    warnings.append(&mut result.warnings);
    warnings.extend(roots.take_warnings());
    result.warnings = warnings;
    Ok(Outcome {
        scan: result,
        exit_code: if missing { 2 } else { 0 },
    })
}
