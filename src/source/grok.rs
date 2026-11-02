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
        sqlite::scan(&self.path, &mut result, |connection, result| {
            let cutoff = self
                .now
                .saturating_sub(i64::from(config.window_days) * 86400);
            let mut statement = connection.prepare("SELECT session_id, cwd, updated_at, title, substr(CAST(content AS BLOB), -8192), CAST(substr(ltrim(content, ?3), 1, 8192) AS BLOB) FROM session_docs WHERE typeof(updated_at) != 'integer' OR (updated_at < 1000000000000 AND updated_at >= ?1) OR updated_at >= max(1000000000000, ?2)")?;
            let whitespace = "\t\n\r \u{b}\u{c}\u{85}\u{a0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}";
            let rows = statement.query_map(
                rusqlite::params![cutoff, cutoff.saturating_mul(1000), whitespace],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<Vec<u8>>>(4)?,
                        row.get::<_, Option<Vec<u8>>>(5)?,
                    ))
                },
            )?;
            for row in rows {
                let Some((id, cwd, time, title, content, head)) =
                    sqlite::row(row, &mut result.skipped_sessions)?
                else {
                    continue;
                };
                if !sqlite::valid_id(&id) || cwd.contains('\0') {
                    result.skipped_sessions += 1;
                    continue;
                }
                let cwd = PathBuf::from(cwd);
                let Some(cwd) = normalize(&cwd, None).filter(|_| time >= 0) else {
                    result.skipped_sessions += 1;
                    continue;
                };
                let mut time = if time >= 1_000_000_000_000 {
                    time / 1000
                } else {
                    time
                };
                if time > self.now.saturating_add(3600) {
                    result.warnings.push(format!(
                        "{} grok:{id}: timestamp more than one hour in the future; clamped to now",
                        self.path.display()
                    ));
                    time = self.now;
                }
                let bytes = content.unwrap_or_default();
                let content = String::from_utf8_lossy(&bytes);
                let title = title
                    .map(|s| read::title(&s))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        String::from_utf8_lossy(&head.unwrap_or_default())
                            .lines()
                            .map(read::title)
                            .find(|s| !s.is_empty())
                            .map(|s| s.chars().take(60).collect())
                            .unwrap_or_else(|| id.clone())
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
                    last_user_text: String::new(),
                    last_assistant_text: read::preview(&content),
                    transcript_path: self.path.clone(),
                    resume_hint: format!("grok --resume={id}"),
                    pane: None,
                });
            }
            Ok(())
        });
        result
    }
}
