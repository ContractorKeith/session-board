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
    if let Err(error) = crate::storage::regular_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            result.skipped_stores += 1;
            result.warnings.push(format!("{}: {error}", path.display()));
        }
        return titles;
    }
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
                titles.insert(
                    read::title(&read::string(&v, "id")),
                    read::string(&v, "thread_name"),
                );
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
    let mut identity = None;
    let mut first_event_user = None;
    let mut last_event_user = None;
    let mut first_fallback_user = None;
    let mut last_fallback_user = None;
    let mut assistant = String::new();
    let mut turn_text = String::new();
    let mut started = None;
    let mut last = None;
    let mut tail = Tail::default();
    let mut task_active = false;
    let mut calls = HashSet::new();
    for (index, v) in records.into_iter().enumerate() {
        if boundary == Some(index) {
            // Task lifecycle survives omitted history; tail task events override it.
            tail = Tail {
                pending: task_active,
                ..Default::default()
            };
            calls.clear();
        }
        let p = &v["payload"];
        let kind = v["type"].as_str().unwrap_or_default();
        if kind == "session_meta" {
            if identity.is_some() {
                continue;
            }
            let owner = match p.get("thread_source") {
                Some(source) => source == "user",
                None => p["source"].is_string(),
            };
            if !owner || p["source"].is_object() {
                return None;
            }
            identity = Some((
                read::title(&read::string(p, "id")),
                PathBuf::from(read::string(p, "cwd")),
            ));
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
                    Some(
                        "task_started"
                            | "task_complete"
                            | "turn_aborted"
                            | "item_completed"
                            | "user_message"
                            | "error"
                    )
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
                calls.clear();
                turn_text.clear();
                tail = Tail {
                    pending: true,
                    ..Default::default()
                };
            }
            ("event_msg", "task_complete" | "turn_aborted") => {
                task_active = false;
                calls.clear();
                let message = p["last_agent_message"].as_str().filter(|s| !s.is_empty());
                let aborted = p["type"] == "turn_aborted";
                let text = if aborted {
                    message.unwrap_or_default()
                } else {
                    message.unwrap_or(&turn_text)
                };
                tail = Tail {
                    ended: !aborted || message.is_some(),
                    text: text.to_owned(),
                    ..Default::default()
                };
                if !text.is_empty() {
                    assistant = text.to_owned();
                }
            }
            ("event_msg", "error") => {
                task_active = false;
                calls.clear();
                tail = Tail {
                    explicit_error: true,
                    ..Default::default()
                };
            }
            ("event_msg", "user_message") => {
                let text = read::string(p, "message");
                if !text.is_empty() {
                    first_event_user.get_or_insert_with(|| text.clone());
                    last_event_user = Some(text);
                }
            }
            ("event_msg", "item_completed") if p["item"]["type"] == "UserMessage" => {
                let text = p["item"]["text"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| read::text(&p["item"]["content"]));
                if !text.is_empty() {
                    first_event_user.get_or_insert_with(|| text.clone());
                    last_event_user = Some(text);
                }
            }
            ("response_item", "message") => {
                let text = read::text(&p["content"]);
                match p["role"].as_str() {
                    Some("user")
                        if !text.is_empty() && !text.trim_start().starts_with(['<', '#']) =>
                    {
                        first_fallback_user.get_or_insert_with(|| text.clone());
                        last_fallback_user = Some(text);
                    }
                    Some("assistant") if !text.is_empty() => {
                        assistant = text.clone();
                        turn_text = text;
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
            }
            _ => (),
        }
    }
    let (id, cwd) = identity?;
    if id.is_empty() || !cwd.is_absolute() {
        return None;
    }
    let cwd = normalize(&cwd, None)?;
    let last = last?;
    let status = status::derive(&tail, last, now);
    if tail.pending && status == Status::Error {
        result.status_reasons.push(format!(
            "codex:{id}: pending task or tool call at least 30 minutes old"
        ));
    }
    let title = titles
        .get(&id)
        .map(|s| read::title(s))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            read::title(&first_event_user.or(first_fallback_user).unwrap_or_default())
                .chars()
                .take(60)
                .collect()
        });
    Some(Session {
        id: format!("codex:{id}"),
        agent: Agent::Codex,
        project_root: cwd.clone(),
        cwd,
        title,
        started_at: started?,
        last_activity_at: last,
        status,
        last_user_text: read::preview(&last_event_user.or(last_fallback_user).unwrap_or_default()),
        last_assistant_text: read::preview(&assistant),
        transcript_path: path.into(),
        resume_hint: format!("codex resume {id}"),
        pane: None,
    })
}
