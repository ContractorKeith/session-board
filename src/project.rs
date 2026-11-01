use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    process::Command,
};

/// Inherited git environment (hooks, `rebase --exec`, editor terminals) would make
/// `rev-parse` answer for the wrong repository (#14).
const GIT_ENV: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
];

#[derive(Default)]
pub struct ProjectRoots {
    cache: HashMap<PathBuf, PathBuf>,
}

impl ProjectRoots {
    /// Cache failures too: old sessions may point at directories that no longer exist.
    pub fn resolve(&mut self, cwd: &Path) -> PathBuf {
        self.cache
            .entry(cwd.to_owned())
            .or_insert_with(|| {
                let normalized = normalize(cwd, None).unwrap_or_else(|| cwd.to_owned());
                let directory = normalized
                    .ancestors()
                    .find(|p| p.file_name().is_some_and(|n| n == ".git"))
                    .and_then(Path::parent)
                    .unwrap_or(&normalized);
                git_root(directory)
                    .and_then(|p| normalize(&p, None))
                    .unwrap_or_else(|| directory.to_owned())
            })
            .clone()
    }
}

fn git_root(cwd: &Path) -> Option<PathBuf> {
    let mut command = Command::new("git");
    for key in GIT_ENV {
        command.env_remove(key);
    }
    let output = command
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut bytes = output.stdout;
    // Remove git's line ending, preserving whitespace in the actual path.
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        (!bytes.is_empty()).then(|| PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes)
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    }
}

/// One identity for existing symlinks and missing paths. Relative session cwds are
/// rejected by sources; config paths may expand a leading home component.
pub fn normalize(path: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let expanded;
    let path = if let Ok(suffix) = path.strip_prefix("~") {
        expanded = home?.join(suffix);
        expanded.as_path()
    } else {
        path
    };
    if !path.is_absolute() {
        return None;
    }
    if let Ok(path) = std::fs::canonicalize(path) {
        return Some(path);
    }
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => (),
            other => result.push(other.as_os_str()),
        }
    }
    Some(result)
}

pub fn normalize_settings(
    config: &mut crate::config::Config,
    state: &mut crate::state::State,
    home: &Path,
) {
    config.roots = config
        .roots
        .iter()
        .filter_map(|p| normalize(p, Some(home)))
        .collect();
    config.roots.sort();
    config.roots.dedup();
    state.pins = state
        .pins
        .iter()
        .filter_map(|p| normalize(p, Some(home)))
        .collect();
    let mut tags = std::collections::BTreeMap::<PathBuf, Vec<String>>::new();
    for (path, values) in &state.tags {
        if let Some(path) = normalize(path, Some(home)) {
            tags.entry(path).or_default().extend(values.iter().cloned());
        }
    }
    for values in tags.values_mut() {
        values.sort();
        values.dedup();
    }
    state.tags = tags;
    for entry in &mut state.archived {
        if let Some(path) = normalize(Path::new(entry), Some(home)) {
            *entry = path.to_string_lossy().into_owned();
        }
    }
}
