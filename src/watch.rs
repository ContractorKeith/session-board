//! Store watching and background scans; the UI only receives completed results.
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use session_board::{
    cli,
    config::Config,
    project::{ProjectRoots, normalize_settings},
    scan, source,
    state::State,
};
use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::{Duration, Instant},
};

pub const DEBOUNCE: Duration = Duration::from_secs(2);

pub struct StoreWatcher {
    _watcher: RecommendedWatcher,
    stop: SyncSender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl StoreWatcher {
    pub fn new(roots: &[PathBuf]) -> notify::Result<(Self, Receiver<()>)> {
        let (events, incoming) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            if let Ok(event) = event
                && !matches!(event.kind, notify::EventKind::Access(_))
            {
                let _ = events.send(());
            }
        })?;
        for root in roots.iter().filter(|root| root.is_dir()) {
            watcher.watch(root, RecursiveMode::Recursive)?;
        }
        let (requests, receiver) = mpsc::channel();
        let (stop, stopped) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let mut changed = None;
            loop {
                if stopped.try_recv().is_ok() {
                    break;
                }
                match incoming.recv_timeout(Duration::from_millis(20)) {
                    Ok(()) => changed = Some(Instant::now()),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if changed.is_some_and(|time| time.elapsed() >= DEBOUNCE) {
                    if requests.send(()).is_err() {
                        break;
                    }
                    changed = None;
                }
            }
        });
        Ok((
            Self {
                _watcher: watcher,
                stop,
                worker: Some(worker),
            },
            receiver,
        ))
    }
}
impl Drop for StoreWatcher {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub struct Scans {
    pub requests: SyncSender<()>,
    pub results: Receiver<io::Result<scan::Scan>>,
    pub changes: Receiver<()>,
    pub warning: Option<String>,
    _watcher: Option<StoreWatcher>,
}
impl Scans {
    pub fn start() -> io::Result<Self> {
        let home = cli::home()?;
        let (claude, codex) = source::homes(&home);
        let (watcher, changes, warning) =
            match StoreWatcher::new(&[claude.join("projects"), codex.join("sessions")]) {
                Ok((watcher, changes)) => (Some(watcher), changes, None),
                Err(error) => (
                    None,
                    mpsc::channel().1,
                    Some(format!("watcher: {error}; use r to rescan")),
                ),
            };
        let (requests, requested) = mpsc::sync_channel(1);
        let (completed, results) = mpsc::channel();
        thread::spawn(move || {
            while requested.recv().is_ok() {
                let result = (|| {
                    let (mut config, mut warnings) = Config::load(&Config::path()?)?;
                    let (mut state, state_warnings) = State::load(&State::path()?)?;
                    warnings.extend(state_warnings);
                    warnings.extend(normalize_settings(&mut config, &mut state, &home));
                    let mut roots = ProjectRoots::default();
                    let mut result = scan::scan(&source::registry(), &config, &state, |cwd| {
                        roots.resolve(cwd)
                    });
                    cli::filter(&mut result.projects, &state, None, None, None, false);
                    result.warnings.extend(warnings);
                    Ok(result)
                })();
                if completed.send(result).is_err() {
                    break;
                }
            }
        });
        let _ = requests.try_send(());
        Ok(Self {
            requests,
            results,
            changes,
            warning,
            _watcher: watcher,
        })
    }
}
