mod ui;
mod watch;

use clap::Parser;
use session_board::{
    cli::{self, Cli, Commands},
    pipeline::{self, Options},
    project::ProjectRoots,
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
    match cli.command {
        None => ui::run(),
        Some(Commands::Open { session_id }) => {
            cli::open(&session_id)?;
            Ok(0)
        }
        Some(Commands::New {
            project_path,
            agent,
            worktree,
        }) => {
            cli::new(&project_path, agent, worktree.as_deref())?;
            Ok(0)
        }
        Some(Commands::Version) => {
            writeln!(io::stdout().lock(), "{}", env!("CARGO_PKG_VERSION"))?;
            Ok(0)
        }
        Some(Commands::Doctor { json }) => {
            cli::doctor(&mut io::stdout().lock(), &cli::home()?, json)?;
            Ok(0)
        }
        Some(Commands::List {
            json,
            project,
            status,
            agent,
            all,
            sort,
            group,
        }) => {
            let outcome = pipeline::run(
                &mut ProjectRoots::default(),
                &Options {
                    project,
                    status,
                    agent,
                    all,
                    sort,
                    group,
                    show_archived: false,
                },
            )?;
            for warning in &outcome.scan.warnings {
                cli::warn(warning);
            }
            let mut out = io::stdout().lock();
            if json && matches!(status, Some(cli::StatusFilter::Attention)) {
                cli::attention_json(&mut out, &outcome.scan.projects, &outcome.state)?;
            } else {
                cli::list_with_state(&mut out, &outcome.scan.projects, json, &outcome.state)?;
            }
            out.flush()?;
            Ok(outcome.exit_code)
        }
    }
}
