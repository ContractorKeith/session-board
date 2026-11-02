use super::App;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
};
use session_board::model::{Agent, Status};

pub fn status(status: Status) -> (&'static str, &'static str, Color) {
    match status {
        Status::Working => ("●", "working", Color::Green),
        Status::Waiting => ("◐", "waiting", Color::Yellow),
        Status::Idle => ("○", "idle", Color::Reset),
        Status::Error => ("✕", "error", Color::Red),
        Status::Done => ("✓", "done", Color::Cyan),
    }
}
fn agent(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "claude_code",
        Agent::Codex => "codex",
        Agent::Grok => "grok",
        Agent::Opencode => "opencode",
    }
}
pub fn age(now: i64, then: i64) -> String {
    let seconds = now.saturating_sub(then).max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86400)
    }
}
fn width(text: &str) -> usize {
    Line::from(text).width()
}
fn truncate(text: &str, available: usize) -> String {
    let mut result = String::new();
    for c in text.chars() {
        if width(&result) + width(&c.to_string()) > available {
            break;
        }
        result.push(c);
    }
    result
}
fn tags_suffix(base: String, tags: &[String], available: usize) -> String {
    if tags.is_empty() {
        return base;
    }
    let tags = format!(" #{}", tags.join(" #"));
    let remaining = available.saturating_sub(width(&base));
    format!("{base}{}", truncate(&tags, remaining))
}
pub fn render(app: &App, frame: &mut Frame, area: Rect, now: i64) {
    let mut items = vec![];
    let available = usize::from(area.width.saturating_sub(4));
    for project in &app.projects {
        let folded = app.folded.contains(&project.root);
        let label = format!(
            "{} {} ({}){}{}{}",
            if folded { "▸" } else { "▾" },
            project.name,
            project.sessions.len(),
            if project.unread_count > 0 {
                format!(" •{}", project.unread_count)
            } else {
                String::new()
            },
            if project.archived { " [archived]" } else { "" },
            if project.pinned { " [pinned]" } else { "" }
        );
        let muted = project.archived || app.organize.missing_projects.contains(&project.root);
        let style = if app.color && muted {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };
        items.push(ListItem::new(tags_suffix(label, &project.tags, available)).style(style));
        if !folded {
            for session in &project.sessions {
                let (glyph, _, color) = status(session.status);
                let archived = app.state.session_archived(session);
                let muted = archived
                    || app
                        .organize
                        .missing_projects
                        .contains(&session.project_root);
                let style = if app.color && muted {
                    Style::default().fg(Color::DarkGray)
                } else if app.color {
                    Style::default().fg(color)
                } else {
                    Style::default()
                };
                let prefix = format!(
                    "  {glyph} {:<11} {:>3}  ",
                    agent(session.agent),
                    age(now, session.last_activity_at)
                );
                let suffix = format!(
                    "{}{}{}",
                    if app.state.unread(session) {
                        " •"
                    } else {
                        ""
                    },
                    if archived { " [archived]" } else { "" },
                    if app.state.group == session_board::scan::Group::Tag
                        && app.state.pins.contains(&session.project_root)
                    {
                        " [pinned]"
                    } else {
                        ""
                    }
                );
                let title = truncate(
                    &session.title,
                    available.saturating_sub(width(&prefix) + width(&suffix)),
                );
                items.push(
                    ListItem::new(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(glyph, style),
                        Span::raw(format!(
                            " {:<11} {:>3}  {title}{suffix}",
                            agent(session.agent),
                            age(now, session.last_activity_at)
                        )),
                    ]))
                    .style(if muted && app.color {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        Style::default()
                    }),
                );
            }
        }
    }
    if app.rows().is_empty() {
        items.push(ListItem::new("No sessions"));
    }
    let title = format!(
        "Projects · {}{}{}",
        if app.state.sort == session_board::scan::Sort::Attention {
            "attention"
        } else {
            "activity"
        },
        if app.state.group == session_board::scan::Group::Tag {
            " · by tag"
        } else {
            ""
        },
        if app.state.show_archived {
            " [archived shown]"
        } else {
            ""
        }
    );
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_symbol("› ")
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    app.page_size
        .set(usize::from(area.height.saturating_sub(2)).max(1));
    let mut state = app.list.borrow_mut();
    state.select((!app.rows().is_empty()).then_some(app.selected));
    frame.render_stateful_widget(list, area, &mut *state);
}

#[cfg(test)]
mod tests {
    use super::age;
    #[test]
    fn compact_ages_and_clock_skew() {
        for (seconds, expected) in [
            (0, "0s"),
            (180, "3m"),
            (7200, "2h"),
            (432000, "5d"),
            (-60, "0s"),
        ] {
            assert_eq!(age(1_000_000, 1_000_000 - seconds), expected);
        }
    }
}
