//! Pane metadata only: never read pane contents or send keys.
use crate::{
    launch,
    model::{Agent, Project, Session, Status},
};
use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    process::Command,
};

const FORMAT: &str = "#{session_name}\x1f#{window_index}\x1f#{pane_index}\x1f#{pane_current_command}\x1f#{pane_pid}\x1f#{pane_current_path}\x1e";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub session: String,
    pub window: String,
    pub index: String,
    pub cwd: PathBuf,
    pub command: String,
    pub pid: u32,
    agents: Vec<Agent>,
}
impl Pane {
    pub fn target(&self) -> String {
        format!("{}:{}.{}", self.session, self.window, self.index)
    }
}
pub struct Discovery {
    pub panes: Vec<Pane>,
    pub warnings: Vec<String>,
}
pub struct Connection {
    socket: String,
    path: bool,
    pub inside: bool,
    pub notice: Option<String>,
}
impl Connection {
    pub fn command(&self) -> Command {
        let mut cmd = Command::new("tmux");
        if !self.socket.is_empty() {
            cmd.args([if self.path { "-S" } else { "-L" }, &self.socket]);
        }
        cmd
    }
}
pub fn connect(configured: &str) -> Connection {
    if let Some(socket) = std::env::var("TMUX").ok().and_then(|v| {
        let (prefix, _) = v.rsplit_once(',')?;
        let (socket, _) = prefix.rsplit_once(',')?;
        (!socket.is_empty()).then(|| socket.to_owned())
    }) {
        let mut connection = Connection {
            socket,
            path: true,
            inside: true,
            notice: None,
        };
        if launch::output(connection.command().args(["display", "-p", ""]))
            .is_ok_and(|o| o.status.success())
        {
            if !configured.is_empty()
                && Path::new(&connection.socket)
                    .file_name()
                    .is_none_or(|n| n != configured)
            {
                connection.notice = Some(format!(
                    "using TMUX socket {} instead of {configured}",
                    connection.socket
                ));
            }
            return connection;
        }
    }
    Connection {
        socket: configured.into(),
        path: false,
        inside: false,
        notice: None,
    }
}
pub fn inside() -> bool {
    connect("").inside
}
pub fn escape(value: &str) -> String {
    value.replace('#', "##")
}
fn parsed(text: &str) -> Discovery {
    let mut result = Discovery {
        panes: vec![],
        warnings: vec![],
    };
    let records: Vec<_> = if text.contains("\x1e\n") {
        text.split("\x1e\n").collect()
    } else {
        text.split('\n').collect()
    };
    for record in records.into_iter().filter(|r| !r.is_empty()) {
        let fields: Vec<_> = record.splitn(6, '\x1f').collect();
        let pane = (|| {
            if fields.len() != 6 {
                return None;
            }
            let command = fields[3].to_owned();
            Some(Pane {
                session: fields[0].into(),
                window: fields[1].into(),
                index: fields[2].into(),
                cwd: fields[5].into(),
                pid: fields[4].parse().ok()?,
                agents: AGENTS
                    .into_iter()
                    .filter(|a| agent_command(*a) == command)
                    .collect(),
                command,
            })
        })();
        match pane {
            Some(p) => result.panes.push(p),
            None => result
                .warnings
                .push("tmux: unparseable pane metadata".into()),
        }
    }
    result
}
pub fn parse(text: &str) -> Vec<Pane> {
    parsed(text).panes
}
pub fn discover(socket: &str) -> Discovery {
    discover_with(&connect(socket), process_table)
}
fn discover_with(
    connection: &Connection,
    processes: impl FnOnce() -> Vec<(u32, u32, String)>,
) -> Discovery {
    let Ok(output) = launch::output(
        connection
            .command()
            .args(["list-panes", "-a", "-F", FORMAT]),
    ) else {
        return Discovery {
            panes: vec![],
            warnings: vec![],
        };
    };
    if !output.status.success() {
        return Discovery {
            panes: vec![],
            warnings: vec![],
        };
    }
    let mut result = identify(parsed(&String::from_utf8_lossy(&output.stdout)), processes);
    if let Some(notice) = &connection.notice {
        result.warnings.push(notice.clone());
    }
    result
}
fn identify(
    mut result: Discovery,
    processes: impl FnOnce() -> Vec<(u32, u32, String)>,
) -> Discovery {
    if !result.panes.is_empty() {
        let processes = processes();
        for pane in &mut result.panes {
            for agent in AGENTS {
                if !pane.agents.contains(&agent)
                    && descendants(&processes, pane.pid, agent_command(agent))
                {
                    pane.agents.push(agent);
                }
            }
        }
    }
    result
}
#[cfg(test)]
std::thread_local! { static PROCESS_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
const AGENTS: [Agent; 4] = [
    Agent::ClaudeCode,
    Agent::Codex,
    Agent::Grok,
    Agent::Opencode,
];
pub fn agent_command(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "claude",
        Agent::Codex => "codex",
        Agent::Grok => "grok",
        Agent::Opencode => "opencode",
    }
}
pub fn matching<'a>(session: &Session, panes: &'a [Pane]) -> Option<&'a Pane> {
    let cwd = session.cwd.canonicalize().ok()?;
    panes
        .iter()
        .find(|p| p.agents.contains(&session.agent) && p.cwd.canonicalize().is_ok_and(|p| p == cwd))
}
pub fn refresh(projects: &mut [Project], socket: &str) -> Vec<String> {
    let discovery = discover(socket);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    assign(projects, &discovery.panes, now);
    discovery.warnings
}
pub fn assign(projects: &mut [Project], panes: &[Pane], now: i64) {
    let mut sessions: Vec<_> = projects.iter_mut().flat_map(|p| &mut p.sessions).collect();
    sessions.sort_by(|a, b| {
        b.last_activity_at
            .cmp(&a.last_activity_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut groups = HashSet::new();
    let mut used = HashSet::new();
    for session in sessions {
        session.pane = None;
        let Ok(cwd) = session.cwd.canonicalize() else {
            continue;
        };
        if !groups.insert((cwd, agent_command(session.agent))) {
            continue;
        }
        if !matches!(
            session.status,
            Status::Working | Status::Waiting | Status::Done
        ) || now.saturating_sub(session.last_activity_at) > 3600
        {
            continue;
        }
        if let Some(pane) = matching(session, panes) {
            let target = pane.target();
            if used.insert(target.clone()) {
                session.pane = Some(target);
            }
        }
    }
}
fn process_table() -> Vec<(u32, u32, String)> {
    #[cfg(test)]
    PROCESS_READS.with(|c| c.set(c.get() + 1));
    #[cfg(target_os = "linux")]
    {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return vec![];
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let pid = entry.file_name().to_str()?.parse().ok()?;
                let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
                let start = stat.find('(')?;
                let end = stat.rfind(')')?;
                Some((
                    pid,
                    stat[end + 1..].split_whitespace().nth(1)?.parse().ok()?,
                    stat[start + 1..end].into(),
                ))
            })
            .collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let Ok(output) = launch::output(Command::new("ps").args(["-axo", "pid=,ppid=,comm="]))
        else {
            return vec![];
        };
        parse_processes(&String::from_utf8_lossy(&output.stdout))
    }
}
#[cfg(any(test, not(target_os = "linux")))]
fn parse_processes(text: &str) -> Vec<(u32, u32, String)> {
    text.lines()
        .filter_map(|line| {
            let (pid, rest) = line.trim_start().split_once(char::is_whitespace)?;
            let (parent, command) = rest.trim_start().split_once(char::is_whitespace)?;
            let command = Path::new(command.trim()).file_name()?.to_str()?.to_owned();
            Some((pid.parse().ok()?, parent.parse().ok()?, command))
        })
        .collect()
}
fn descendants(processes: &[(u32, u32, String)], pid: u32, name: &str) -> bool {
    let mut children: HashMap<u32, Vec<_>> = HashMap::new();
    for (pid, parent, command) in processes {
        children.entry(*parent).or_default().push((*pid, command));
    }
    let mut pending = vec![pid];
    let mut seen = HashSet::new();
    while let Some(parent) = pending.pop() {
        if !seen.insert(parent) {
            continue;
        }
        for (pid, command) in children.get(&parent).into_iter().flatten() {
            if *command == name {
                return true;
            }
            pending.push(*pid);
        }
    }
    false
}
pub fn jump(socket: &str, target: &str) -> io::Result<()> {
    let connection = connect(socket);
    let (session, rest) = target
        .rsplit_once(':')
        .ok_or_else(|| io::Error::other("invalid pane target"))?;
    let (window, _) = rest
        .split_once('.')
        .ok_or_else(|| io::Error::other("invalid pane target"))?;
    // Empty display formats succeed even for missing targets; require an actual pane id.
    let live = launch::output(connection.command().args([
        "display",
        "-p",
        "-t",
        &format!("={}", escape(target)),
        "#{pane_id}",
    ]))?;
    if !live.status.success() || live.stdout.iter().all(u8::is_ascii_whitespace) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "pane no longer exists",
        ));
    }
    if connection.inside {
        launch::checked(connection.command().args([
            "switch-client",
            "-t",
            &format!("={}", escape(session)),
        ]))?;
    }
    launch::checked(connection.command().args([
        "select-window",
        "-t",
        &format!("={}:{}", escape(session), escape(window)),
    ]))?;
    launch::checked(connection.command().args([
        "select-pane",
        "-t",
        &format!("={}", escape(target)),
    ]))?;
    if !connection.inside {
        launch::run(
            connection
                .command()
                .args(["attach", "-t", &format!("={}", escape(session))]),
        )?;
    }
    Ok(())
}
pub fn new_window(connection: &Connection, cwd: &Path, agent: Agent) -> io::Result<()> {
    let executable = launch::executable(agent_command(agent))?;
    let mut command = connection.command();
    command.args([
        "new-window",
        "-c",
        &escape(
            cwd.to_str()
                .ok_or_else(|| io::Error::other("project path is not UTF-8"))?,
        ),
    ]);
    for key in ["CLAUDE_CONFIG_DIR", "CODEX_HOME"] {
        if let Some(value) = std::env::var_os(key) {
            let mut entry = std::ffi::OsString::from(format!("{key}="));
            entry.push(value);
            command.arg("-e").arg(entry);
        }
    }
    // Multiple argv elements make tmux exec directly, bypassing shell rc files.
    launch::checked(command.arg(executable).arg("--"))
}
pub fn doctor(socket: &str) -> String {
    let connection = connect(socket);
    let Ok(version) = launch::output(connection.command().arg("-V")) else {
        return "not installed".into();
    };
    if !version.status.success() {
        return "not installed".into();
    }
    if !launch::output(connection.command().args(["display", "-p", ""]))
        .is_ok_and(|o| o.status.success())
    {
        return "no server".into();
    }
    String::from_utf8_lossy(&version.stdout).trim().to_owned()
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
    #[test]
    fn issue51_process_table_once_for_fifty_sessions_twenty_panes() {
        use super::*;
        let cwd = tempfile::tempdir().unwrap();
        let capture: String = (0..20)
            .map(|i| {
                format!(
                    "dev\x1f0\x1f{i}\x1fbash\x1f999999999\x1f{}\x1e\n",
                    cwd.path().display()
                )
            })
            .collect();
        let mut projects: Vec<Project> =
            serde_json::from_str(include_str!("../fixtures/list.json")).unwrap();
        let mut session = projects.remove(0).sessions.remove(0);
        session.cwd = cwd.path().into();
        PROCESS_READS.with(|c| c.set(0));
        let started = std::time::Instant::now();
        let discovery = identify(parsed(&capture), process_table);
        for _ in 0..50 {
            assert!(matching(&session, &discovery.panes).is_none());
        }
        assert_eq!(PROCESS_READS.with(|c| c.get()), 1);
        assert!(started.elapsed() < std::time::Duration::from_millis(300));
    }

    #[test]
    fn issue52_malformed_records_warn_and_newlines_and_tabs_survive() {
        let result = super::parsed(
            "bad record\x1e\ndev\x1f0\x1f0\x1fclaude\x1f1\x1f/project\ttab\nnewline\x1e\n",
        );
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(
            result.panes[0].cwd,
            std::path::Path::new("/project\ttab\nnewline")
        );
    }
    #[test]
    fn issue50_latest_session_must_be_eligible() {
        use super::*;
        let cwd = tempfile::tempdir().unwrap();
        let panes = parse(&format!(
            "dev\x1f0\x1f0\x1fclaude\x1f1\x1f{}\x1e\n",
            cwd.path().display()
        ));
        let original: Vec<Project> =
            serde_json::from_str(include_str!("../fixtures/list.json")).unwrap();
        for (status, age, expected) in [
            (Status::Working, 1, 1),
            (Status::Waiting, 1, 1),
            (Status::Done, 1, 1),
            (Status::Idle, 1, 0),
            (Status::Error, 1, 0),
            (Status::Done, 3601, 0),
        ] {
            let mut projects = original.clone();
            projects.truncate(1);
            projects[0].sessions.truncate(1);
            let newest = &mut projects[0].sessions[0];
            newest.cwd = cwd.path().into();
            newest.agent = Agent::ClaudeCode;
            newest.status = status;
            newest.last_activity_at = 10000 - age;
            let mut older = newest.clone();
            older.id = "claude_code:older".into();
            older.status = Status::Done;
            older.last_activity_at -= 1;
            projects[0].sessions.push(older);
            assign(&mut projects, &panes, 10000);
            assert_eq!(
                projects[0]
                    .sessions
                    .iter()
                    .filter(|s| s.pane.is_some())
                    .count(),
                expected,
                "{status:?}, age {age}"
            );
            assert!(projects[0].sessions[1].pane.is_none());
        }
    }
    #[test]
    fn ps_process_names_keep_spaces_in_the_executable_path() {
        assert_eq!(
            super::parse_processes(" 11  10 /Users/test name/bin/codex\n"),
            vec![(11, 10, "codex".into())]
        );
    }
}
