//! Shared command building and worktree creation for CLI and TUI.
use crate::{
    model::{Agent, Session},
    tmux,
};
use std::{
    io::{self, Read, Seek},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

/// Bound metadata commands without risking a full stdout/stderr pipe.
pub fn output(command: &mut Command) -> io::Result<Output> {
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .spawn()
        .map_err(|e| command_error(command, e))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if started.elapsed() < Duration::from_secs(2) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::other("command timed out"));
            }
        }
    };
    stdout.rewind()?;
    stderr.rewind()?;
    let mut result = Output {
        status,
        stdout: vec![],
        stderr: vec![],
    };
    stdout.read_to_end(&mut result.stdout)?;
    stderr.read_to_end(&mut result.stderr)?;
    Ok(result)
}
pub fn checked(command: &mut Command) -> io::Result<()> {
    succeeded(output(command)?)
}
fn succeeded(result: Output) -> io::Result<()> {
    if result.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "command failed: {}",
            String::from_utf8_lossy(&result.stderr)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        )))
    }
}
fn command_error(command: &Command, error: io::Error) -> io::Error {
    let name = Path::new(command.get_program())
        .file_name()
        .unwrap_or(command.get_program())
        .to_string_lossy();
    if error.kind() == io::ErrorKind::NotFound {
        io::Error::other(format!("{name}: not found on PATH"))
    } else {
        io::Error::new(error.kind(), format!("{name}: {error}"))
    }
}
static TERMINAL_BASELINE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
pub fn capture_terminal() -> io::Result<()> {
    if TERMINAL_BASELINE.get().is_none() {
        let result = Command::new("stty")
            .arg("-g")
            .stdin(std::fs::File::open("/dev/tty")?)
            .output()?;
        if !result.status.success() {
            return Err(io::Error::other("cannot capture terminal settings"));
        }
        let _ = TERMINAL_BASELINE.set(
            String::from_utf8(result.stdout)
                .map_err(io::Error::other)?
                .trim()
                .into(),
        );
    }
    Ok(())
}
pub fn restore_terminal() {
    if child_hung_up() {
        return;
    }
    if let Some(settings) = TERMINAL_BASELINE.get()
        && let Ok(tty) = std::fs::File::open("/dev/tty")
    {
        let _ = Command::new("stty")
            .arg(settings)
            .stdin(tty)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
pub fn executable(name: &str) -> io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(path) = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join(name))
            .find(|p| {
                p.metadata()
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
    }) {
        return if path.is_absolute() {
            Ok(path)
        } else {
            Ok(std::env::current_dir()?.join(path))
        };
    }
    Err(io::Error::other(format!("{name}: not found on PATH")))
}
static CHILD_INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn take_child_interrupt() -> bool {
    CHILD_INTERRUPTED.swap(false, std::sync::atomic::Ordering::SeqCst)
}
static CHILD_HUNG_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn child_hung_up() -> bool {
    CHILD_HUNG_UP.load(std::sync::atomic::Ordering::SeqCst)
}

// Block SIGTTOU only on this thread while transferring terminal ownership.
fn foreground(fd: i32, group: i32) -> io::Result<()> {
    unsafe {
        let mut blocked = std::mem::zeroed();
        let mut previous = std::mem::zeroed();
        libc::sigemptyset(&mut blocked);
        libc::sigaddset(&mut blocked, libc::SIGTTOU);
        let error = libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut previous);
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
        let result = libc::tcsetpgrp(fd, group);
        let error = io::Error::last_os_error();
        libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
        if result == -1 { Err(error) } else { Ok(()) }
    }
}

fn terminal_hung_up() -> bool {
    let mut fd = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: 0,
        revents: 0,
    };
    // A zero-time poll observes PTY closure without consuming the child's input.
    (unsafe { libc::poll(&mut fd, 1, 0) > 0 })
        && fd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
}

