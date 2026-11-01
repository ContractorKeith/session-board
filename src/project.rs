use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

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
    let output = Command::new("git")
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
