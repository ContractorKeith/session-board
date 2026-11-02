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

pub struct Options {
    pub project: Option<PathBuf>,
    pub status: Option<StatusFilter>,
    pub agent: Option<AgentFilter>,
    pub all: bool,
    pub sort: Option<scan::Sort>,
    pub group: Option<scan::Group>,
    /// The background TUI scan retains archives for its local view toggle.
    pub show_archived: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            project: None,
            status: None,
            agent: None,
            all: false,
            sort: Some(scan::Sort::Activity),
            group: Some(scan::Group::Project),
            show_archived: true,
        }
    }
}
pub struct Outcome {
    pub scan: Scan,
    pub state: State,
    pub exit_code: i32,
}

pub fn run(roots: &mut ProjectRoots, options: &Options) -> io::Result<Outcome> {
    let home = cli::home()?;
    let (mut config, mut warnings) = Config::load(&Config::path()?)?;
    let (mut state, state_warnings) = State::load(&State::path()?)?;
    warnings.extend(state_warnings);
    warnings.extend(State::cleanup(&State::path()?));
    warnings.extend(normalize_settings(&mut config, &mut state, &home));
    let selected = options.agent.map(|agent| match agent {
        AgentFilter::ClaudeCode => "claude_code",
        AgentFilter::Codex => "codex",
        AgentFilter::Grok => "grok",
        AgentFilter::Opencode => "opencode",
    });
    let disabled = [
        (!config.agents.claude_code, "claude_code"),
        (!config.agents.codex, "codex"),
        (!config.agents.grok, "grok"),
        (!config.agents.opencode, "opencode"),
    ]
    .into_iter()
    .filter_map(|(off, name)| (off && selected.is_none_or(|s| s == name)).then_some(name))
    .collect::<Vec<_>>();
    if options.all {
        config.window_days = u32::MAX;
    }
    if let Some(agent) = options.agent {
        config.agents.claude_code &= agent.agent() == crate::model::Agent::ClaudeCode;
        config.agents.codex &= agent.agent() == crate::model::Agent::Codex;
        config.agents.grok &= agent.agent() == crate::model::Agent::Grok;
        config.agents.opencode &= agent.agent() == crate::model::Agent::Opencode;
    }
    let stores = cli::stores(&home);
    let missing = !(config.agents.claude_code && stores[0].exists
        || config.agents.codex && stores[1].exists
        || config.agents.grok && stores[2].exists
        || config.agents.opencode && stores[3].exists);
    if missing {
        let looked_for = stores
            .iter()
            .filter(|s| selected.is_none_or(|name| name == s.agent))
            .map(|s| format!("{} at {}", s.agent, s.path.display()))
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!(
            "no enabled stores found; looked for {looked_for}; disabled by config: {}",
            if disabled.is_empty() {
                "none".into()
            } else {
                disabled.join(", ")
            }
        ));
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
    warnings.extend(crate::tmux::refresh(
        &mut result.projects,
        &config.tmux.socket,
    ));
    result.missing_projects = result
        .projects
        .iter()
        .filter(|p| !p.root.is_dir())
        .map(|p| p.root.clone())
        .collect();
    cli::filter(
        &mut result.projects,
        &state,
        project.as_deref(),
        options.status,
        options.agent,
        options.all || options.show_archived,
    );
    scan::organize(
        &mut result.projects,
        &state,
        if matches!(options.status, Some(StatusFilter::Attention)) {
            scan::Sort::Attention
        } else {
            options.sort.unwrap_or_default()
        },
        options.group.unwrap_or_default(),
    );
    if !missing
        && !options.all
        && options.status.is_none()
        && result.warnings.is_empty()
        && result
            .projects
            .iter()
            .all(|project| project.sessions.is_empty())
    {
        warnings.push(format!(
            "no sessions in the last {} days; use --all",
            config.window_days
        ));
    }
    warnings.append(&mut result.warnings);
    warnings.extend(roots.take_warnings());
    result.warnings = warnings;
    Ok(Outcome {
        scan: result,
        state,
        exit_code: if missing { 2 } else { 0 },
    })
}
