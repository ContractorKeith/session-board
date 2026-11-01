use crate::{
    config::Config,
    model::{Agent, Session},
};

/// Sources report malformed lines and missing stores without failing a scan.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SourceScan {
    pub sessions: Vec<Session>,
    pub skipped_lines: usize,
    pub skipped_stores: usize,
    pub warnings: Vec<String>,
}

pub trait Source {
    fn agent(&self) -> Agent;
    fn scan(&self, config: &Config) -> SourceScan;
}

pub type Registry = Vec<Box<dyn Source>>;

/// M1 registers the first real sources here.
pub fn registry() -> Registry {
    vec![]
}
