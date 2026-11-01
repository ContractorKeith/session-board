use crate::storage;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub pins: Vec<PathBuf>,
    pub tags: BTreeMap<PathBuf, Vec<String>>,
    /// Project roots or session IDs, as specified by the state file contract.
    pub archived: Vec<String>,
    pub read: BTreeMap<String, i64>,
}

impl State {
    pub fn path() -> io::Result<PathBuf> {
        Ok(storage::directory("SBOARD_STATE_DIR", ".local/state/sboard")?.join("state.toml"))
    }

    /// A missing file is empty state. Anything else that keeps the file from loading
    /// (bad TOML, invalid UTF-8, a directory, no permission) is reported to the caller
    /// and treated as empty; state is never a reason for a command to fail (#11).
    pub fn load(path: &Path) -> io::Result<(Self, Vec<String>)> {
        let text = match storage::read_text(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok((Self::default(), vec![]));
            }
            Err(error) => return Ok((Self::default(), vec![corrupt(path, error)])),
        };
        match toml::from_str(&text) {
            Ok(state) => Ok((state, vec![])),
            Err(error) => Ok((Self::default(), vec![corrupt(path, error)])),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        storage::write_toml(path, self)
    }
}

fn corrupt(path: &Path, error: impl std::fmt::Display) -> String {
    format!("{}: corrupt state ignored: {error}", path.display())
}
