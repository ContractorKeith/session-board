use crate::{config::Config, model::Project, source::Registry, state::State};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Default)]
pub struct Scan {
    pub projects: Vec<Project>,
    pub missing_projects: Vec<PathBuf>,
    pub skipped_lines: usize,
    pub skipped_stores: usize,
    pub skipped_sessions: usize,
    pub warnings: Vec<String>,
}

/// Root discovery is supplied by the caller; grouping itself does no I/O.
pub fn scan(
    registry: &Registry,
    config: &Config,
    state: &State,
    mut resolve: impl FnMut(&Path) -> PathBuf,
) -> Scan {
    let mut result = Scan::default();
    let mut projects = BTreeMap::new();
    for root in config
        .roots
        .iter()
        .chain(&state.pins)
        .chain(state.tags.keys())
    {
        if root.to_str().is_none() {
            result.warnings.push(format!(
                "non-UTF-8 project path skipped: {}",
                root.display()
            ));
            continue;
        }
        projects
            .entry(root.clone())
            .or_insert_with(|| project(root, state));
    }
    for source in registry {
        if !config.agents.enabled(source.agent()) {
            continue;
        }
        let scanned = source.scan(config);
        result.skipped_lines += scanned.skipped_lines;
        result.skipped_stores += scanned.skipped_stores;
        result.skipped_sessions += scanned.skipped_sessions;
        result.warnings.extend(scanned.warnings);
        for mut session in scanned.sessions {
            session.project_root = resolve(&session.cwd);
            if [
                &session.cwd,
                &session.project_root,
                &session.transcript_path,
            ]
            .iter()
            .any(|p| p.to_str().is_none())
            {
                result.skipped_sessions += 1;
                result
                    .warnings
                    .push(format!("non-UTF-8 session path skipped: {}", session.id));
                continue;
            }
            let project = projects
                .entry(session.project_root.clone())
                .or_insert_with(|| project(&session.project_root, state));
            if state.unread(&session) && !state.archived.contains(&session.id) {
                project.unread_count += 1;
            }
            project.sessions.push(session);
        }
    }
    for project in projects.values_mut() {
        project.sessions.sort_by(|a, b| {
            b.last_activity_at
                .cmp(&a.last_activity_at)
                .then_with(|| a.id.cmp(&b.id))
        });
    }
    result.projects = projects.into_values().collect();
    result
}

fn project(root: &Path, state: &State) -> Project {
    Project {
        root: root.to_owned(),
        name: root
            .file_name()
            .unwrap_or(root.as_os_str())
            .to_string_lossy()
            .into_owned(),
        sessions: vec![],
        pinned: state.pins.iter().any(|pin| pin == root),
        tags: state.tags.get(root).cloned().unwrap_or_default(),
        archived: state.archived.iter().any(|entry| Path::new(entry) == root),
        unread_count: 0,
    }
}

#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    #[default]
    Activity,
    Attention,
}
#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    #[default]
    Project,
    Tag,
}

pub fn rank(session: &crate::model::Session, state: &State) -> u8 {
    use crate::model::Status;
    match session.status {
        Status::Waiting => 0,
        Status::Error => 1,
        Status::Done if state.unread(session) => 2,
        _ => 3,
    }
}

/// Tag groups retain each session's real project_root for organizing and launch.
pub fn organize(projects: &mut Vec<Project>, state: &State, sort: Sort, group: Group) {
    if group == Group::Tag {
        let mut groups: BTreeMap<String, Project> = BTreeMap::new();
        for project in std::mem::take(projects) {
            let tags = if project.tags.is_empty() {
                vec!["untagged".into()]
            } else {
                project.tags.clone()
            };
            for tag in tags {
                let entry = groups.entry(tag.clone()).or_insert_with(|| Project {
                    root: PathBuf::from(format!("tag:{tag}")),
                    name: tag.clone(),
                    sessions: vec![],
                    pinned: false,
                    tags: vec![tag],
                    archived: true,
                    unread_count: 0,
                });
                entry.sessions.extend(project.sessions.clone());
                entry.pinned |= project.pinned;
                entry.archived &= project.archived;
                entry.unread_count += project.unread_count;
            }
        }
        *projects = groups.into_values().collect();
    }
    for project in projects.iter_mut() {
        project.sessions.sort_by(|a, b| {
            (if sort == Sort::Attention {
                rank(a, state).cmp(&rank(b, state))
            } else {
                std::cmp::Ordering::Equal
            })
            .then_with(|| b.last_activity_at.cmp(&a.last_activity_at))
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.transcript_path.cmp(&b.transcript_path))
        });
    }
    projects.sort_by(|a, b| {
        let activity = |p: &Project| {
            p.sessions
                .iter()
                .map(|s| s.last_activity_at)
                .max()
                .unwrap_or(i64::MIN)
        };
        let priority = |p: &Project| p.sessions.iter().map(|s| rank(s, state)).min().unwrap_or(4);
        b.pinned
            .cmp(&a.pinned)
            .then_with(|| {
                if sort == Sort::Attention {
                    priority(a).cmp(&priority(b))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| activity(b).cmp(&activity(a)))
            .then_with(|| a.root.cmp(&b.root))
    });
}
