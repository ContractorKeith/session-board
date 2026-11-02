use super::{Source, SourceScan, read, sqlite};
use crate::{
    config::Config,
    model::{Agent, Session, Status},
    project::normalize,
    status::DONE_SECONDS,
};
use std::path::PathBuf;

pub struct Grok {
    pub path: PathBuf,
    pub now: i64,
}

impl Source for Grok {
    fn agent(&self) -> Agent {
        Agent::Grok
    }

    fn scan(&self, config: &Config) -> SourceScan {
        let mut result = SourceScan::default();
        let Some(connection) = sqlite::open(&self.path, &mut result) else {
            return result;
        };
        let mut scan = || -> rusqlite::Result<()> {
            let mut statement = connection.prepare("SELECT session_id, cwd, updated_at, title, content FROM session_docs ORDER BY session_id")?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?;
            for row in rows {
                let Ok((id, cwd, time, title, content)) = row else {
                    result.skipped_sessions += 1;
                    continue;
                };
                let id = read::title(&id);
                let cwd = PathBuf::from(cwd);
                let Some(cwd) = normalize(&cwd, None)
                    .filter(|_| !id.is_empty() && cwd.is_absolute() && time >= 0)
                else {
                    result.skipped_sessions += 1;
                    continue;
                };
                let time = if time > 1_000_000_000_000 {
                    time / 1000
                } else {
                    time
                };
                if self.now.saturating_sub(time) > i64::from(config.window_days) * 86400 {
                    continue;
                }
                let content = content.unwrap_or_default();
                let title = title
                    .map(|s| read::title(&s))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        read::title(content.lines().next().unwrap_or_default())
                            .chars()
                            .take(60)
                            .collect()
                    });
                result.sessions.push(Session {
                    id: format!("grok:{id}"),
                    agent: Agent::Grok,
                    project_root: cwd.clone(),
                    cwd,
                    title,
                    started_at: time,
                    last_activity_at: time,
                    status: if self.now.saturating_sub(time) < DONE_SECONDS {
                        Status::Done
                    } else {
                        Status::Idle
                    },
                    last_user_text: read::preview(&content),
                    last_assistant_text: read::preview(&content),
                    transcript_path: self.path.clone(),
                    resume_hint: format!("grok --resume {id}"),
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
