use super::{Source, SourceScan, read};
use crate::{
    config::Config,
    model::{Agent, Session},
    project::normalize,
    status::{self, Tail},
};
use std::path::{Path, PathBuf};

pub struct Claude {
    pub root: PathBuf,
    pub now: i64,
}
impl Source for Claude {
    fn agent(&self) -> Agent {
        Agent::ClaudeCode
    }
    fn scan(&self, config: &Config) -> SourceScan {
        let mut result = SourceScan::default();
        for path in read::files(&self.root, true, true, &mut result) {
            match session(&path, self.now, &mut result) {
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
fn session(path: &Path, now: i64, result: &mut SourceScan) -> Option<Session> {
    let (records, boundary) = read::records(path, result)
        .map_err(|e| result.warnings.push(format!("{}: {e}", path.display())))
        .ok()?;
    let mut id = String::new();
    let mut cwd = None;
    let mut title = String::new();
    let mut first_user = None;
    let mut user = String::new();
    let mut assistant = String::new();
    let mut started = None;
    let mut last = None;
    let mut tail = Tail::default();
    for (index, v) in records.into_iter().enumerate() {
        // The omitted middle may contain completions; head status is not tail evidence.
        if boundary == Some(index) {
            tail = Tail::default();
        }

        if v["isSidechain"] == true {
            continue;
        }
        let kind = v["type"].as_str().unwrap_or_default();
        if kind == "ai-title" {
            title = v["aiTitle"]
                .as_str()
                .or_else(|| v["title"].as_str())
                .unwrap_or_default()
                .to_owned();
            continue;
        }
        if !matches!(kind, "user" | "assistant") {
            continue;
        }
        let Some(time) = read::timestamp(&v["timestamp"]) else {
            result.skipped_lines += 1;
            continue;
        };
        if id.is_empty()
            && let Some(native) = v["sessionId"].as_str()
        {
            id = read::title(native);
            if id.is_empty() {
                return None;
            }
        }
        if cwd.is_none() {
            cwd = v["cwd"].as_str().map(PathBuf::from);
        }
        if v["isMeta"] == true {
            continue;
        }
        started.get_or_insert(time);
        last = Some(time);
        let message = &v["message"];
        let text = read::text(&message["content"]);
        tail.explicit_error =
            v["isApiErrorMessage"] == true || v.get("error").is_some_and(|e| !e.is_null());
        let tool = message["content"]
            .as_array()
            .is_some_and(|parts| parts.iter().any(|p| p["type"] == "tool_use"));
        let tool_result = message["content"]
            .as_array()
            .is_some_and(|parts| parts.iter().any(|p| p["type"] == "tool_result"));
        if kind == "assistant" {
            if !text.is_empty() {
                assistant = text.clone();
            }
            tail.pending = tool || message["stop_reason"] == "tool_use";
            tail.ended = !tail.pending
                && matches!(
                    message["stop_reason"].as_str(),
                    Some("end_turn" | "max_tokens" | "stop_sequence")
                );
            tail.text = text;
        } else {
            if !tool_result && !text.is_empty() && !text.starts_with('<') {
                first_user.get_or_insert_with(|| text.clone());
                user = text.clone();
            }
            tail.pending = !text.starts_with("[Request interrupted by user");
            tail.ended = false;
            tail.text.clear();
        }
    }
    let cwd = cwd?;
    if id.is_empty() || !cwd.is_absolute() {
        return None;
    }
    let cwd = normalize(&cwd, None)?;
    let last = last?;
    if title.is_empty() {
        title = first_user.unwrap_or_default().chars().take(60).collect();
    }
    let status = status::derive(&tail, last, now);
    if tail.pending && status == crate::model::Status::Error {
        result.status_reasons.push(format!(
            "claude_code:{id}: pending generation or tool call at least 30 minutes old"
        ));
    }
    Some(Session {
        id: format!("claude_code:{id}"),
        agent: Agent::ClaudeCode,
        project_root: cwd.clone(),
        cwd,
        title: read::title(&title),
        started_at: started?,
        last_activity_at: last,
        status,
        last_user_text: read::preview(&user),
        last_assistant_text: read::preview(&assistant),
        transcript_path: path.into(),
        resume_hint: format!("claude --resume {id}"),
        pane: None,
    })
}
