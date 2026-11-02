use super::{App, Row, tree};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, Borders, Paragraph},
};
use session_board::attention::AttentionItem;

impl App {
    pub fn attention_items(&self) -> Vec<AttentionItem<'_>> {
        session_board::attention::attention(&self.projects, &self.state)
    }
    pub fn select_attention(&mut self, index: usize) {
        let index = index.min(self.attention_items().len().saturating_sub(1));
        let path = self
            .attention_items()
            .get(index)
            .map(|item| item.session.transcript_path.clone());
        if let Some(path) = path {
            self.attention_selected = index;
            if let Some(project) = self
                .projects
                .iter()
                .find(|p| p.sessions.iter().any(|s| s.transcript_path == path))
            {
                self.folded.remove(&project.root);
            }
            if let Some(index) = self
                .rows()
                .iter()
                .position(|r| r == &Row::Session(path.clone()))
            {
                self.selected = index;
            }
        }
    }
}

pub fn height(app: &App) -> u16 {
    let count = app.attention_items().len();
    if count == 0 {
        0
    } else if app.attention_collapsed {
        3
    } else {
        (count + 2).min(10) as u16
    }
}
pub fn render(app: &App, frame: &mut Frame, area: Rect, now: i64) {
    let items = app.attention_items();
    let title = if app.attention_focus {
        "Attention [focused]"
    } else {
        "Attention"
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.attention_collapsed {
        frame.render_widget(
            Paragraph::new(format!("{} need you · l to expand", items.len())),
            inner,
        );
        return;
    }
    let capacity = usize::from(inner.height).min(8);
    let visible = capacity.saturating_sub(usize::from(items.len() > capacity));
    let start = if app.attention_focus {
        app.attention_selected
            .saturating_sub(visible.saturating_sub(1))
    } else {
        0
    };
    for (row, item) in items.iter().skip(start).take(visible).enumerate() {
        let index = start + row;
        let (glyph, status, _) = tree::status(item.session.status);
        let agent = match item.session.agent {
            session_board::model::Agent::ClaudeCode => "claude_code",
            session_board::model::Agent::Codex => "codex",
            session_board::model::Agent::Grok => "grok",
            session_board::model::Agent::Opencode => "opencode",
        };
        let line = format!(
            "{glyph} {status:<7} {agent:<11} {} · {} · {}",
            item.project.name,
            item.session.title,
            tree::age(now, item.session.last_activity_at)
        );
        let style = if app.attention_focus && index == app.attention_selected {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
        );
    }
    if items.len() > capacity && capacity > 0 {
        frame.render_widget(
            Paragraph::new(format!("+{} more", items.len() - visible)),
            Rect::new(inner.x, inner.y + visible as u16, inner.width, 1),
        );
    }
}
