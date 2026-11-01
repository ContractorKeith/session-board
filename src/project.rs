use std::{
    collections::HashMap,
    path::{Path, PathBuf},
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
            .or_insert_with(|| git_root(cwd).unwrap_or_else(|| cwd.to_owned()))
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
