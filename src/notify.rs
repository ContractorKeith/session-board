//! Desktop commands and scan-to-scan status transitions.
use crate::model::{Project, Status};
use std::{collections::HashMap, io, process::Command};

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
        success(
            Command::new("notify-send")
                .args(["--app-name", "sboard", "--", title, body])
                .status()?,
        )
    }
}
pub struct Osascript;
impl Notifier for Osascript {
    fn notify(&mut self, title: &str, body: &str) -> io::Result<()> {
        // Script is constant; arbitrary transcript text is passed only as argv.
        success(Command::new("osascript").args(["-e", "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run", "--", title, body]).status()?)
    }
}
fn success(status: std::process::ExitStatus) -> io::Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "notification command exited {status}"
        )))
    }
}

pub struct Desktop {
    notifier: Box<dyn Notifier>,
    warned: bool,
}
impl Default for Desktop {
    fn default() -> Self {
        #[cfg(target_os = "macos")]
        let notifier: Box<dyn Notifier> = Box::new(Osascript);
        #[cfg(not(target_os = "macos"))]
        let notifier: Box<dyn Notifier> = Box::new(NotifySend);
        Self {
            notifier,
            warned: false,
        }
    }
}
impl Notifier for Desktop {
    fn notify(&mut self, title: &str, body: &str) -> io::Result<()> {
        if let Err(error) = self.notifier.notify(title, body) {
            if !self.warned {
                crate::cli::warn(&format!("desktop notification: {error}"));
                self.warned = true;
            }
            if error.kind() == io::ErrorKind::NotFound {
                self.notifier = Box::new(Noop);
            }
        }
        Ok(())
    }
}
#[derive(Default)]
pub struct Transitions {
    previous: Option<HashMap<String, Status>>,
}
impl Transitions {
    pub fn scan(&mut self, projects: &[Project], enabled: bool, notifier: &mut impl Notifier) {
        let mut current = HashMap::new();
        for project in projects {
            for session in &project.sessions {
                if current.insert(session.id.clone(), session.status).is_some() {
                    continue;
                }
                if enabled
                    && matches!(
                        session.status,
                        Status::Waiting | Status::Done | Status::Error
                    )
                    && self
                        .previous
                        .as_ref()
                        .is_some_and(|old| old.get(&session.id) != Some(&session.status))
                {
                    let status = match session.status {
                        Status::Waiting => "waiting",
                        Status::Error => "error",
                        _ => "done",
                    };
                    let _ = notifier.notify(
                        &format!("sboard: {} — {status}", project.name),
                        &session.title,
                    );
                }
            }
        }
        self.previous = Some(current);
    }
}
