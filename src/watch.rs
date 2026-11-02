//! Store watching and background scans; no terminal work runs on these threads.
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use session_board::{
    cli,
    pipeline::{self, Options},
    project::ProjectRoots,
    scan, source,
};
use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

pub const DEBOUNCE: Duration = Duration::from_secs(2);
const RECHECK: Duration = Duration::from_secs(5);
const REFRESH: Duration = Duration::from_secs(60);
enum Wake {
    Event(notify::Result<Event>),
    Stop,
}
struct Root {
    name: String,
    path: PathBuf,
    watched: Option<PathBuf>,
}
pub struct StoreWatcher {
    stop: mpsc::Sender<Wake>,
    worker: Option<thread::JoinHandle<()>>,
    pub status: Receiver<Vec<String>>,
    #[cfg(test)]
    pub observed: Receiver<()>,
}
impl StoreWatcher {
    #[cfg(test)]
    pub fn new(roots: &[PathBuf]) -> notify::Result<(Self, Receiver<()>)> {
        let (requests, receiver) = mpsc::sync_channel(16);
        let named = roots
            .iter()
            .map(|p| (p.display().to_string(), p.clone()))
            .collect();
        Ok((Self::attach(named, requests)?, receiver))
    }
    fn attach(roots: Vec<(String, PathBuf)>, requests: SyncSender<()>) -> notify::Result<Self> {
        let (events, incoming) = mpsc::channel();
        let sender = events.clone();
        let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(Wake::Event(event));
        })?;
        let (status, statuses) = mpsc::channel();
        let mut roots: Vec<_> = roots
            .into_iter()
            .map(|(name, path)| Root {
                name,
                path,
                watched: None,
            })
            .collect();
        reconcile(&mut roots, &mut watcher, &status);
        #[cfg(test)]
        let (observed, observations) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut due = None;
            let mut recheck = Instant::now() + RECHECK;
            loop {
                let deadline = due.unwrap_or(recheck).min(recheck);
                match incoming.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(Wake::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Ok(Wake::Event(Ok(event))) if !matches!(event.kind, EventKind::Access(_)) => {
                        if matches!(
                            event.kind,
                            EventKind::Remove(_)
                                | EventKind::Modify(notify::event::ModifyKind::Name(_))
                        ) {
                            for root in &mut roots {
                                if event.paths.iter().any(|p| {
                                    root.path == *p
                                        || root.path.starts_with(p)
                                        || root.watched.as_ref().is_some_and(|target| {
                                            target == p || target.starts_with(p)
                                        })
                                }) && let Some(target) = root.watched.take()
                                {
                                    let _ = watcher.unwatch(&target);
                                }
                            }
                        }
                        // A fixed window cannot be starved by a streaming transcript.
                        due.get_or_insert_with(|| Instant::now() + DEBOUNCE);
                        #[cfg(test)]
                        let _ = observed.send(());
                    }
                    Ok(Wake::Event(Err(_))) => {
                        recheck = Instant::now();
                    }
                    _ => {}
                }
                let now = Instant::now();
                if now >= recheck {
                    if reconcile(&mut roots, &mut watcher, &status) {
                        let _ = requests.try_send(());
                    }
                    recheck = now + RECHECK;
                }
                if due.is_some_and(|deadline| now >= deadline) {
                    let _ = requests.try_send(());
                    due = None;
                }
            }
        });
        Ok(Self {
            stop: events,
            worker: Some(worker),
            status: statuses,
            #[cfg(test)]
            observed: observations,
        })
    }
}

