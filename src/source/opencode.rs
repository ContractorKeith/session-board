use super::{Source, SourceScan, read, sqlite};
use crate::{
    config::Config,
    model::{Agent, Session, Status},
    project::normalize,
    status::{self, Tail},
};
use rusqlite::Connection;
use serde_json::Value;
use std::path::PathBuf;

pub struct Opencode {
    pub path: PathBuf,
    pub now: i64,
}

impl Source for Opencode {
    fn agent(&self) -> Agent {
        Agent::Opencode
    }

    fn scan(&self, config: &Config) -> SourceScan {
        let mut result = SourceScan::default();
        let Some(connection) = sqlite::open(&self.path, &mut result) else {
            return result;
        };
        let mut scan = || -> rusqlite::Result<()> {
            // Keep one snapshot across session, message and part queries.
            connection.execute_batch("BEGIN")?;
            let mut statement = connection.prepare("SELECT id, directory, title, time_created, time_updated FROM session WHERE parent_id IS NULL ORDER BY id")?;
            let rows = statement.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })?;
            for row in rows {
                let Ok((native_id, cwd, title, started, updated)) = row else {
                    result.skipped_sessions += 1;
                    continue;
                };
                let id = read::title(&native_id);
                let cwd = PathBuf::from(cwd);
                let Some(cwd) = normalize(&cwd, None).filter(|_| {
                    !id.is_empty() && cwd.is_absolute() && started >= 0 && updated >= 0
                }) else {
                    result.skipped_sessions += 1;
                    continue;
                };
                let last = updated / 1000;
                if self.now.saturating_sub(last) > i64::from(config.window_days) * 86400 {
                    continue;
                }
                let (user, assistant, first_user, tail) =
                    match messages(&connection, &native_id, &mut result) {
                        Ok(messages) => messages,
                        Err(error) => {
                            result.skipped_sessions += 1;
                            result
                                .warnings
                                .push(format!("{} session {id}: {error}", self.path.display()));
                            continue;
                        }
                    };
                let status = status::derive(&tail, last, self.now);
                if tail.pending && status == Status::Error {
                    result.status_reasons.push(format!(
                        "opencode:{id}: pending message or tool call at least 30 minutes old"
                    ));
                }
                let title = title
                    .map(|s| read::title(&s))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| read::title(&first_user).chars().take(60).collect());
                result.sessions.push(Session {
                    id: format!("opencode:{id}"),
                    agent: Agent::Opencode,
                    project_root: cwd.clone(),
                    cwd,
                    title,
                    started_at: started / 1000,
                    last_activity_at: last,
                    status,
                    last_user_text: read::preview(&user),
                    last_assistant_text: read::preview(&assistant),
                    transcript_path: self.path.clone(),
                    resume_hint: format!("opencode --session {id}"),
                    pane: None,
                });
            }
            Ok(())
        };
        if let Err(error) = scan() {
            sqlite::failed(&self.path, &error, &mut result);
        }
        result
    }
}

fn json(data: rusqlite::Result<String>, result: &mut SourceScan) -> Option<Value> {
    match data
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .filter(Value::is_object)
    {
        Some(value) => Some(value),
        None => {
            result.skipped_lines += 1;
            None
        }
    }
}

/// Roles and completion belong to message.data; prose and tool state to part.data.
fn messages(
    connection: &Connection,
    id: &str,
    result: &mut SourceScan,
) -> rusqlite::Result<(String, String, String, Tail)> {
    let mut statement = connection
        .prepare("SELECT id, data FROM message WHERE session_id = ?1 ORDER BY time_created, id")?;
    let mut parts = connection.prepare(
        "SELECT data FROM part WHERE message_id = ?1 AND session_id = ?2 ORDER BY time_created, id",
    )?;
    let mut rows = statement.query([id])?;
    let (mut user, mut assistant, mut first_user) = (String::new(), String::new(), None);
    let mut tail = Tail::default();
    while let Some(row) = rows.next()? {
        let Some(message) = json(row.get(1), result) else {
            continue;
        };
        let role = message["role"].as_str().unwrap_or_default();
        if !matches!(role, "user" | "assistant") {
            continue;
        }
        let message_id: String = row.get(0)?;
        let mut text = String::new();
        let mut pending_tool = false;
        let mut tool_error = false;
        for part in parts.query_map([&message_id, id], |row| row.get::<_, String>(0))? {
            let Some(part) = json(part, result) else {
                continue;
            };
            match part["type"].as_str() {
                Some("text") => {
                    if let Some(s) = part["text"].as_str() {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(s);
                    }
                }
                Some("tool") => {
                    pending_tool |= matches!(
                        part["state"]["status"].as_str(),
                        Some("pending" | "running")
                    );
                    tool_error |= part["state"]["status"] == "error";
                }
                _ => (),
            }
        }
        if role == "user" {
            first_user.get_or_insert_with(|| text.clone());
            user = text;
            tail = Tail {
                pending: true,
                ..Default::default()
            };
        } else {
            let error = message.get("error").is_some_and(|e| !e.is_null()) || tool_error;
            let ended = !message["time"]["completed"].is_null()
                || message["finish"]
                    .as_str()
                    .is_some_and(|s| s != "tool-calls" && !s.is_empty());
            tail = Tail {
                pending: pending_tool || (!ended && !error),
                ended: ended && !error,
                text: text.clone(),
                explicit_error: error,
            };
            assistant = text;
        }
    }
    Ok((user, assistant, first_user.unwrap_or_default(), tail))
}
