use clap::Parser;
use session_board::{
    cli::{self, Cli, Commands},
    config::Config,
    project::ProjectRoots,
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
    if let Err(error) = run(cli) {
        // A reader that went away (`sboard list | head`) is not an error worth a word (#10).
        if error.kind() == io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        eprintln!("sboard: {error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> io::Result<()> {
    let mut out = io::stdout().lock();
    match cli.command {
        None => writeln!(out, "TUI lands in M2")?,
        Some(Commands::Version) => writeln!(out, "{}", env!("CARGO_PKG_VERSION"))?,
        Some(Commands::Doctor { json }) => cli::doctor(&mut out, &cli::home()?, json)?,
        Some(Commands::List { json }) => {
            let (config, warnings) = Config::load(&Config::path()?)?;
            let (state, state_warnings) = State::load(&State::path()?)?;
            let mut roots = ProjectRoots::default();
            let result = scan::scan(&source::registry(), &config, &state, |cwd| {
                roots.resolve(cwd)
            });
            for warning in warnings
                .into_iter()
                .chain(state_warnings)
                .chain(result.warnings)
            {
                eprintln!("sboard: {warning}");
            }
            if json {
                writeln!(out, "{}", serde_json::to_string_pretty(&result.projects)?)?;
            } else {
                for project in result.projects {
                    writeln!(
                        out,
                        "{} ({} sessions)",
                        project.root.display(),
                        project.sessions.len()
                    )?;
                }
            }
        }
    }
    out.flush()
}
