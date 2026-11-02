use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
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
    warnings: Vec<String>,
    cancelled: Arc<AtomicBool>,
}

impl ProjectRoots {
    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }
    /// Cache failures too: old sessions may point at directories that no longer exist.
    pub fn resolve(&mut self, cwd: &Path) -> PathBuf {
        let warnings = &mut self.warnings;
        let cancelled = &self.cancelled;
        self.cache
            .entry(cwd.to_owned())
            .or_insert_with(|| {
                let normalized = normalize(cwd, None).unwrap_or_else(|| cwd.to_owned());
                let directory = if normalized.is_file() {
                    normalized.parent().unwrap_or(&normalized)
                } else {
                    &normalized
                };
                let directory = directory
                    .ancestors()
                    .find(|p| p.file_name().is_some_and(|n| n == ".git"))
                    .and_then(Path::parent)
                    .unwrap_or(directory);
                git_root(directory, warnings, cancelled)
                    .and_then(|p| normalize(&p, None))
                    .unwrap_or_else(|| directory.to_owned())
            })
            .clone()
    }
}

fn git_root(cwd: &Path, warnings: &mut Vec<String>, cancelled: &AtomicBool) -> Option<PathBuf> {
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    let mut command = Command::new("git");
    for key in GIT_ENV {
        command.env_remove(key);
    }
    // A file avoids pipe-buffer deadlocks even with a very long root path.
    let output_file = tempfile::tempfile().ok()?;
    let mut child = command
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(output_file.try_clone().ok()?)
        .spawn()
        .ok()?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None)
                if started.elapsed() < Duration::from_secs(2)
                    && !cancelled.load(Ordering::Relaxed) =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                warnings.push(format!(
                    "git root lookup timed out or cancelled: {}",
                    cwd.display()
                ));
                return None;
            }
        }
    };
    use std::io::{Read, Seek};
    let mut output_file = output_file;
    output_file.rewind().ok()?;
    let mut bytes = vec![];
    output_file.read_to_end(&mut bytes).ok()?;
    if !status.success() {
        return None;
    }
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
) -> Vec<String> {
    let warnings = config
        .roots
        .iter()
        .filter(|p| normalize(p, Some(home)).is_none())
        .map(|p| {
            format!(
                "invalid config root ignored: {} (expected an absolute path or leading ~/)",
                p.display()
            )
        })
        .collect();
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
    warnings
}
