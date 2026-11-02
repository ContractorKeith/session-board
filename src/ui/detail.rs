use super::{App, tree};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

fn tail(text: &str, width: u16, lines: usize) -> String {
    let width = usize::from(width).max(1);
    let mut wrapped = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let line: String = line
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let span = Span::raw(&line);
        let mut current = String::new();
        let mut used = 0;
        for grapheme in span.styled_graphemes(Style::default()) {
            let cells = Line::raw(grapheme.symbol).width();
            if used + cells > width && !current.is_empty() {
                wrapped.push(std::mem::take(&mut current));
                used = 0;
            }
            current.push_str(grapheme.symbol);
            used += cells;
        }
        if !current.is_empty() {
            wrapped.push(current);
        }
    }
    wrapped[wrapped.len().saturating_sub(lines)..].join("\n")
}
pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title("Session");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(session) = app.session() else {
        frame.render_widget(Paragraph::new("Select a session"), inner);
        return;
    };
    // Separate regions stop long prose from hiding the other text or resume hint.
    let regions = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(0),
        Constraint::Min(0),
        Constraint::Length(3),
    ])
    .split(inner);
    let heading = format!(
        "Cwd: {}\nBranch: unknown\nStatus: {} {}",
        session.cwd.display(),
        tree::status(session.status).0,
        tree::status(session.status).1
    );
    frame.render_widget(
        Paragraph::new(heading).wrap(Wrap { trim: false }),
        regions[0],
    );
    for (label, text, region) in [
        ("Last user:", session.last_user_text.as_str(), regions[1]),
        (
            "Last assistant:",
            session.last_assistant_text.as_str(),
            regions[2],
        ),
    ] {
        let lines = usize::from(region.height.saturating_sub(1)).max(1);
        let text = tail(text, region.width, lines);
        frame.render_widget(
            Paragraph::new(label),
            Rect {
                height: region.height.min(1),
                ..region
            },
        );
        let content = Rect {
            y: region.y.saturating_add(1),
            height: region.height.saturating_sub(1),
            ..region
        };
        let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, content);
    }
    frame.render_widget(
        Paragraph::new(format!("Resume:\n{}", session.resume_hint)).wrap(Wrap { trim: false }),
        regions[3],
    );
}
