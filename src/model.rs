use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    ClaudeCode,
    Codex,
    Grok,
    Opencode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Working,
    Waiting,
    Idle,
    Error,
    Done,
}

/// Times are Unix seconds in UTC; status is derived by a source, never persisted in state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub agent: Agent,
    pub cwd: PathBuf,
    pub project_root: PathBuf,
    pub title: String,
    pub started_at: i64,
    pub last_activity_at: i64,
    pub status: Status,
    pub last_user_text: String,
    pub last_assistant_text: String,
    pub transcript_path: PathBuf,
    pub resume_hint: String,
    pub pane: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub root: PathBuf,
    pub name: String,
    pub sessions: Vec<Session>,
    pub pinned: bool,
    pub tags: Vec<String>,
    pub archived: bool,
    pub unread_count: usize,
}
