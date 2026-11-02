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
        .spawn()?;
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
pub fn run(command: &mut Command) -> io::Result<()> {
    if command.status()?.success() {
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
        .filter(|s| !s.is_empty())
        .ok_or_else(|| io::Error::other("invalid session id"))?;
    let mut command = Command::new(tmux::agent_command(session.agent));
    command.arg(verb).arg(native).current_dir(&session.cwd);
    Ok(command)
}
pub fn open(session: &Session, socket: &str) -> io::Result<String> {
    if let Some(target) = &session.pane {
        tmux::jump(socket, target)?;
        Ok(format!("jumped to {target}"))
    } else {
        run(&mut resume_command(session)?)?;
        Ok(format!("resumed {}", session.id))
    }
}
pub fn launch(cwd: &Path, agent: Agent, socket: &str) -> io::Result<String> {
    directory(cwd)?;
    if !matches!(agent, Agent::ClaudeCode | Agent::Codex) {
        return Err(io::Error::other("launch supports claude_code or codex"));
    }
    if tmux::inside() {
        tmux::new_window(socket, cwd, agent)?;
    } else {
        run(Command::new(tmux::agent_command(agent)).current_dir(cwd))?;
    }
    Ok(format!(
        "launched {} in {}",
        tmux::agent_command(agent),
        cwd.display()
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
pub fn worktree(project: &Path, branch: &str) -> io::Result<PathBuf> {
    directory(project)?;
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
    let root = PathBuf::from(
        String::from_utf8(root.stdout)
            .map_err(io::Error::other)?
            .trim_end_matches('\n'),
    );
    let name = root
        .file_name()
        .ok_or_else(|| io::Error::other("project has no directory name"))?
        .to_string_lossy();
    let path = root
        .parent()
        .ok_or_else(|| io::Error::other("project has no parent"))?
        .join(format!("{name}-wt"))
        .join(branch);
    if path.symlink_metadata().is_ok() {
        return Err(io::Error::other(format!(
            "worktree exists: {}",
            path.display()
        )));
    }
    // Reject symlink ancestors so the requested sibling directory cannot escape elsewhere.
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
    let mut cmd = git(&root);
    cmd.args(["worktree", "add"]);
    if !exists {
        cmd.args(["-b", branch]);
    }
    cmd.arg(&path);
    if exists {
        cmd.arg(branch);
    }
    // A checkout can take longer than metadata discovery; let git finish atomically.
    succeeded(cmd.stdin(Stdio::null()).output()?)?;
    Ok(path)
}