// A failed attachment must not discard the watches that succeeded.
fn reconcile(
    roots: &mut [Root],
    watcher: &mut impl Watcher,
    status: &mpsc::Sender<Vec<String>>,
) -> bool {
    let mut attached = false;
    let mut warnings = vec![];
    let mut missing = vec![];
    for root in roots {
        if root.watched.is_some()
            && (!root.path.is_dir()
                || std::fs::canonicalize(&root.path).ok().as_ref() != root.watched.as_ref())
            && let Some(target) = root.watched.take()
        {
            let _ = watcher.unwatch(&target);
        }
        if root.watched.is_none() && root.path.is_dir() {
            // notify reports canonical paths (notably /private/var on macOS).
            let target = std::fs::canonicalize(&root.path).unwrap_or_else(|_| root.path.clone());
            match watcher.watch(&target, RecursiveMode::Recursive) {
                Ok(()) => {
                    root.watched = Some(target);
                    attached = true;
                }
                Err(error) => warnings.push(format!("watcher {}: {error}", root.name)),
            }
        }
        if root.watched.is_none() {
            missing.push(root.name.clone());
        }
    }
    if !missing.is_empty() {
        warnings.insert(0, format!("unwatched: {}", missing.join(", ")));
    }
    let _ = status.send(warnings);
    attached
}
impl Drop for StoreWatcher {
    fn drop(&mut self) {
        let _ = self.stop.send(Wake::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub struct Scans {
    pub requests: SyncSender<()>,
    pub results: Receiver<io::Result<scan::Scan>>,
    pub status: Receiver<Vec<String>>,
    cancelled: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    _watcher: Option<StoreWatcher>,
}
impl Scans {
    pub fn start() -> io::Result<Self> {
        let home = cli::home()?;
        let (claude, codex) = source::homes(&home);
        let (grok, opencode) = source::database_paths(&home);
        let mut roots = ProjectRoots::default();
        let cancelled = roots.cancel_handle();
        let panic_once = std::env::var("SBOARD_SCAN_PROBE").as_deref() == Ok("panic-once");
        Self::spawn(
            vec![
                ("claude_code".into(), claude.join("projects")),
                ("codex".into(), codex.join("sessions")),
                ("grok".into(), grok.parent().unwrap_or(&home).to_path_buf()),
                (
                    "opencode".into(),
                    opencode.parent().unwrap_or(&home).to_path_buf(),
                ),
            ],
            REFRESH,
            cancelled,
            move |first| {
                if panic_once && first {
                    // Exercise a panic after the first frame, while input remains live.
                    thread::sleep(Duration::from_millis(100));
                    panic!("scan probe");
                }
                pipeline::run(&mut roots, &Options::default()).map(|outcome| outcome.scan)
            },
        )
    }
    fn spawn(
        roots: Vec<(String, PathBuf)>,
        interval: Duration,
        cancelled: Arc<AtomicBool>,
        mut scan: impl FnMut(bool) -> io::Result<scan::Scan> + Send + 'static,
    ) -> io::Result<Self> {
        let (requests, requested) = mpsc::sync_channel(1);
        let (watcher, status) = match StoreWatcher::attach(roots, requests.clone()) {
            Ok(watcher) => (Some(watcher), None),
            Err(error) => (None, Some(format!("watcher: {error}; use r to rescan"))),
        };
        // Status is a separate channel so a watch recovery updates the bar without a scan.
        let (statuses, status_receiver) = mpsc::channel();
        if let Some(warning) = status {
            let _ = statuses.send(vec![warning]);
        }
        let (completed, results) = mpsc::channel();
        let stop = cancelled.clone();
        let worker = thread::spawn(move || {
            let mut first = true;
            while !stop.load(Ordering::Relaxed) {
                // File/manual requests reset the quiet-store refresh deadline.
                if let Err(mpsc::RecvTimeoutError::Disconnected) = requested.recv_timeout(interval)
                {
                    break;
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let result =
                    catch_unwind(AssertUnwindSafe(|| scan(first))).unwrap_or_else(|panic| {
                        let message = panic
                            .downcast_ref::<&str>()
                            .copied()
                            .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                            .unwrap_or("unknown panic");
                        Err(io::Error::other(format!("panicked: {message}")))
                    });
                first = false;
                if completed.send(result).is_err() {
                    break;
                }
            }
        });
        let _ = requests.try_send(());
        drop(statuses);
        // Transfer the watch status receiver without a forwarding thread.
        let mut watcher = watcher;
        let status = watcher
            .as_mut()
            .map(|w| std::mem::replace(&mut w.status, mpsc::channel().1))
            .unwrap_or(status_receiver);
        Ok(Self {
            requests,
            results,
            status,
            cancelled,
            worker: Some(worker),
            _watcher: watcher,
        })
    }
    #[cfg(test)]
    pub fn test_scanner(
        roots: Vec<PathBuf>,
        interval: Duration,
        scan: impl FnMut(bool) -> io::Result<scan::Scan> + Send + 'static,
    ) -> Self {
        Self::spawn(
            roots.into_iter().map(|p| ("test".into(), p)).collect(),
            interval,
            Arc::new(AtomicBool::new(false)),
            scan,
        )
        .unwrap()
    }
}
impl Drop for Scans {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        let _ = self.requests.try_send(());
        self._watcher.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct LimitedWatcher;
    impl Watcher for LimitedWatcher {
        fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> {
            Ok(Self)
        }
        fn watch(&mut self, path: &std::path::Path, _: RecursiveMode) -> notify::Result<()> {
            if path.ends_with("limited") {
                Err(notify::Error::generic("watch limit"))
            } else {
                Ok(())
            }
        }
        fn unwatch(&mut self, _: &std::path::Path) -> notify::Result<()> {
            Ok(())
        }
        fn kind() -> notify::WatcherKind {
            notify::WatcherKind::NullWatcher
        }
    }
    #[test]
    fn attachment_failure_preserves_other_roots_and_reports_missing() {
        let temp = tempfile::tempdir().unwrap();
        let good = temp.path().join("good");
        let limited = temp.path().join("limited");
        std::fs::create_dir(&good).unwrap();
        std::fs::create_dir(&limited).unwrap();
        let mut roots = vec![
            Root {
                name: "good".into(),
                path: good,
                watched: None,
            },
            Root {
                name: "limited".into(),
                path: limited,
                watched: None,
            },
        ];
        let (status, statuses) = mpsc::channel();
        assert!(reconcile(&mut roots, &mut LimitedWatcher, &status));
        assert!(roots[0].watched.is_some());
        assert!(roots[1].watched.is_none());
        let warnings = statuses.recv_timeout(Duration::from_secs(10)).expect(
            "attachment_failure_preserves_other_roots_and_reports_missing: reconciliation status",
        );
        assert!(warnings.iter().any(|s| s.contains("unwatched: limited")));
        assert!(warnings.iter().any(|s| s.contains("watch limit")));
        let (watcher, _) = StoreWatcher::new(&[temp.path().join("missing")]).unwrap();
        assert!(
            watcher.status.recv_timeout(Duration::from_secs(10)).expect(
                "attachment_failure_preserves_other_roots_and_reports_missing: missing-root status"
            )[0]
            .contains("unwatched:")
        );
        assert!(watcher.observed.try_recv().is_err());
    }
    #[test]
    fn initial_scan_and_cancel_do_not_wait_for_refresh_deadline() {
        let started = Instant::now();
        let scans = Scans::test_scanner(vec![], Duration::from_secs(60), |_| {
            Ok(scan::Scan::default())
        });
        scans
            .results
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        drop(scans);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
