use clap::Parser;
use session_board::{
    cli::{self, Cli, Commands},
    config::Config,
    project::ProjectRoots,
    scan, source,
    state::State,
};
use std::{env, io, path::PathBuf};

fn main() {
    let cli = Cli::try_parse().unwrap_or_else(|error| {
        let code = if error.use_stderr() { 1 } else { 0 };
        let _ = error.print();
        std::process::exit(code);
    });
    if let Err(error) = run(cli) {
        eprintln!("sboard: {error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> io::Result<()> {
    match cli.command {
        None => println!("TUI lands in M2"),
        Some(Commands::Version) => println!("{}", env!("CARGO_PKG_VERSION")),
        Some(Commands::Doctor { json }) => {
            let home = env::var_os("HOME")
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
            cli::doctor(&PathBuf::from(home), json)?;
        }
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
                println!("{}", serde_json::to_string_pretty(&result.projects)?);
            } else {
                for project in result.projects {
                    println!(
                        "{} ({} sessions)",
                        project.root.display(),
                        project.sessions.len()
                    );
                }
            }
        }
    }
    Ok(())
}
