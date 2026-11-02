pub mod claude;
pub mod codex;
mod read;
use crate::{
    config::Config,
    model::{Agent, Session},
};
pub use read::title;

/// Sources report malformed lines and missing stores without failing a scan.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SourceScan {
    pub sessions: Vec<Session>,
    pub skipped_lines: usize,
    pub skipped_stores: usize,
    pub skipped_sessions: usize,
    pub warnings: Vec<String>,
    pub status_reasons: Vec<String>,
}

pub trait Source {
    fn agent(&self) -> Agent;
    fn scan(&self, config: &Config) -> SourceScan;
}

pub type Registry = Vec<Box<dyn Source>>;

/// Environment overrides point at the agent config homes, not their stores.
pub fn registry() -> Registry {
    let home = crate::storage::home().unwrap_or_default();
    registry_at(&home)
}

pub fn homes(home: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = |key, fallback| {
        std::env::var_os(key)
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| home.join(fallback))
    };
    (
        root("CLAUDE_CONFIG_DIR", ".claude"),
        root("CODEX_HOME", ".codex"),
    )
}

pub fn registry_at(home: &std::path::Path) -> Registry {
    let (claude, codex) = homes(home);
    let now = read::now();
    vec![
        Box::new(claude::Claude {
            root: claude.join("projects"),
            now,
        }),
        Box::new(codex::Codex {
            root: codex.join("sessions"),
            index: codex.join("session_index.jsonl"),
            now,
        }),
    ]
}
