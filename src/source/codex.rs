use super::{Source, SourceScan, read};
use crate::{
    config::Config,
    model::{Agent, Session, Status},
    project::normalize,
    status::{self, Tail},
};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

pub struct Codex {
    pub root: PathBuf,
    pub index: PathBuf,
    pub now: i64,
}
impl Source for Codex {
    fn agent(&self) -> Agent {
        Agent::Codex
    }
    fn scan(&self, config: &Config) -> SourceScan {
        let mut result = SourceScan::default();
        let titles = titles(&self.index, &mut result);
        for path in read::files(&self.root, true, false, &mut result) {
            match session(&path, self.now, &titles, &mut result) {
                Some(s)
                    if self.now - s.last_activity_at <= i64::from(config.window_days) * 86400 =>
                {
                    result.sessions.push(s)
                }
                Some(_) => (),
                None => result.skipped_sessions += 1,
            }
        }
        result
    }
}
fn titles(path: &Path, result: &mut SourceScan) -> HashMap<String, String> {
    let mut titles = HashMap::new();
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                result.warnings.push(format!("{}: {e}", path.display()));
            }
            return titles;
        }
    };
    for line in BufReader::new(file).split(b'\n') {
        let Ok(line) = line else {
            result.skipped_lines += 1;
            continue;
        };
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<serde_json::Value>(&line) {
            Ok(v) if v["id"].is_string() && v["thread_name"].is_string() => {
                titles.insert(read::string(&v, "id"), read::string(&v, "thread_name"));
            }
            _ => result.skipped_lines += 1,
        }
    }
    titles
}
fn session(
    path: &Path,
    now: i64,
    titles: &HashMap<String, String>,
    result: &mut SourceScan,
) -> Option<Session> {
    let (records, boundary) = read::records(path, result)
        .map_err(|e| result.warnings.push(format!("{}: {e}", path.display())))
        .ok()?;
    let mut id = String::new();
    let mut cwd = PathBuf::new();
    let mut first_user = None;
    let mut user = String::new();
    let mut assistant = String::new();
    let mut started = None;
    let mut last = None;
    let mut tail = Tail::default();
    let mut task_active = false;
    let mut calls = HashSet::new();
    for (index, v) in records.into_iter().enumerate() {
        // The omitted middle may contain completions; head status is not tail evidence.
        if boundary == Some(index) {
            tail = Tail::default();
            calls.clear();
            task_active = false;
        }

        let p = &v["payload"];
        let kind = v["type"].as_str().unwrap_or_default();
        if kind == "session_meta" {
            if p["thread_source"] != "user" {
                return None;
            }
            id = read::string(p, "id");
            cwd = read::string(p, "cwd").into();
        }
        if kind == "turn_context" {
            if let Some(value) = p["cwd"].as_str() {
                cwd = value.into();
            }
            continue;
        }
        if kind == "response_item"
            && p["type"] == "message"
            && !matches!(p["role"].as_str(), Some("user" | "assistant"))
        {
            continue;
        }
        let meaningful = kind == "session_meta"
            || (kind == "event_msg"
                && matches!(
                    p["type"].as_str(),
                    Some("task_started" | "task_complete" | "item_completed" | "error")
                ))
            || (kind == "response_item"
                && matches!(
                    p["type"].as_str(),
                    Some(
                        "message"
                            | "custom_tool_call"
                            | "custom_tool_call_output"
                            | "function_call"
                            | "function_call_output"
                    )
                ));
        if !meaningful {
            continue;
        }
        let Some(time) = read::timestamp(&v["timestamp"]) else {
            result.skipped_lines += 1;
            continue;
        };
        started.get_or_insert(time);
        last = Some(time);
        match (kind, p["type"].as_str().unwrap_or_default()) {
            ("event_msg", "task_started") => {
                task_active = true;
                tail.pending = true;
                tail.ended = false;
            }
            ("event_msg", "task_complete") => {
                task_active = false;
                calls.clear();
                tail.pending = false;
                tail.ended = true;
                tail.text = p["last_agent_message"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&assistant)
                    .to_owned();
                if !tail.text.is_empty() {
                    assistant = tail.text.clone();
                }
            }
            ("event_msg", "error") => tail.explicit_error = true,
            ("event_msg", "item_completed") if p["item"]["type"] == "UserMessage" => {
                user = p["item"]["text"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| read::text(&p["item"]["content"]));
                if !user.is_empty() {
                    first_user.get_or_insert_with(|| user.clone());
                }
            }
            ("response_item", "message") => {
                let text = read::text(&p["content"]);
                match p["role"].as_str() {
                    Some("user") => {
                        first_user.get_or_insert_with(|| text.clone());
                        user = text;
                    }
                    Some("assistant") if !text.is_empty() => {
                        assistant = text;
                    }
                    _ => (),
                }
            }
            ("response_item", "custom_tool_call" | "function_call") => {
                calls.insert(read::string(p, "call_id"));
                tail.pending = true;
                tail.ended = false;
            }
            ("response_item", "custom_tool_call_output" | "function_call_output") => {
                calls.remove(&read::string(p, "call_id"));
                tail.pending = task_active || !calls.is_empty();
                let output = read::string(p, "output");
                tail.explicit_error |= output.lines().any(|l| {
                    l.strip_prefix("Process exited with code ")
                        .is_some_and(|c| c.trim().parse::<i32>().is_ok_and(|code| code != 0))
                });
            }
            _ => (),
        }
    }
    if id.is_empty() || !cwd.is_absolute() {
        return None;
    }
    let cwd = normalize(&cwd, None)?;
    let last = last?;
    let status = status::derive(&tail, last, now);
    if tail.pending && status == Status::Error {
        result.warnings.push(format!(
            "codex:{id}: pending task or tool call older than 10 minutes"
        ));
    }
    Some(Session {
        id: format!("codex:{id}"),
        agent: Agent::Codex,
        project_root: cwd.clone(),
        cwd,
        title: titles
            .get(&id)
            .cloned()
            .unwrap_or_else(|| first_user.unwrap_or_default().chars().take(60).collect()),
        started_at: started?,
        last_activity_at: last,
        status,
        last_user_text: user,
        last_assistant_text: assistant,
        transcript_path: path.into(),
        resume_hint: format!("codex resume {id}"),
        pane: None,
    })
}
