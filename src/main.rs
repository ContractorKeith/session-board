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
        let _ = error.print();
        std::process::exit(code);
    });
    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => std::process::exit(0),
        Err(error) => {
            eprintln!("sboard: {error}");
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
            let (mut config, warnings) = Config::load(&Config::path()?)?;
            let (mut state, state_warnings) = State::load(&State::path()?)?;
            let home = cli::home()?;
            normalize_settings(&mut config, &mut state, &home);
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
                eprintln!("sboard: {warning}");
            }
            cli::list(&mut out, &result.projects, json)?;
        }
    }
    out.flush()?;
    Ok(exit)
}
