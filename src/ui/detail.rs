use super::{App, tree};
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, Paragraph},
};

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let text = match app.session() {
        Some(session) => {
            // Reserve room for both transcript tails and the resume hint.
            let lines = usize::from(area.height.saturating_sub(13) / 2).max(1);
            let tail = |text: &str| text.lines().take(lines).collect::<Vec<_>>().join("\n");
            format!(
                "Cwd: {}\nBranch: unknown\nStatus: {} {}\n\nLast user:\n{}\n\nLast assistant:\n{}\n\nResume:\n{}",
                session.cwd.display(),
                tree::status(session.status).0,
                tree::status(session.status).1,
                tail(&session.last_user_text),
                tail(&session.last_assistant_text),
                session.resume_hint
            )
        }
        None => "Select a session".into(),
    };
    // Paragraph clips by display cells, including wide Unicode, without byte slicing.
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Session")),
        area,
    );
}
