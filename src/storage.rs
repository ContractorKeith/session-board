use serde::Serialize;
use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

pub(crate) fn directory(override_key: &str, fallback: &str) -> io::Result<PathBuf> {
    if let Some(path) = env::var_os(override_key) {
        return Ok(PathBuf::from(path));
    }
    let home = env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(PathBuf::from(home).join(fallback))
}

/// Persist beside the target so rename stays on the same filesystem.
pub(crate) fn write_toml(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let text = toml::to_string_pretty(value).map_err(io::Error::other)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(text.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}
