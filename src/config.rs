use crate::{model::Agent, storage};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub roots: Vec<PathBuf>,
    pub window_days: u32,
    pub agents: Agents,
    pub notify: bool,
    pub tmux: Tmux,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            roots: vec![],
            window_days: 7,
            agents: Agents::default(),
            notify: false,
            tmux: Tmux::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Agents {
    pub claude_code: bool,
    pub codex: bool,
    pub grok: bool,
    pub opencode: bool,
}

impl Default for Agents {
    fn default() -> Self {
        Self {
            claude_code: true,
            codex: true,
            grok: true,
            opencode: true,
        }
    }
}

impl Agents {
    pub fn enabled(&self, agent: Agent) -> bool {
        match agent {
            Agent::ClaudeCode => self.claude_code,
            Agent::Codex => self.codex,
            Agent::Grok => self.grok,
            Agent::Opencode => self.opencode,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tmux {
    pub socket: String,
}

impl Config {
    pub fn path() -> io::Result<PathBuf> {
        Ok(storage::directory("SBOARD_CONFIG_DIR", ".config/sboard")?.join("config.toml"))
    }

    pub fn load(path: &Path) -> io::Result<(Self, Vec<String>)> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok((Self::default(), vec![]));
            }
            Err(error) => return Err(error),
        };
        let value: toml::Value = toml::from_str(&text).map_err(io::Error::other)?;
        let schema = toml::Value::try_from(Self::default()).map_err(io::Error::other)?;
        let mut warnings = vec![];
        unknown_keys(&value, &schema, "", &mut warnings);
        let config = value.try_into().map_err(io::Error::other)?;
        Ok((config, warnings))
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        storage::write_toml(path, self)
    }
}

fn unknown_keys(
    value: &toml::Value,
    schema: &toml::Value,
    prefix: &str,
    warnings: &mut Vec<String>,
) {
    if let (Some(table), Some(known)) = (value.as_table(), schema.as_table()) {
        for (key, value) in table {
            let name = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            if let Some(expected) = known.get(key) {
                unknown_keys(value, expected, &name, warnings);
            } else {
                warnings.push(format!("unknown config key: {name}"));
            }
        }
    }
}
