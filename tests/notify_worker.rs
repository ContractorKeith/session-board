use session_board::notify::{Delivery, Notifier};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

struct Blocked {
    entered: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
}
impl Notifier for Blocked {
    fn notify(&mut self, _: &str, _: &str) -> io::Result<()> {
        self.entered.store(true, Ordering::SeqCst);
        while !self.release.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
#[test]
fn delivery_is_nonblocking_and_bounded_under_backpressure() {
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut worker = Delivery::start(Blocked {
        entered: entered.clone(),
        release: release.clone(),
        calls: calls.clone(),
    })
    .unwrap();
    worker.notify("first", "body").unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while !entered.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::yield_now();
    }
    let start = Instant::now();
    for _ in 0..1000 {
        worker.notify("title", "body").unwrap();
    }
    let elapsed = start.elapsed();
    let warnings = worker.take_warnings();
    release.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(1);
    while calls.load(Ordering::SeqCst) < 17 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(elapsed < Duration::from_millis(100), "{elapsed:?}");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("queue full"));
    assert_eq!(calls.load(Ordering::SeqCst), 17);
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
    let deadline = Instant::now() + Duration::from_secs(7);
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
    if mode == "timeout" {
        assert!(warnings[0].contains("exceeded 5 s"));
        let pid = std::fs::read_to_string(std::env::var("NOTIFY_LOG").unwrap()).unwrap();
        assert!(
            !std::process::Command::new("/bin/kill")
                .args(["-0", pid.trim()])
                .output()
                .unwrap()
                .status
                .success(),
            "timed-out child still alive"
        );
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
    let script = if mode == "timeout" {
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
