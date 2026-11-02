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
    #[serde(deserialize_with = "deserialize_tags")]
    pub tags: BTreeMap<PathBuf, Vec<String>>,
    /// Project roots or session IDs, as specified by the state file contract.
    pub archived: Vec<String>,
    pub read: BTreeMap<String, i64>,
}

impl State {
    pub fn path() -> io::Result<PathBuf> {
        Ok(storage::directory("SBOARD_STATE_DIR", ".local/state/sboard")?.join("state.toml"))
    }

    /// Loading is lenient; saving re-reads strictly before replacing anything.
    pub fn load(path: &Path) -> io::Result<(Self, Vec<String>)> {
        let bytes = match read_bytes(path) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok((Self::default(), vec![])),
            Err(error) => return Ok((Self::default(), vec![corrupt(path, error)])),
        };
        match parse(&bytes) {
            Ok(mut state) => {
                let warnings = state.normalize();
                Ok((state, warnings))
            }
            Err(error) => Ok((Self::default(), vec![corrupt(path, error)])),
        }
    }

    pub fn normalize(&mut self) -> Vec<String> {
        let mut warnings = vec![];
        self.pins.retain(|p| valid_path(p, &mut warnings));
        self.tags.retain(|p, _| valid_path(p, &mut warnings));
        if let Ok(home) = storage::home() {
            // Resolve dangling links too: their spelling must keep the target's identity.
            let resolve = |root: &Path| {
                let expanded = root
                    .strip_prefix("~")
                    .map_or_else(|_| root.to_owned(), |tail| home.join(tail));
                if !expanded.is_absolute() {
                    return expanded;
                }
                let target = storage::target(&expanded).unwrap_or(expanded);
                crate::project::normalize(&target, Some(&home)).unwrap_or(target)
            };
            self.pins = self.pins.iter().map(|p| resolve(p)).collect();
            let mut tags: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
            for (root, values) in &self.tags {
                tags.entry(resolve(root))
                    .or_default()
                    .extend(values.clone());
            }
            self.tags = tags;
            for entry in &mut self.archived {
                if entry.starts_with('/') || Path::new(entry).starts_with("~") {
                    *entry = resolve(Path::new(entry)).to_string_lossy().into_owned();
                }
            }
            warnings.extend(crate::project::normalize_settings(
                &mut crate::config::Config::default(),
                self,
                &home,
            ));
        }
        self.pins.sort();
        self.pins.dedup();
        for tags in self.tags.values_mut() {
            *tags = tags
                .iter()
                .map(|tag| crate::source::title(tag))
                .filter(|t| !t.is_empty())
                .collect();
            tags.sort();
            tags.dedup();
        }
        self.tags.retain(|_, tags| !tags.is_empty());
        warnings
    }

    pub fn session_archived(&self, session: &crate::model::Session) -> bool {
        self.archived.contains(&session.id)
            || self
                .archived
                .contains(&session.project_root.to_string_lossy().into_owned())
    }

    pub fn unread(&self, session: &crate::model::Session) -> bool {
        !self.session_archived(session)
            && self
                .read
                .get(&session.id)
                .copied()
                .or(self.first_seen)
                .is_some_and(|seen| session.last_activity_at > seen)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        self.save_changes(path, None).map(|_| ())
    }

    /// Apply only edits since this instance's last successful load/save.
    pub fn merge(&self, previous: &Self, mut current: Self) -> Self {
        merge_set(&mut current.pins, &previous.pins, &self.pins);
        merge_set(&mut current.archived, &previous.archived, &self.archived);
        for root in previous.tags.keys().chain(self.tags.keys()) {
            let empty = vec![];
            let tags = current.tags.entry(root.clone()).or_default();
            merge_set(
                tags,
                previous.tags.get(root).unwrap_or(&empty),
                self.tags.get(root).unwrap_or(&empty),
            );
        }
        current.tags.retain(|_, tags| !tags.is_empty());
        if self.sort != previous.sort {
            current.sort = self.sort;
        }
        if self.group != previous.group {
            current.group = self.group;
        }
        if self.show_archived != previous.show_archived {
            current.show_archived = self.show_archived;
        }
        current.first_seen = current.first_seen.or(self.first_seen);
        for (id, time) in &self.read {
            current
                .read
                .entry(id.clone())
                .and_modify(|t| *t = (*t).max(*time))
                .or_insert(*time);
        }
        current
    }

    pub fn save_changes(&self, path: &Path, previous: Option<&Self>) -> io::Result<Vec<String>> {
        let target = storage::target(path)?;
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| storage::at(parent, e))?;
        let lock = std::fs::File::open(parent)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(storage::at(
                        path,
                        "state lock timed out after 2 seconds; another sboard instance is saving",
                    ));
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(storage::at(path, error)),
            }
        }
        let mut warnings = vec![];
        // Read fully before creating a backup; read failures never leave empty copies.
        let mut current = match read_bytes(&target).map_err(|e| storage::at(&target, e))? {
            None => Self::default(),
            Some(bytes) => match parse(&bytes) {
                Ok(state) => state,
                Err(_) => {
                    let mut backup = target.as_os_str().to_owned();
                    backup.push(format!(".corrupt-{}", now()));
                    let backup = PathBuf::from(backup);
                    let mut output = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&backup)?;
                    output.set_permissions(std::fs::metadata(&target)?.permissions())?;
                    use std::io::Write;
                    output.write_all(&bytes)?;
                    output.sync_all()?;
                    lock.sync_all()?;
                    warnings.push(format!("corrupt state preserved at {}", backup.display()));
                    Self::default()
                }
            },
        };
        warnings.extend(current.normalize());
        let mut edited = self.clone();
        warnings.extend(edited.normalize());
        let mut baseline = previous.cloned().unwrap_or_default();
        warnings.extend(baseline.normalize());
        let now = now();
        current
            .read
            .retain(|_, time| *time >= now - 90 * 86400 && *time <= now);
        edited
            .read
            .retain(|_, time| *time >= now - 90 * 86400 && *time <= now);
        let mut merged = edited.merge(&baseline, current);
        merged.first_seen = merged.first_seen.or(Some(now));

        storage::write_toml(&target, &merged)?;
        Ok(warnings)
    }

    pub fn cleanup(path: &Path) -> Vec<String> {
        let Ok(target) = storage::target(path) else {
            return vec![];
        };
        let Some(parent) = target.parent() else {
            return vec![];
        };
        let Ok(entries) = std::fs::read_dir(parent) else {
            return vec![];
        };
        let mut warnings = vec![];
        for entry in entries.flatten() {
            if !entry.file_name().to_string_lossy().starts_with(".tmp") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if entry.file_type().is_ok_and(|kind| kind.is_file())
                && metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age.as_secs() > 600)
                && let Err(error) = std::fs::remove_file(entry.path())
            {
                warnings.push(format!(
                    "stale state temporary file {}: {error}",
                    entry.path().display()
                ));
            }
        }
        warnings
    }
}

