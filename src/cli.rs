use crate::storage;
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Parser)]
#[command(
    name = "sboard",
    version,
    about = "Observe agent sessions grouped by project"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    List {
        #[arg(long)]
        json: bool,
    },
    Doctor {
        #[arg(long)]
        json: bool,
    },
    Version,
}

#[derive(Debug, Serialize)]
pub struct Store {
    pub agent: &'static str,
    pub path: PathBuf,
    pub exists: bool,
}

/// Only inspect store existence. No agent store is opened or scanned in M0.
pub fn stores(home: &Path) -> Vec<Store> {
    [
        ("claude_code", ".claude/projects"),
        ("codex", ".codex/sessions"),
        ("grok", ".grok/sessions/session_search.sqlite"),
        ("opencode", ".local/share/opencode/opencode.db"),
    ]
    .into_iter()
    .map(|(agent, suffix)| {
        let path = home.join(suffix);
        let exists = path.exists();
        Store {
            agent,
            path,
            exists,
        }
    })
    .collect()
}

/// The home directory doctor inspects; empty `HOME` counts as unset.
pub fn home() -> io::Result<PathBuf> {
    storage::home()
}

pub fn doctor(out: &mut impl Write, home: &Path, json: bool) -> io::Result<()> {
    let stores = stores(home);
    if json {
        writeln!(out, "{}", serde_json::to_string_pretty(&stores)?)?;
    } else {
        for store in stores {
            writeln!(
                out,
                "{}: {} ({})",
                store.agent,
                store.path.display(),
                if store.exists { "found" } else { "missing" }
            )?;
        }
        writeln!(out, "Source scanning lands in M1; no agent stores read.")?;
    }
    Ok(())
}
