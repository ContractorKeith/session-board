use clap::Parser;
use session_board::{
    cli::{self, Cli, Commands},
    config::Config,
    project::{ProjectRoots, normalize, normalize_settings},
    scan, source,
    state::State,
};
use std::io::{self, Write};

fn main() {
    let cli = Cli::try_parse().unwrap_or_else(|error| {
        let code = if error.use_stderr() { 1 } else { 0 };
        if error.use_stderr() {
            cli::warn(&error.to_string());
        } else {
            let _ = error.print();
        }
        std::process::exit(code);
    });
    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => std::process::exit(0),
        Err(error) => {
            cli::warn(&error.to_string());
            std::process::exit(1);
        }
    }
}

fn run(cli: Cli) -> io::Result<i32> {
    let mut exit = 0;
    let mut out = io::stdout().lock();
    match cli.command {
        None => writeln!(out, "TUI lands in M2")?,
        Some(Commands::Version) => writeln!(out, "{}", env!("CARGO_PKG_VERSION"))?,
        Some(Commands::Doctor { json }) => cli::doctor(&mut out, &cli::home()?, json)?,
        Some(Commands::List {
            json,
            project,
            status,
            agent,
            all,
        }) => {
            if matches!(
                agent,
                Some(cli::AgentFilter::Grok | cli::AgentFilter::Opencode)
            ) {
                return Err(io::Error::other(
                    "Grok and OpenCode sources are not implemented until M6; use claude_code or codex",
                ));
            }
            let (mut config, mut warnings) = Config::load(&Config::path()?)?;
            let (mut state, state_warnings) = State::load(&State::path()?)?;
            let home = cli::home()?;
            warnings.extend(normalize_settings(&mut config, &mut state, &home));
            let disabled = [
                (!config.agents.claude_code, "claude_code"),
                (!config.agents.codex, "codex"),
            ]
            .into_iter()
            .filter_map(|(off, name)| off.then_some(name))
            .collect::<Vec<_>>();
            if all {
                config.window_days = u32::MAX;
            }
            if let Some(agent) = agent {
                config.agents.claude_code &=
                    agent.agent() == session_board::model::Agent::ClaudeCode;
                config.agents.codex &= agent.agent() == session_board::model::Agent::Codex;
            }
            let stores = cli::stores(&home);
            if !(config.agents.claude_code && stores[0].exists
                || config.agents.codex && stores[1].exists)
            {
                exit = 2;
                cli::warn(&format!(
                    "no enabled stores found; looked for claude_code at {} and codex at {}; disabled by config: {}",
                    stores[0].path.display(),
                    stores[1].path.display(),
                    if disabled.is_empty() {
                        "none".into()
                    } else {
                        disabled.join(", ")
                    }
                ));
            }
            let project = project
                .map(|p| {
                    let absolute = if p.is_relative() && !p.starts_with("~") {
                        std::env::current_dir()?.join(p)
                    } else {
                        p
                    };
                    normalize(&absolute, Some(&home))
                        .ok_or_else(|| io::Error::other("invalid project path"))
                })
                .transpose()?;
            let mut roots = ProjectRoots::default();
            let project = project.map(|p| roots.resolve(&p));
            let mut result = scan::scan(&source::registry(), &config, &state, |cwd| {
                roots.resolve(cwd)
            });
            cli::filter(
                &mut result.projects,
                &state,
                project.as_deref(),
                status,
                agent,
                all,
            );
            for warning in warnings
                .into_iter()
                .chain(state_warnings)
                .chain(result.warnings)
            {
                cli::warn(&warning);
            }
            cli::list(&mut out, &result.projects, json)?;
        }
    }
    out.flush()?;
    Ok(exit)
}