fn merge_set<T: Clone + Ord>(current: &mut Vec<T>, previous: &[T], edited: &[T]) {
    current.retain(|value| !previous.contains(value) || edited.contains(value));
    current.extend(edited.iter().filter(|v| !previous.contains(v)).cloned());
    current.sort();
    current.dedup();
}
fn read_bytes(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match storage::regular_file(path) {
        Ok(()) => std::fs::read(path).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
fn parse(bytes: &[u8]) -> Result<State, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    toml::from_str(text).map_err(|e| e.to_string())
}
fn corrupt(path: &Path, error: impl std::fmt::Display) -> String {
    format!("{}: corrupt state ignored: {error}", path.display())
}
fn valid_path(path: &Path, warnings: &mut Vec<String>) -> bool {
    if path.to_str().is_some() {
        true
    } else {
        warnings.push(format!("non-UTF-8 state path skipped: {}", path.display()));
        false
    }
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// PathBuf considers /p and /p/ equal; union raw keys before that comparison can lose data.
fn deserialize_tags<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<PathBuf, Vec<String>>, D::Error> {
    let raw = BTreeMap::<String, Vec<String>>::deserialize(deserializer)?;
    let mut tags: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    for (root, values) in raw {
        tags.entry(PathBuf::from(root)).or_default().extend(values);
    }
    Ok(tags)
}
