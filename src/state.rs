use crate::storage;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub first_seen: Option<i64>,
    pub sort: crate::scan::Sort,
    pub group: crate::scan::Group,
    pub show_archived: bool,
    pub pins: Vec<PathBuf>,
    pub tags: BTreeMap<PathBuf, Vec<String>>,
    /// Project roots or session IDs, as specified by the state file contract.
    pub archived: Vec<String>,
    pub read: BTreeMap<String, i64>,
}

impl State {
    pub fn path() -> io::Result<PathBuf> {
        Ok(storage::directory("SBOARD_STATE_DIR", ".local/state/sboard")?.join("state.toml"))
    }

    /// A missing file is empty state. Anything else that keeps the file from loading
    /// (bad TOML, invalid UTF-8, a directory, no permission) is reported to the caller
    /// and treated as empty; state is never a reason for a command to fail (#11).
    pub fn load(path: &Path) -> io::Result<(Self, Vec<String>)> {
        let text = match storage::read_text(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok((Self::default(), vec![]));
            }
            Err(error) => return Ok((Self::default(), vec![corrupt(path, error)])),
        };
        match toml::from_str(&text) {
            Ok(state) => Ok((state, vec![])),
            Err(error) => Ok((Self::default(), vec![corrupt(path, error)])),
        }
    }

    pub fn unread(&self, session: &crate::model::Session) -> bool {
        self.read
            .get(&session.id)
            .copied()
            .or(self.first_seen)
            .is_some_and(|seen| session.last_activity_at > seen)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        self.save_changes(path, None)
    }

    /// Explicit removals are relative to the snapshot edited by this instance.
    pub fn save_changes(&self, path: &Path, previous: Option<&Self>) -> io::Result<()> {
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        // Lock the directory: locking the state inode would not survive atomic rename.
        let lock = std::fs::File::open(parent)?;
        lock.lock()?;
        let (mut merged, warnings) = Self::load(&target)?;
        if !warnings.is_empty() {
            storage::regular_file(&target)?;
            let stamp = now();
            let mut backup = target.as_os_str().to_owned();
            backup.push(format!(".corrupt-{stamp}"));
            let backup = PathBuf::from(backup);
            // Never overwrite an earlier backup in the same second.
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&backup)?;
            let mut input = std::fs::File::open(&target)?;
            output.set_permissions(input.metadata()?.permissions())?;
            std::io::copy(&mut input, &mut output)?;
            output.sync_all()?;
            lock.sync_all()?;
            crate::cli::warn(&format!("corrupt state preserved at {}", backup.display()));
        }
        if let Some(old) = previous {
            merged
                .pins
                .retain(|p| !old.pins.contains(p) || self.pins.contains(p));
            merged
                .archived
                .retain(|p| !old.archived.contains(p) || self.archived.contains(p));
            for (root, tags) in &old.tags {
                if let Some(current) = merged.tags.get_mut(root) {
                    current.retain(|tag| {
                        !tags.contains(tag) || self.tags.get(root).is_some_and(|v| v.contains(tag))
                    });
                }
            }
        }
        merged.first_seen = merged.first_seen.or(self.first_seen).or(Some(now()));
        merged.sort = self.sort;
        merged.group = self.group;
        merged.show_archived = self.show_archived;
        merged.pins.extend(self.pins.clone());
        merged.pins.sort();
        merged.pins.dedup();
        merged.pins.retain(|p| valid_path(p));
        merged.tags.retain(|p, _| valid_path(p));
        for (root, tags) in &self.tags {
            if !valid_path(root) {
                continue;
            }
            let values = merged.tags.entry(root.clone()).or_default();
            values.extend(tags.clone());
            values.sort();
            values.dedup();
        }
        merged.tags.retain(|_, values| !values.is_empty());
        merged.archived.extend(self.archived.clone());
        merged.archived.sort();
        merged.archived.dedup();
        for (id, time) in &self.read {
            merged
                .read
                .entry(id.clone())
                .and_modify(|t| *t = (*t).max(*time))
                .or_insert(*time);
        }
        storage::write_toml(&target, &merged)
    }
}

fn corrupt(path: &Path, error: impl std::fmt::Display) -> String {
    format!("{}: corrupt state ignored: {error}", path.display())
}

fn valid_path(path: &Path) -> bool {
    if path.to_str().is_some() {
        true
    } else {
        crate::cli::warn(&format!("non-UTF-8 state path skipped: {}", path.display()));
        false
    }
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
