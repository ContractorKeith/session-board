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

const FORMAT: &str = "#{q:session_name}\x1f#{window_index}\x1f#{pane_index}\x1f#{q:pane_current_command}\x1f#{pane_pid}\x1f#{q:pane_current_path}\x1e";

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
fn escape_format(value: &str) -> String {
    value.replace('#', "##")
}
// tmux q: prefixes shell-special characters (including newlines) with a backslash.
// Decode data only; never pass the quoted representation to a shell.
fn unquote(value: &str) -> Option<String> {
    let mut decoded = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => decoded.push(chars.next()?),
            '\n' | '\r' | '\t' | '\x1e' | '\x1f' => return None,
            _ => decoded.push(c),
        }
    }
    (!decoded.contains(['\x1e', '\x1f'])).then_some(decoded)
}
fn parsed(text: &str) -> Discovery {
    let mut result = Discovery {
        panes: vec![],
        warnings: vec![],
    };
    for record in text.split("\x1e\n").filter(|r| !r.is_empty()) {
        let fields: Vec<_> = record.split('\x1f').collect();
        let pane = (|| {
            if fields.len() != 6 {
                return None;
            }
            let session = unquote(fields[0])?;
            let command = unquote(fields[3])?;
            let cwd = unquote(fields[5])?;
            fields[1].parse::<u32>().ok()?;
            fields[2].parse::<u32>().ok()?;
            Some(Pane {
                session,
                window: fields[1].into(),
                index: fields[2].into(),
                cwd: cwd.into(),
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
        let children = children(&processes);
        for pane in &mut result.panes {
            for agent in AGENTS {
                if !pane.agents.contains(&agent)
                    && descendants(&children, pane.pid, agent_command(agent))
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
        if !matches!(
            session.status,
            Status::Working | Status::Waiting | Status::Done
        ) || now.saturating_sub(session.last_activity_at) > 3600
        {
            continue;
        }
        if !groups.insert((cwd, agent_command(session.agent))) {
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
fn children(processes: &[(u32, u32, String)]) -> HashMap<u32, Vec<(u32, &str)>> {
    let mut children: HashMap<u32, Vec<_>> = HashMap::new();
    for (pid, parent, command) in processes {
        children
            .entry(*parent)
            .or_default()
            .push((*pid, command.as_str()));
    }
    children
}
fn descendants(children: &HashMap<u32, Vec<(u32, &str)>>, pid: u32, name: &str) -> bool {
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
        &format!("={target}"),
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
            &format!("={session}"),
        ]))?;
    }
    launch::checked(connection.command().args([
        "select-window",
        "-t",
        &format!("={session}:{window}"),
    ]))?;
    launch::checked(
        connection
            .command()
            .args(["select-pane", "-t", &format!("={target}")]),
    )?;
    if !connection.inside {
        use std::io::{Read, Seek};
        let mut stderr = tempfile::tempfile()?;
        let result = launch::run(
            connection
                .command()
                .args(["attach", "-t", &format!("={session}")])
                .stderr(stderr.try_clone()?),
        );
        if let Err(error) = result {
            stderr.rewind()?;
            let mut message = String::new();
            stderr.read_to_string(&mut message)?;
            let message = message.split_whitespace().collect::<Vec<_>>().join(" ");
            return Err(if message.is_empty() {
                error
            } else {
                io::Error::other(format!("tmux: {message}"))
            });
        }
    }
    Ok(())
}
pub fn new_window(connection: &Connection, cwd: &Path, agent: Agent) -> io::Result<()> {
    let executable = launch::executable(agent_command(agent))?;
    let mut command = connection.command();
    command.args([
        "new-window",
        "-c",
        &escape_format(
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
        let children = super::children(&processes);
        assert!(descendants(&children, 10, "codex"));
        assert!(!descendants(&children, 10, "claude"));
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
        let mut fastest = std::time::Duration::MAX;
        for _ in 0..3 {
            PROCESS_READS.with(|c| c.set(0));
            let started = std::time::Instant::now();
            let discovery = identify(parsed(&capture), process_table);
            for _ in 0..50 {
                assert!(matching(&session, &discovery.panes).is_none());
            }
            fastest = fastest.min(started.elapsed());
            assert_eq!(PROCESS_READS.with(|c| c.get()), 1);
        }
        assert!(
            fastest < std::time::Duration::from_millis(300),
            "{fastest:?}"
        );
    }

    #[test]
    fn issue52_malformed_records_warn_and_newlines_and_tabs_survive() {
        let result = super::parsed(
            "bad record\x1e\ndev\x1f0\x1f0\x1fclaude\x1f1\x1f/project\\\ttab\\\nnewline\x1e\n",
        );
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(
            result.panes[0].cwd,
            std::path::Path::new("/project\ttab\nnewline")
        );
    }
    #[test]
    fn issue82_newest_eligible_session_gets_pane() {
        use super::*;
        let cwd = tempfile::tempdir().unwrap();
        let panes = parse(&format!(
            "dev\x1f0\x1f0\x1fclaude\x1f1\x1f{}\x1e\n",
            cwd.path().display()
        ));
        let original: Vec<Project> =
            serde_json::from_str(include_str!("../fixtures/list.json")).unwrap();
        for (status, age, older_age, expected) in [
            (Status::Working, 1, 2, Some(0)),
            (Status::Waiting, 1, 2, Some(0)),
            (Status::Done, 1, 2, Some(0)),
            (Status::Idle, 5, 10, Some(1)),
            (Status::Error, 5, 10, Some(1)),
            (Status::Done, 3601, 3602, None),
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
            older.status = Status::Working;
            older.last_activity_at = 10000 - older_age;
            projects[0].sessions.push(older);
            assign(&mut projects, &panes, 10000);
            let assigned: Vec<_> = projects[0]
                .sessions
                .iter()
                .enumerate()
                .filter_map(|(i, s)| s.pane.as_ref().map(|_| i))
                .collect();
            assert_eq!(
                assigned,
                expected.into_iter().collect::<Vec<_>>(),
                "{status:?}, age {age}"
            );
        }
    }
    #[test]
    fn ps_process_names_keep_spaces_in_the_executable_path() {
        assert_eq!(
            super::parse_processes(" 11  10 /Users/test name/bin/codex\n"),
            vec![(11, 10, "codex".into())]
        );
    }
    #[test]
    fn issue79_malformed_fields_and_separator_paths_never_become_panes() {
        for path in ["/project\x1f/victim", "/project\x1e/victim"] {
            let result = super::parsed(&format!(
                "base\x1f1\x1f1\x1fclaude\x1f999999999\x1f{path}\x1e\n"
            ));
            assert!(result.panes.is_empty());
            assert_eq!(result.warnings.len(), 1);
        }
    }
}
