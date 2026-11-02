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
pub fn run(command: &mut Command) -> io::Result<()> {
    use std::io::IsTerminal;
    if io::stdin().is_terminal() {
        capture_terminal()?;
    }
    restore_terminal();
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let interrupted = Arc::new(AtomicBool::new(false));
    // A caught parent signal resets to the default disposition across the child's exec.
    let registration =
        signal_hook::flag::register(signal_hook::consts::SIGINT, interrupted.clone())?;
    let result = command.status();
    restore_terminal();
    signal_hook::low_level::unregister(registration);
    if interrupted.load(Ordering::SeqCst) {
        CHILD_INTERRUPTED.store(true, Ordering::SeqCst);
    }
    if result.map_err(|e| command_error(command, e))?.success() {
        Ok(())
    } else {
        Err(io::Error::other("agent or tmux exited unsuccessfully"))
    }
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
        match tmux::jump(socket, target) {
            Ok(()) => {
                return Ok(with_notice(
                    format!("jumped to {target}"),
                    &tmux::connect(socket),
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    run(&mut resume_command(session)?)?;
    Ok(format!("resumed {}", session.id))
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
    let mut cmd = git(&root);
    cmd.args(["worktree", "add"]);
    if remote.is_some() {
        cmd.arg("--track");
    }
    if !exists {
        cmd.args(["-b", branch]);
    }
    cmd.arg(&path);
    if let Some(remote) = remote {
        cmd.arg(remote);
    } else if exists {
        cmd.arg(branch);
    }
    // Checkouts may take longer than metadata discovery.
    let result = cmd.stdin(Stdio::null()).output()?;
    if !result.status.success() {
        let message = String::from_utf8_lossy(&result.stderr)
            .lines()
            .next()
            .unwrap_or("git failed")
            .to_owned();
        // The path was absent before this attempt; preserve pre-existing branches.
        let _ = output(
            git(&root)
                .args(["worktree", "remove", "--force"])
                .arg(&path),
        );
        if path.exists() {
            let _ = std::fs::remove_dir_all(&path);
        }
        if !exists {
            let _ = output(git(&root).args(["branch", "-D", "--", branch]));
        }
        return Err(io::Error::other(format!("worktree add failed: {message}")));
    }
    Ok(path)
}
