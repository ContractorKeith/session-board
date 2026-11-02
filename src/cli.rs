use crate::storage;
use clap::{Parser, Subcommand, ValueEnum};
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
        #[arg(long)]
        project: Option<PathBuf>,
        #[arg(long, value_enum)]
        status: Option<StatusFilter>,
        #[arg(long, value_enum)]
        agent: Option<AgentFilter>,
        #[arg(long)]
        all: bool,
    },
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Jump to a live pane or resume a recorded session.
    Open {
        session_id: String,
    },
    /// Launch a new session in a project or fresh worktree.
    New {
        project_path: PathBuf,
        #[arg(long, value_enum, default_value = "claude_code")]
        agent: LaunchAgent,
        #[arg(long)]
        worktree: Option<String>,
    },
    Version,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum StatusFilter {
    Working,
    Waiting,
    Idle,
    Error,
    Done,
    Attention,
}
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum AgentFilter {
    #[value(name = "claude_code")]
    ClaudeCode,
    Codex,
    Grok,
    Opencode,
}
impl AgentFilter {
    pub fn agent(self) -> crate::model::Agent {
        use crate::model::Agent;
        match self {
            Self::ClaudeCode => Agent::ClaudeCode,
            Self::Codex => Agent::Codex,
            Self::Grok => Agent::Grok,
            Self::Opencode => Agent::Opencode,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Store {
    pub agent: &'static str,
    pub path: PathBuf,
    pub exists: bool,
    pub reason: String,
    pub sessions: usize,
    pub skipped_lines: usize,
    pub duplicate_ids: usize,
    pub skipped_stores: usize,
    pub skipped_sessions: usize,
    pub warnings: Vec<String>,
}

pub fn stores(home: &Path) -> Vec<Store> {
    let (claude, codex) = crate::source::homes(home);
    [
        ("claude_code", claude.join("projects"), true),
        ("codex", codex.join("sessions"), true),
        (
            "grok",
            home.join(".grok/sessions/session_search.sqlite"),
            false,
        ),
        (
            "opencode",
            home.join(".local/share/opencode/opencode.db"),
            false,
        ),
    ]
    .into_iter()
    .map(|(agent, path, directory)| {
        let exists = if directory {
            path.is_dir()
        } else {
            path.is_file()
        };
        let reason = if exists {
            "found"
        } else if directory && path.exists() {
            "not a directory"
        } else {
            "missing"
        }
        .into();
        Store {
            agent,
            path,
            exists,
            reason,
            sessions: 0,
            skipped_lines: 0,
            duplicate_ids: 0,
            skipped_stores: usize::from(!exists),
            skipped_sessions: 0,
            warnings: vec![],
        }
    })
    .collect()
}

/// The home directory doctor inspects; empty `HOME` counts as unset.
pub fn home() -> io::Result<PathBuf> {
    storage::home()
}

pub fn doctor(out: &mut impl Write, home: &Path, json: bool) -> io::Result<()> {
    let mut stores = stores(home);
    let config = crate::config::Config {
        window_days: u32::MAX,
        ..Default::default()
    };
    for (store, source) in stores.iter_mut().zip(crate::source::registry_at(home)) {
        let result = source.scan(&config);
        store.sessions = result.sessions.len();
        let mut ids = std::collections::HashSet::new();
        store.duplicate_ids = result
            .sessions
            .iter()
            .filter(|s| !ids.insert(&s.id))
            .count();
        store.skipped_lines = result.skipped_lines;
        store.skipped_stores = result.skipped_stores;
        store.skipped_sessions = result.skipped_sessions;
        store.warnings = result.warnings;
        store.warnings.extend(result.status_reasons);
    }
    let (config, _) = crate::config::Config::load(&crate::config::Config::path()?)?;
    let tmux = crate::tmux::doctor(&config.tmux.socket);
    if json {
        writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "stores": stores, "tmux": tmux }))?
        )?;
    } else {
        writeln!(out, "tmux: {tmux}")?;
        for store in stores {
            writeln!(
                out,
                "{}: {} ({}); sessions: {}, skipped lines: {}, skipped stores: {}, skipped sessions: {}, duplicate ids: {}",
                store.agent,
                store.path.display(),
                store.reason,
                store.sessions,
                store.skipped_lines,
                store.skipped_stores,
                store.skipped_sessions,
                store.duplicate_ids
            )?;
            for warning in store.warnings {
                writeln!(out, "  {warning}")?;
            }
        }
    }
    Ok(())
}