pub fn run(command: &mut Command) -> io::Result<()> {
    use std::{
        io::IsTerminal,
        os::unix::process::{CommandExt, ExitStatusExt},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    let tty = io::stdin().is_terminal();
    if tty {
        capture_terminal()?;
    }
    restore_terminal();
    let interrupted = Arc::new(AtomicBool::new(false));
    let hung_up = Arc::new(AtomicBool::new(false));
    // Caught signals reset to their default disposition across the child's exec.
    let int = signal_hook::flag::register(signal_hook::consts::SIGINT, interrupted.clone())?;
    let hup = match signal_hook::flag::register(signal_hook::consts::SIGHUP, hung_up.clone()) {
        Ok(id) => id,
        Err(error) => {
            signal_hook::low_level::unregister(int);
            return Err(error);
        }
    };
    let parent_group = unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) };
    command.process_group(0);
    unsafe {
        command.pre_exec(move || {
            if tty {
                foreground(libc::STDIN_FILENO, libc::getpgrp())?;
            }
            Ok(())
        });
    }
    let result = (|| {
        let mut child = command.spawn().map_err(|e| command_error(command, e))?;
        let group = child.id() as i32;
        let mut forwarded_int = false;
        loop {
            if hung_up.load(Ordering::SeqCst) || (tty && terminal_hung_up()) {
                CHILD_HUNG_UP.store(true, Ordering::SeqCst);
                // Give the whole group time to handle HUP and reap its own children.
                unsafe {
                    libc::kill(-group, libc::SIGHUP);
                }
                let deadline = Instant::now() + Duration::from_millis(500);
                // Keep the leader unreaped until escalation so its group id cannot be reused.
                while Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                // The leader may have exited while a descendant ignored HUP.
                unsafe {
                    libc::kill(-group, libc::SIGKILL);
                }
                let deadline = Instant::now() + Duration::from_secs(2);
                while child.try_wait()?.is_none() {
                    if Instant::now() >= deadline {
                        return Err(io::Error::other("agent did not exit after hangup"));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                return Err(io::Error::other("terminal hung up"));
            }
            if interrupted.load(Ordering::SeqCst) && !forwarded_int {
                unsafe {
                    libc::kill(-group, libc::SIGINT);
                }
                forwarded_int = true;
            }
            // Observe stops as well as exits: the shell tracks sboard's job group,
            // while terminal input goes to the separate agent group.
            let mut status = 0;
            let waited =
                unsafe { libc::waitpid(group, &mut status, libc::WNOHANG | libc::WUNTRACED) };
            if waited == -1 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if waited == group && libc::WIFSTOPPED(status) {
                if tty {
                    foreground(libc::STDIN_FILENO, parent_group)?;
                }
                // SIGSTOP also works when sboard is the leader of an orphaned PTY group.
                unsafe { libc::raise(libc::SIGSTOP) };
                if tty {
                    foreground(libc::STDIN_FILENO, group)?;
                }
                unsafe { libc::kill(-group, libc::SIGCONT) };
                continue;
            }
            if waited == group {
                return if std::process::ExitStatus::from_raw(status).success() {
                    Ok(())
                } else {
                    Err(io::Error::other("agent or tmux exited unsuccessfully"))
                };
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    if tty && !child_hung_up() {
        let _ = foreground(libc::STDIN_FILENO, parent_group);
    }
    restore_terminal();
    signal_hook::low_level::unregister(hup);
    signal_hook::low_level::unregister(int);
    if interrupted.load(Ordering::SeqCst) {
        CHILD_INTERRUPTED.store(true, Ordering::SeqCst);
    }
    result
}

pub fn directory(cwd: &Path) -> io::Result<()> {
    if cwd.is_dir() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "directory no longer exists: {}",
            cwd.display()
        )))
    }
}
pub fn resume_command(session: &Session) -> io::Result<Command> {
    directory(&session.cwd)?;
    let verb = match session.agent {
        Agent::ClaudeCode => "--resume",
        Agent::Codex => "resume",
        _ => return Err(io::Error::other("no resume command until M6")),
    };
    let prefix = match session.agent {
        Agent::ClaudeCode => "claude_code:",
        _ => "codex:",
    };
    let native = session
        .id
        .strip_prefix(prefix)
        .filter(|s| {
            !s.is_empty()
                && !s.starts_with('-')
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
        })
        .ok_or_else(|| io::Error::other("invalid session id"))?;
    let mut command = Command::new(executable(tmux::agent_command(session.agent))?);
    if session.agent == Agent::ClaudeCode {
        command.arg(format!("{verb}={native}"));
    } else {
        command.args([verb, "--", native]);
    }
    command.current_dir(&session.cwd);
    Ok(command)
}
pub fn open(session: &Session, socket: &str) -> io::Result<String> {
    if let Some(target) = &session.pane {
        tmux::jump(socket, target)
            .map_err(|error| io::Error::other(format!("tmux jump to {target} failed: {error}")))?;
        return Ok(with_notice(
            format!("jumped to {target}"),
            &tmux::connect(socket),
        ));
    }
    run(&mut resume_command(session)?)?;
    Ok(format!(
        "resumed {} in {}",
        session.id,
        session.cwd.display()
    ))
}

fn with_notice(message: String, connection: &tmux::Connection) -> String {
    match &connection.notice {
        Some(notice) => format!("{notice}; {message}"),
        None => message,
    }
}
pub fn launch(cwd: &Path, agent: Agent, socket: &str) -> io::Result<String> {
    directory(cwd)?;
    if !matches!(agent, Agent::ClaudeCode | Agent::Codex) {
        return Err(io::Error::other("launch supports claude_code or codex"));
    }
    let connection = tmux::connect(socket);
    if connection.inside {
        tmux::new_window(&connection, cwd, agent)?;
    } else {
        run(Command::new(executable(tmux::agent_command(agent))?).current_dir(cwd))?;
    }
    Ok(with_notice(
        format!(
            "launched {} in {}",
            tmux::agent_command(agent),
            cwd.display()
        ),
        &connection,
    ))
}
fn git(cwd: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd);
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    ] {
        cmd.env_remove(key);
    }
    cmd
}
fn git_path(mut bytes: Vec<u8>) -> PathBuf {
    // rev-parse writes one terminator; a newline may also be part of the name.
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec(bytes).into()
}
fn main_worktree(root: &Path) -> io::Result<PathBuf> {
    let result = output(git(root).args(["worktree", "list", "--porcelain", "-z"]))?;
    if !result.status.success() {
        return Err(io::Error::other("cannot find main worktree"));
    }
    let first = result.stdout.split(|b| *b == 0).next().unwrap_or_default();
    let path = first
        .strip_prefix(b"worktree ")
        .ok_or_else(|| io::Error::other("cannot find main worktree"))?;
    use std::os::unix::ffi::OsStringExt;
    Ok(std::ffi::OsString::from_vec(path.to_vec()).into())
}
fn git_error(result: &Output) -> String {
    let stderr = String::from_utf8_lossy(&result.stderr);
    let details = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("Preparing worktree"))
        .collect::<Vec<_>>()
        .join(" | ");
    if details.is_empty() {
        "git failed".into()
    } else {
        details
    }
}
pub fn worktree(project: &Path, branch: &str) -> io::Result<PathBuf> {
    directory(project)?;
    if branch == "@" || branch.starts_with("refs/") || branch.len() > 200 {
        return Err(io::Error::other("invalid branch name"));
    }
    let root = output(git(project).args(["rev-parse", "--show-toplevel"]))?;
    if !root.status.success() {
        return Err(io::Error::other("not a git repo"));
    }
    let validated = output(git(project).args(["check-ref-format", "--branch", branch]))?;
    if !validated.status.success()
        || validated.stdout != format!("{branch}\n").as_bytes()
        || branch.starts_with('-')
        || branch.split('/').any(|part| part == "." || part == "..")
    {
        return Err(io::Error::other("invalid branch name"));
    }
    let root = git_path(root.stdout);
    if output(git(&root).args([
        "show-ref",
        "--verify",
        "--quiet",
        &format!("refs/tags/{branch}"),
    ]))?
    .status
    .success()
    {
        return Err(io::Error::other("branch name matches an existing tag"));
    }
    let main = main_worktree(&root)?;
    let superproject = output(git(&root).args(["rev-parse", "--show-superproject-working-tree"]))?;
    let superproject = git_path(superproject.stdout);
    let anchor = if superproject.as_os_str().is_empty() {
        main.clone()
    } else {
        main_worktree(&superproject)?
    };
    let name = main
        .file_name()
        .ok_or_else(|| io::Error::other("project has no directory name"))?;
    let mut sibling = name.to_os_string();
    sibling.push("-wt");
    let path = anchor
        .parent()
        .ok_or_else(|| io::Error::other("project has no parent"))?
        .join(sibling)
        .join(branch);
    if path.symlink_metadata().is_ok() {
        return Err(io::Error::other(format!(
            "worktree exists: {}",
            path.display()
        )));
    }
    let mut ancestor = path.parent();
    while let Some(p) = ancestor {
        if p.symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(io::Error::other("worktree parent is a symlink"));
        }
        ancestor = p.parent();
    }
    let exists = output(git(&root).args([
        "show-ref",
        "--verify",
        "--quiet",
        &format!("refs/heads/{branch}"),
    ]))?
    .status
    .success();
    let mut remote = None;
    if !exists {
        let refs = output(git(&root).args([
            "for-each-ref",
            "--format=%(refname:strip=2)",
            "refs/remotes",
        ]))?;
        let refs = String::from_utf8(refs.stdout).map_err(io::Error::other)?;
        let remotes = output(git(&root).arg("remote"))?;
        let remotes = String::from_utf8(remotes.stdout).map_err(io::Error::other)?;
        let candidates: Vec<_> = remotes
            .lines()
            .map(|remote| format!("{remote}/{branch}"))
            .filter(|name| refs.lines().any(|r| r == name))
            .collect();
        match candidates.as_slice() {
            [only] => remote = Some(only.clone()),
            [] => {}
            _ => return Err(io::Error::other("branch exists on multiple remotes")),
        }
    }
    // Successful branch creation, rather than the earlier lookup, establishes ownership.
    let created_branch = if exists {
        false
    } else {
        let mut create = git(&root);
        create.arg("branch");
        if remote.is_some() {
            create.arg("--track");
        }
        let result = create
            .arg(branch)
            .arg(remote.as_deref().unwrap_or("HEAD"))
            .stdin(Stdio::null())
            .output()?;
        if result.status.success() {
            true
        } else if output(git(&root).args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ]))?
        .status
        .success()
        {
            false
        } else {
            return Err(io::Error::other(format!(
                "branch creation failed: {}",
                git_error(&result)
            )));
        }
    };
    let mut cmd = git(&root);
    cmd.args(["worktree", "add"]).arg(&path).arg(branch);
    // Checkouts may take longer than metadata discovery.
    let result = cmd.stdin(Stdio::null()).output()?;
    if !result.status.success() {
        let message = git_error(&result);
        // The path was absent before this attempt; preserve pre-existing branches.
        let _ = output(
            git(&root)
                .args(["worktree", "remove", "--force"])
                .arg(&path),
        );
        if path.exists() {
            let _ = std::fs::remove_dir_all(&path);
        }
        if created_branch {
            let _ = output(git(&root).args(["branch", "-D", "--", branch]));
        }
        return Err(io::Error::other(format!("worktree add failed: {message}")));
    }
    Ok(path)
}
