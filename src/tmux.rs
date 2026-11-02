//! Pane metadata only: never read pane contents or send keys.
use crate::{
    launch,
    model::{Agent, Session},
};
use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub session: String,
    pub window: String,
    pub index: String,
    pub cwd: PathBuf,
    pub command: String,
    pub pid: u32,
}
impl Pane {
    pub fn target(&self) -> String {
        format!("{}:{}.{}", self.session, self.window, self.index)
    }
}
pub fn command(socket: &str) -> Command {
    let mut command = Command::new("tmux");
    if !socket.is_empty() {
        command.args(["-L", socket]);
    }
    command
}
pub fn parse(text: &str) -> Vec<Pane> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 6 {
                return None;
            }
            Some(Pane {
                session: fields[0].into(),
                window: fields[1].into(),
                index: fields[2].into(),
                cwd: fields[3].into(),
                command: fields[4].into(),
                pid: fields[5].parse().ok()?,
            })
        })
        .collect()
}
pub fn discover(socket: &str) -> Vec<Pane> {
    let mut cmd = command(socket);
    cmd.args(["list-panes", "-a", "-F", "#{session_name}\t#{window_index}\t#{pane_index}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_pid}"]);
    launch::output(&mut cmd)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}
pub fn agent_command(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "claude",
        Agent::Codex => "codex",
        Agent::Grok => "grok",
        Agent::Opencode => "opencode",
    }
}
fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
pub fn matching<'a>(session: &Session, panes: &'a [Pane]) -> Option<&'a Pane> {
    let name = agent_command(session.agent);
    panes
        .iter()
        .filter(|p| same_path(&p.cwd, &session.cwd))
        .find(|p| p.command == name || descendant_has_command(p.pid, name))
}

fn descendant_has_command(pid: u32, name: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return false;
        };
        let processes: Vec<_> = entries
            .flatten()
            .filter_map(|entry| {
                let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
                let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
                let start = stat.find('(')?;
                let end = stat.rfind(')')?;
                let parent: u32 = stat[end + 1..].split_whitespace().nth(1)?.parse().ok()?;
                Some((pid, parent, stat[start + 1..end].to_owned()))
            })
            .collect();
        descendants(&processes, pid, name)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut cmd = Command::new("ps");
        cmd.args(["-axo", "pid=,ppid=,comm="]);
        let Ok(output) = launch::output(&mut cmd) else {
            return false;
        };
        let processes: Vec<_> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let pid = words.next()?.parse().ok()?;
                let parent = words.next()?.parse().ok()?;
                let command = Path::new(words.next()?).file_name()?.to_str()?.to_owned();
                Some((pid, parent, command))
            })
            .collect();
        descendants(&processes, pid, name)
    }
}
fn descendants(processes: &[(u32, u32, String)], pid: u32, name: &str) -> bool {
    let mut pending = vec![pid];
    let mut seen = std::collections::HashSet::new();
    while let Some(parent) = pending.pop() {
        if !seen.insert(parent) {
            continue;
        }
        for (pid, ppid, command) in processes {
            if *ppid == parent {
                if command == name {
                    return true;
                }
                pending.push(*pid);
            }
        }
    }
    false
}
pub fn inside() -> bool {
    std::env::var_os("TMUX").is_some_and(|v| !v.is_empty())
}
pub fn jump(socket: &str, target: &str) -> io::Result<()> {
    let (session, rest) = target
        .rsplit_once(':')
        .ok_or_else(|| io::Error::other("invalid pane target"))?;
    let (window, _) = rest
        .split_once('.')
        .ok_or_else(|| io::Error::other("invalid pane target"))?;
    if inside() {
        launch::checked(command(socket).args(["switch-client", "-t", &format!("={session}")]))?;
    }
    launch::checked(command(socket).args([
        "select-window",
        "-t",
        &format!("={session}:{window}"),
    ]))?;
    launch::checked(command(socket).args(["select-pane", "-t", &format!("={target}")]))?;
    if !inside() {
        launch::run(command(socket).args(["attach", "-t", &format!("={session}")]))?;
    }
    Ok(())
}
pub fn new_window(socket: &str, cwd: &Path, agent: Agent) -> io::Result<()> {
    launch::checked(
        command(socket)
            .args(["new-window", "-c"])
            .arg(cwd)
            .arg(agent_command(agent)),
    )
}

#[cfg(test)]
mod tests {
    use super::descendants;
    #[test]
    fn child_and_grandchild_commands_match_without_unrelated_processes() {
        let processes = vec![
            (11, 10, "sh".into()),
            (12, 11, "codex".into()),
            (21, 20, "claude".into()),
        ];
        assert!(descendants(&processes, 10, "codex"));
        assert!(!descendants(&processes, 10, "claude"));
    }
}
