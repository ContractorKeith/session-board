use session_board::notify::{Delivery, Notifier};
use std::{
    io,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

struct Blocked {
    entered: Sender<()>,
    release: Receiver<()>,
    completed: Sender<()>,
}
impl Notifier for Blocked {
    fn notify(&mut self, _: &str, _: &str) -> io::Result<()> {
        let _ = self.entered.send(());
        self.release
            .recv_timeout(Duration::from_secs(30))
            .map_err(io::Error::other)?;
        let _ = self.completed.send(());
        Ok(())
    }
}
struct Gate {
    entered: Receiver<()>,
    release: Sender<()>,
    completed: Receiver<()>,
}
impl Gate {
    fn started(&self) {
        self.entered
            .recv_timeout(Duration::from_secs(10))
            .expect("worker did not start");
    }
    fn allow(&self, count: usize) {
        for _ in 0..count {
            let _ = self.release.send(());
        }
    }
    fn finished(&self, count: usize) {
        for _ in 0..count {
            self.completed
                .recv_timeout(Duration::from_secs(10))
                .expect("worker did not finish");
        }
        self.entered.try_iter().for_each(drop);
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        self.allow(1001);
    }
}
fn blocked() -> (Delivery, Gate) {
    let (entered, starts) = mpsc::channel();
    let (permit, release) = mpsc::channel();
    let (completed, finishes) = mpsc::channel();
    let worker = Delivery::start(Blocked {
        entered,
        release,
        completed,
    })
    .unwrap();
    (
        worker,
        Gate {
            entered: starts,
            release: permit,
            completed: finishes,
        },
    )
}
#[test]
fn delivery_is_nonblocking_and_bounded_under_backpressure() {
    let (mut worker, gate) = blocked();
    worker.notify("first", "body").unwrap();
    gate.started();
    let (sent, result) = mpsc::channel();
    let producer = std::thread::spawn(move || {
        for _ in 0..1000 {
            worker.notify("title", "body").unwrap();
        }
        let warnings = worker.take_warnings();
        let _ = sent.send((worker, warnings));
    });
    // Enqueue finishes while the notifier is still blocked, independent of CPU scheduling.
    let result = result.recv_timeout(Duration::from_secs(5));
    gate.allow(1001);
    let (mut worker, warnings) = result.expect("enqueue blocked behind delivery");
    producer.join().unwrap();
    gate.finished(17);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("queue full"));
    assert!(worker.take_warnings().is_empty());
}

#[test]
#[ignore]
fn desktop_failure_probe() {
    let mode = std::env::var("NOTIFY_MODE").unwrap();
    let mut worker = Delivery::desktop().unwrap();
    let count = if mode == "fail" { 4 } else { 1 };
    for _ in 0..count {
        worker.notify("title", "body").unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut warnings = vec![];
    while Instant::now() < deadline {
        warnings.extend(worker.take_warnings());
        let calls = std::fs::read_to_string(std::env::var("NOTIFY_LOG").unwrap())
            .unwrap_or_default()
            .lines()
            .count();
        if !warnings.is_empty() && (mode != "fail" || calls == 4) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    #[cfg(target_os = "macos")]
    assert!(warnings[0].contains("osascript"));
    #[cfg(not(target_os = "macos"))]
    assert!(warnings[0].contains("notify-send"));
    assert!(worker.take_warnings().is_empty());
    if matches!(mode.as_str(), "timeout" | "fork") {
        assert!(warnings[0].contains("exceeded 5 s"));
        let pids = std::fs::read_to_string(std::env::var("NOTIFY_LOG").unwrap()).unwrap();
        for pid in pids.lines() {
            let deadline = Instant::now() + Duration::from_secs(5);
            let dead = loop {
                let state = std::process::Command::new("/bin/ps")
                    .args(["-o", "stat=", "-p", pid])
                    .output()
                    .unwrap();
                let state = String::from_utf8_lossy(&state.stdout);
                // A killed orphan may briefly await init's reap; it is no longer running.
                if state.trim().is_empty() || state.trim().starts_with('Z') {
                    break true;
                }
                if Instant::now() >= deadline {
                    break false;
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            if !dead {
                let _ = std::process::Command::new("/bin/kill")
                    .args(["-KILL", pid])
                    .output();
            }
            assert!(dead, "timed-out notification left process {pid} alive");
        }
    }
}
#[test]
#[cfg(unix)]
fn repeatedly_failing_command_warns_once_without_stderr() {
    failure_case("fail");
}
#[test]
#[cfg(unix)]
fn timed_out_command_is_killed_and_warns_once() {
    failure_case("timeout");
}
#[cfg(unix)]
fn failure_case(mode: &str) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    #[cfg(target_os = "macos")]
    let name = "osascript";
    #[cfg(not(target_os = "macos"))]
    let name = "notify-send";
    let path = dir.path().join(name);
    let script = if mode == "fork" {
        "#!/bin/sh\n/bin/sleep 30 &\nprintf '%s\\n' \"$$\" \"$!\" >> \"$NOTIFY_LOG\"\nwait\n"
    } else if mode == "timeout" {
        "#!/bin/sh\nprintf '%s\\n' \"$$\" >> \"$NOTIFY_LOG\"\nexec /bin/sleep 30\n"
    } else {
        "#!/bin/sh\nprintf 'call\\n' >> \"$NOTIFY_LOG\"\nexit 1\n"
    };
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "desktop_failure_probe",
            "--nocapture",
        ])
        .env("PATH", dir.path())
        .env("NOTIFY_LOG", dir.path().join("calls"))
        .env("NOTIFY_MODE", mode)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore]
fn markup_probe() {
    session_board::notify::NotifySend
        .notify(
            &format!("title\n\u{1b}[31m{}\u{1b}[0m", "界".repeat(300)),
            &format!("<b>&{}", "<&>界".repeat(100)),
        )
        .unwrap();
}
#[test]
#[cfg(unix)]
fn notify_send_escapes_markup_and_caps_complete_arguments() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("notify-send");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$NOTIFY_LOG\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let log = dir.path().join("args");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "markup_probe"])
        .env("PATH", dir.path())
        .env("NOTIFY_LOG", &log)
        .output()
        .unwrap();
    assert!(output.status.success());
    let args = std::fs::read_to_string(log).unwrap();
    let args: Vec<_> = args.lines().collect();
    assert_eq!(args.len(), 5, "{args:?}");
    assert_eq!(args[3].chars().count(), 200);
    assert!(args[3].starts_with("title 界"));
    assert!(!args[3].contains('\u{1b}'));
    assert!(args[4].starts_with("&lt;b&gt;&amp;"));
    assert!(args[4].chars().count() <= 200);
    assert!(!args[4].contains(['<', '>']));
    assert!(!args[4].ends_with('&'));
}

#[test]
fn queue_overflow_warns_again_after_delivery_recovers() {
    let (mut worker, gate) = blocked();
    for episode in 0..2 {
        worker.notify("start", "body").unwrap();
        gate.started();
        for _ in 0..20 {
            worker.notify("overflow", "body").unwrap();
        }
        let warnings = worker.take_warnings();
        gate.allow(17);
        gate.finished(17);
        assert_eq!(warnings.len(), 1, "episode {episode} was silent");
        assert!(warnings[0].contains("queue full"));
        assert!(worker.take_warnings().is_empty());
    }
}

#[test]
#[cfg(unix)]
fn timed_out_forking_command_has_no_live_descendants() {
    failure_case("fork");
}
