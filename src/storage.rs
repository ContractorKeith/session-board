use serde::Serialize;
use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// An exported-but-empty override or HOME counts as unset (#12).
fn env_path(key: &str) -> Option<PathBuf> {
    env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn home() -> io::Result<PathBuf> {
    env_path("HOME").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))
}

pub(crate) fn directory(override_key: &str, fallback: &str) -> io::Result<PathBuf> {
    match env_path(override_key) {
        Some(path) => Ok(path),
        None => Ok(home()?.join(fallback)),
    }
}

/// Prefix an error with the file it concerns so two override dirs stay tellable apart (#15).
pub(crate) fn at(path: &Path, error: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("{}: {error}", path.display()))
}

/// Reject FIFOs, devices and directories before opening optional text paths. Follows
/// symlinks on purpose: dotfiles-managed config and state files are links (#13).
pub(crate) fn regular_file(path: &Path) -> io::Result<()> {
    if fs::metadata(path)?.file_type().is_file() {
        Ok(())
    } else {
        Err(io::Error::other("not a regular file"))
    }
}

pub(crate) fn read_text(path: &Path) -> io::Result<String> {
    regular_file(path)?;
    fs::read_to_string(path)
}

/// Persist beside the target so rename stays on the same filesystem. A symlinked target is
/// written through to its destination and the existing mode is kept (#13).
pub(crate) fn write_toml(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let text = toml::to_string_pretty(value).map_err(|e| at(path, e))?;
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| at(parent, e))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| at(&target, e))?;
    temporary.write_all(text.as_bytes())?;
    if let Ok(existing) = fs::metadata(&target) {
        temporary
            .as_file()
            .set_permissions(existing.permissions())?;
    }
    temporary.as_file().sync_all()?;
    temporary
        .persist(&target)
        .map_err(|error| at(&target, error.error))?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
