//! Desktop commands and scan-to-scan status transitions.
use crate::model::{Project, Status};
use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub trait Notifier {
    fn notify(&mut self, title: &str, body: &str) -> io::Result<()>;
}
#[derive(Default)]
pub struct Recording {
    pub calls: Vec<(String, String)>,
}
impl Notifier for Recording {
    fn notify(&mut self, title: &str, body: &str) -> io::Result<()> {
        self.calls.push((title.into(), body.into()));
        Ok(())
    }
}
pub struct Noop;
impl Notifier for Noop {
    fn notify(&mut self, _: &str, _: &str) -> io::Result<()> {
        Ok(())
    }
}
pub struct NotifySend;
impl Notifier for NotifySend {
    fn notify(&mut self, title: &str, body: &str) -> io::Result<()> {
        notify_send(title, body, &AtomicBool::new(false))
    }
}
pub struct Osascript;
impl Notifier for Osascript {
    fn notify(&mut self, title: &str, body: &str) -> io::Result<()> {
        osascript(title, body, &AtomicBool::new(false))
    }
}
fn text(value: &str) -> String {
    crate::source::title(value).chars().take(200).collect()
}
fn markup(value: &str) -> String {
    let mut escaped = String::new();
    let mut length = 0;
    for c in text(value).chars() {
        let entity = match c {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            _ => "",
        };
        let width = if entity.is_empty() { 1 } else { entity.len() };
        if length + width > 200 {
            break;
        }
        if entity.is_empty() {
            escaped.push(c);
        } else {
            escaped.push_str(entity);
        }
        length += width;
    }
    escaped
}
fn notify_send(title: &str, body: &str, cancelled: &AtomicBool) -> io::Result<()> {
    let title = text(title);
    let body = markup(body);
    run(
        Command::new("notify-send").args(["--app-name", "sboard", "--", &title, &body]),
        "notify-send",
        cancelled,
    )
}
fn osascript(title: &str, body: &str, cancelled: &AtomicBool) -> io::Result<()> {
    // Script is constant; transcript text is passed only as argv.
    let title = text(title);
    let body = text(body);
    run(Command::new("osascript").args(["-e", "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run", "--", &title, &body]), "osascript", cancelled)
}
fn run(command: &mut Command, name: &str, cancelled: &AtomicBool) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut execute = || -> io::Result<()> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other(status.to_string()))
                };
            }
            if cancelled.load(Ordering::Relaxed) || started.elapsed() >= Duration::from_secs(5) {
                stop(&mut child);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "delivery cancelled or exceeded 5 s",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    };
    execute().map_err(|error| io::Error::new(error.kind(), format!("{name}: {error}")))
}
fn stop(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: std::os::raw::c_int, signal: std::os::raw::c_int) -> std::os::raw::c_int;
        }
        if let Ok(pid) = i32::try_from(child.id())
            && pid > 0
        {
            // SAFETY: this unreaped child owns the isolated group created before spawn.
            unsafe {
                kill(-pid, signal_hook::consts::SIGKILL);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}
#[derive(Default)]
pub struct Desktop {
    missing: bool,
    cancelled: Arc<AtomicBool>,
}
impl Notifier for Desktop {
    fn notify(&mut self, title: &str, body: &str) -> io::Result<()> {
        if self.missing {
            return Noop.notify(title, body);
        }
        #[cfg(target_os = "macos")]
        let result = osascript(title, body, &self.cancelled);
        #[cfg(not(target_os = "macos"))]
        let result = notify_send(title, body, &self.cancelled);
        if result
            .as_ref()
            .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
        {
            self.missing = true;
        }
        result
    }
}

/// A bounded queue keeps desktop processes off the event-loop thread.
pub struct Delivery {
    sender: Option<SyncSender<(String, String)>>,
    warnings: Receiver<String>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    queue_warned: bool,
    queue_warning: Option<String>,
}
impl Delivery {
    pub fn desktop() -> io::Result<Self> {
        let cancelled = Arc::default();
        Self::spawn(
            Desktop {
                missing: false,
                cancelled: Arc::clone(&cancelled),
            },
            cancelled,
        )
    }
    pub fn start(notifier: impl Notifier + Send + 'static) -> io::Result<Self> {
        Self::spawn(notifier, Default::default())
    }
    fn spawn(
        mut notifier: impl Notifier + Send + 'static,
        cancelled: Arc<AtomicBool>,
    ) -> io::Result<Self> {
        let (sender, requests) = mpsc::sync_channel::<(String, String)>(16);
        let (reports, warnings) = mpsc::sync_channel(16);
        let stop = cancelled.clone();
        let worker = thread::Builder::new()
            .name("notifications".into())
            .spawn(move || {
                let mut warned = false;
                while let Ok((title, body)) = requests.recv() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Err(error) = notifier.notify(&title, &body)
                        && !warned
                    {
                        let _ = reports.try_send(format!("desktop notification: {error}"));
                        warned = true;
                    }
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            warnings,
            cancelled,
            worker: Some(worker),
            queue_warned: false,
            queue_warning: None,
        })
    }
    pub fn take_warnings(&mut self) -> Vec<String> {
        self.queue_warning
            .take()
            .into_iter()
            .chain(self.warnings.try_iter())
            .collect()
    }
}
impl Notifier for Delivery {
    fn notify(&mut self, title: &str, body: &str) -> io::Result<()> {
        match self
            .sender
            .as_ref()
            .unwrap()
            .try_send((title.into(), body.into()))
        {
            Ok(()) => self.queue_warned = false,
            Err(_) if !self.queue_warned => {
                self.queue_warned = true;
                self.queue_warning = Some(
                    "desktop notification: queue full or worker stopped; notification dropped"
                        .into(),
                );
            }
            Err(_) => {}
        }
        Ok(())
    }
}
impl Drop for Delivery {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
type Identity = (PathBuf, String);
struct Announced {
    event: (Status, i64),
    floor_activity: Option<i64>,
}
#[derive(Default)]
pub struct Transitions {
    previous: HashMap<Identity, (Status, i64)>,
    announced: HashMap<Identity, Vec<Announced>>,
    clock: Option<i64>,
}
impl Transitions {
    pub fn scan(&mut self, projects: &[Project], enabled: bool, notifier: &mut impl Notifier) {
        self.scan_at(projects, enabled, notifier, unix_time());
    }
    pub fn scan_at(
        &mut self,
        projects: &[Project],
        enabled: bool,
        notifier: &mut impl Notifier,
        now: i64,
    ) {
        self.scan_at_with_state(
            projects,
            &crate::state::State::default(),
            enabled,
            notifier,
            now,
        );
    }
    pub fn scan_with_state(
        &mut self,
        projects: &[Project],
        state: &crate::state::State,
        enabled: bool,
        notifier: &mut impl Notifier,
    ) {
        self.scan_at_with_state(projects, state, enabled, notifier, unix_time());
    }
    fn scan_at_with_state(
        &mut self,
        projects: &[Project],
        state: &crate::state::State,
        enabled: bool,
        notifier: &mut impl Notifier,
        now: i64,
    ) {
        // Raw pairs still deduplicate across clock steps; floors belong to one clock interval.
        if self.clock.is_some_and(|previous| now < previous) {
            for history in self.announced.values_mut() {
                for entry in history {
                    entry.floor_activity = None;
                }
            }
        }
        let mut current = HashMap::new();
        let mut notices = Vec::with_capacity(5);
        let mut changed_count = 0;
        for project in projects {
            for session in &project.sessions {
                let key = (session.transcript_path.clone(), session.id.clone());
                if current.contains_key(&key) {
                    continue;
                }
                let event = (session.status, session.last_activity_at);
                let history = self.announced.get(&key);
                let announced =
                    history.is_some_and(|events| events.iter().any(|e| e.event == event));
                let floor = history.and_then(|events| events.iter().find_map(|e| e.floor_activity));
                let changed = self.clock.is_some()
                    && self.previous.get(&key).map_or(
                        now.saturating_sub(session.last_activity_at) <= 5 * 60,
                        |seen| *seen != event,
                    );
                current.insert(key.clone(), event);
                if enabled
                    && !project.archived
                    && !state.session_archived(session)
                    && changed
                    && !announced
                    && floor.is_none_or(|oldest| session.last_activity_at >= oldest)
                    && matches!(
                        session.status,
                        Status::Waiting | Status::Done | Status::Error
                    )
                {
                    let history = self.announced.entry(key).or_default();
                    if history.len() == 32 {
                        history.remove(0);
                    }
                    history.push(Announced {
                        event,
                        floor_activity: Some(session.last_activity_at.min(now)),
                    });
                    changed_count += 1;
                    if changed_count > 5 {
                        continue;
                    }
                    let status = match session.status {
                        Status::Waiting => "waiting",
                        Status::Error => "error",
                        _ => "done",
                    };
                    notices.push((
                        text(&format!(
                            "sboard: {} — {status}",
                            crate::source::title(&project.name)
                        )),
                        text(&session.title),
                    ));
                }
            }
        }
        if changed_count > 5 {
            let _ = notifier.notify(
                &format!("sboard: {changed_count} sessions need you"),
                "Waiting, done or error sessions changed.",
            );
        } else {
            for (title, body) in notices {
                let _ = notifier.notify(&title, &body);
            }
        }
        // Missing stores and rolled-back transcripts must not erase known activity.
        self.previous.extend(current);
        self.clock = Some(now);
    }
}

fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announcement_storage_is_bounded_even_when_the_floor_hides_eviction() {
        let mut projects: Vec<Project> =
            serde_json::from_str(include_str!("../tests/fixtures/attention-projects.json"))
                .unwrap();
        projects.truncate(1);
        projects[0].sessions.truncate(1);
        projects[0].sessions[0].status = Status::Working;
        let mut tracker = Transitions::default();
        let mut recording = Recording::default();
        tracker.scan_at(&projects, true, &mut recording, 1000);
        for activity in 1001..=1040 {
            projects[0].sessions[0].status = Status::Waiting;
            projects[0].sessions[0].last_activity_at = activity;
            tracker.scan_at(&projects, true, &mut recording, activity);
        }
        assert_eq!(recording.calls.len(), 40);
        let history = tracker.announced.values().next().unwrap();
        assert_eq!(history.len(), 32);
        assert_eq!(history[0].event, (Status::Waiting, 1009));
    }
    #[test]
    fn archived_transitions_stay_silent_without_replaying_or_muting_read_marks() {
        for project_archive in [false, true] {
            let mut projects: Vec<Project> =
                serde_json::from_str(include_str!("../tests/fixtures/attention-projects.json"))
                    .unwrap();
            projects.truncate(1);
            projects[0].sessions.truncate(1);
            let id = projects[0].sessions[0].id.clone();
            let root = projects[0].sessions[0]
                .project_root
                .to_string_lossy()
                .into_owned();
            let mut state = crate::state::State {
                first_seen: Some(0),
                ..Default::default()
            };
            state
                .archived
                .push(if project_archive { root } else { id.clone() });
            let mut tracker = Transitions::default();
            let mut recording = Recording::default();
            projects[0].sessions[0].status = Status::Working;
            projects[0].sessions[0].last_activity_at = 1000;
            tracker.scan_at_with_state(&projects, &state, true, &mut recording, 1000);
            projects[0].sessions[0].status = Status::Done;
            projects[0].sessions[0].last_activity_at = 1001;
            tracker.scan_at_with_state(&projects, &state, true, &mut recording, 1001);
            assert!(recording.calls.is_empty());
            state.archived.clear();
            tracker.scan_at_with_state(&projects, &state, true, &mut recording, 1002);
            assert!(
                recording.calls.is_empty(),
                "unarchive replayed a muted transition"
            );
            projects[0].sessions[0].status = Status::Waiting;
            projects[0].sessions[0].last_activity_at = 1003;
            tracker.scan_at_with_state(&projects, &state, true, &mut recording, 1003);
            assert_eq!(recording.calls.len(), 1);
            projects[0].sessions[0].status = Status::Done;
            projects[0].sessions[0].last_activity_at = 1004;
            state.read.insert(id, 1004);
            assert!(!state.unread(&projects[0].sessions[0]));
            tracker.scan_at_with_state(&projects, &state, true, &mut recording, 1004);
            assert_eq!(
                recording.calls.len(),
                2,
                "read marks suppressed transition notifications"
            );
        }
    }
}
