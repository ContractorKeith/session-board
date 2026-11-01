use crate::{config::Config, model::Project, source::Registry, state::State};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Default)]
pub struct Scan {
    pub projects: Vec<Project>,
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
    for root in &config.roots {
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
            let project = projects
                .entry(session.project_root.clone())
                .or_insert_with(|| project(&session.project_root, state));
            if state
                .read
                .get(&session.id)
                .is_none_or(|read| session.last_activity_at > *read)
            {
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