/// Filter and sort the project tree without reading stores or state.
pub fn filter(
    projects: &mut Vec<crate::model::Project>,
    state: &crate::state::State,
    project: Option<&Path>,
    status: Option<StatusFilter>,
    agent: Option<AgentFilter>,
    all: bool,
) {
    use crate::model::{Session, Status};
    let unread = |s: &Session| {
        state
            .read
            .get(&s.id)
            .is_none_or(|t| s.last_activity_at > *t)
    };
    let rank = |s: &Session| match s.status {
        Status::Waiting => 0,
        Status::Error => 1,
        Status::Done => 2,
        _ => 3,
    };
    let attention = matches!(status, Some(StatusFilter::Attention));
    projects.retain_mut(|p| {
        if project.is_some_and(|root| root != p.root) || (!all && p.archived) {
            return false;
        }
        let had_sessions = !p.sessions.is_empty();
        p.sessions.retain(|s| {
            if !all && state.archived.contains(&s.id) {
                return false;
            }
            if agent.is_some_and(|a| a.agent() != s.agent) {
                return false;
            }
            match status {
                None => true,
                Some(StatusFilter::Attention) => {
                    rank(s) < 2 || (s.status == Status::Done && unread(s))
                }
                Some(f) => {
                    s.status
                        == match f {
                            StatusFilter::Working => Status::Working,
                            StatusFilter::Waiting => Status::Waiting,
                            StatusFilter::Idle => Status::Idle,
                            StatusFilter::Error => Status::Error,
                            _ => Status::Done,
                        }
                }
            }
        });
        p.unread_count = p.sessions.iter().filter(|s| unread(s)).count();
        if attention {
            p.sessions.sort_by(|a, b| {
                rank(a)
                    .cmp(&rank(b))
                    .then_with(|| b.last_activity_at.cmp(&a.last_activity_at))
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
        !p.sessions.is_empty() || (!had_sessions && status.is_none() && agent.is_none())
    });
    projects.sort_by(|a, b| {
        let activity =
            |p: &crate::model::Project| p.sessions.first().map_or(i64::MIN, |s| s.last_activity_at);
        let priority = |p: &crate::model::Project| p.sessions.first().map_or(4, rank);
        (if attention {
            priority(a).cmp(&priority(b))
        } else {
            b.pinned.cmp(&a.pinned)
        })
        .then_with(|| activity(b).cmp(&activity(a)))
        .then_with(|| a.root.cmp(&b.root))
    });
}

/// Shared renderer for the cold scan and its fixed-clock fixture golden.
pub fn list(
    out: &mut impl Write,
    projects: &[crate::model::Project],
    json: bool,
) -> io::Result<()> {
    if json {
        writeln!(out, "{}", serde_json::to_string_pretty(projects)?)?;
    } else {
        for project in projects {
            writeln!(
                out,
                "{} ({} sessions)",
                project.root.display(),
                project.sessions.len()
            )?;
            for session in &project.sessions {
                writeln!(
                    out,
                    "  {} {} {}  {}",
                    serde_json::to_value(session.status)?
                        .as_str()
                        .unwrap_or_default(),
                    serde_json::to_value(session.agent)?
                        .as_str()
                        .unwrap_or_default(),
                    session.id,
                    session.title
                )?;
            }
        }
    }
    Ok(())
}

/// Diagnostics must not prevent successful stdout when stderr is closed or full.
pub fn warn(message: &str) {
    let _ = writeln!(io::stderr().lock(), "sboard: {message}");
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LaunchAgent {
    #[value(name = "claude_code")]
    ClaudeCode,
    Codex,
}
impl LaunchAgent {
    pub fn agent(self) -> crate::model::Agent {
        match self {
            Self::ClaudeCode => crate::model::Agent::ClaudeCode,
            Self::Codex => crate::model::Agent::Codex,
        }
    }
}
pub fn open(id: &str) -> io::Result<()> {
    let result = crate::pipeline::run(
        &mut crate::project::ProjectRoots::default(),
        &crate::pipeline::Options {
            all: true,
            ..Default::default()
        },
    )?;
    let session = result
        .scan
        .projects
        .iter()
        .flat_map(|p| &p.sessions)
        .find(|s| s.id == id)
        .ok_or_else(|| io::Error::other(format!("session not found: {id}")))?;
    let (config, _) = crate::config::Config::load(&crate::config::Config::path()?)?;
    crate::launch::open(session, &config.tmux.socket)?;
    Ok(())
}
pub fn new(project: &Path, agent: LaunchAgent, branch: Option<&str>) -> io::Result<()> {
    crate::launch::directory(project)?;
    let (config, _) = crate::config::Config::load(&crate::config::Config::path()?)?;
    let cwd = if let Some(branch) = branch {
        crate::launch::worktree(project, branch)?
    } else {
        project.canonicalize()?
    };
    crate::launch::launch(&cwd, agent.agent(), &config.tmux.socket)?;
    Ok(())
}
