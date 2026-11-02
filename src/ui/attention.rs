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
        let items = self.attention_items();
        let index = index.min(items.len().saturating_sub(1));
        self.attention_path = items
            .get(index)
            .map(|item| item.session.transcript_path.clone());
        self.attention_selected = index;
        if self.attention_focus && !self.attention_collapsed {
            self.sync_attention_tree(false);
        }
    }
    pub fn reconcile_attention(&mut self) {
        let items = self.attention_items();
        if items.is_empty() {
            self.attention_path = None;
            self.attention_selected = 0;
            self.attention_focus = false;
            return;
        }
        let index = items
            .iter()
            .position(|item| Some(&item.session.transcript_path) == self.attention_path.as_ref())
            .unwrap_or(self.attention_selected.min(items.len() - 1));
        self.select_attention(index);
    }
    fn sync_attention_tree(&mut self, unfold: bool) {
        let Some(path) = self.attention_path.clone() else {
            return;
        };
        let Some(root) = self
            .projects
            .iter()
            .find(|p| p.sessions.iter().any(|s| s.transcript_path == path))
            .map(|p| p.root.clone())
        else {
            return;
        };
        if unfold {
            self.folded.remove(&root);
        }
        let rows = self.rows();
        if let Some(index) = rows
            .iter()
            .position(|r| matches!(r, Row::Session(p) if p == &path))
            .or_else(|| {
                rows.iter()
                    .position(|r| matches!(r, Row::Project(p) if p == &root))
            })
        {
            self.selected = index;
        }
    }
    pub fn attention_key(&mut self, code: crossterm::event::KeyCode) -> Option<super::Action> {
        use super::Action;
        use crossterm::event::KeyCode;
        if code == KeyCode::Tab {
            self.attention_focus = !self.attention_focus && !self.attention_items().is_empty();
            if self.attention_focus {
                self.reconcile_attention();
            }
            return Some(Action::None);
        }
        if !self.attention_focus {
            return None;
        }
        self.reconcile_attention();
        if !self.attention_focus {
            return None;
        }
        match code {
            KeyCode::Char('j') | KeyCode::Down if !self.attention_collapsed => {
                self.select_attention(self.attention_selected + 1)
            }
            KeyCode::Char('k') | KeyCode::Up if !self.attention_collapsed => {
                self.select_attention(self.attention_selected.saturating_sub(1))
            }
            KeyCode::Char('h') | KeyCode::Left => self.attention_collapsed = true,
            KeyCode::Char('l') | KeyCode::Right => self.attention_collapsed = false,
            KeyCode::Enter => {
                self.sync_attention_tree(true);
                return None;
            }
            KeyCode::Char('m' | 'r' | 'n' | 'w') => return None,
            _ => {}
        }
        Some(Action::None)
    }
}

pub fn height(count: usize, collapsed: bool, terminal_height: u16) -> u16 {
    if count == 0 {
        0
    } else if collapsed {
        3
    } else {
        ((count + 2).min(10) as u16).min((terminal_height / 3).max(3))
    }
}
pub fn render(app: &App, items: &[AttentionItem<'_>], frame: &mut Frame, area: Rect, now: i64) {
    let selected = items
        .iter()
        .position(|i| Some(&i.session.transcript_path) == app.attention_path.as_ref())
        .unwrap_or(app.attention_selected.min(items.len().saturating_sub(1)));
    let capacity = usize::from(area.height.saturating_sub(2)).min(8);
    let (start, visible) = if items.len() <= capacity {
        (0, items.len())
    } else if capacity < 3 {
        (selected, capacity.min(1))
    } else {
        let start = selected
            .saturating_sub(capacity - 3)
            .min(items.len().saturating_sub(capacity - 1));
        let space = capacity - usize::from(start > 0);
        let visible = space - usize::from(items.len() - start > space);
        (start, visible)
    };
    let above = start;
    let below = items.len().saturating_sub(start + visible);
    let mut title = if app.attention_focus {
        "Attention [focused]"
    } else {
        "Attention"
    }
    .to_owned();
    if capacity < 3 && !app.attention_collapsed {
        if above > 0 {
            title.push_str(&format!(" · +{above} above"));
        }
        if below > 0 {
            title.push_str(&format!(" · +{below} below"));
        }
    }
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
    let mut row = 0;
    if above > 0 && capacity >= 3 {
        frame.render_widget(
            Paragraph::new(format!("+{above} above")),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        row += 1;
    }
    for (offset, item) in items.iter().skip(start).take(visible).enumerate() {
        let (glyph, status, _) = tree::status(item.session.status);
        let agent = match item.session.agent {
            session_board::model::Agent::ClaudeCode => "claude_code",
            session_board::model::Agent::Codex => "codex",
            session_board::model::Agent::Grok => "grok",
            session_board::model::Agent::Opencode => "opencode",
        };
        let line = format!(
            "{glyph} {status:<7} {agent:<11} {} · {} · {}",
            session_board::source::title(&item.project.name),
            item.session.title,
            tree::age(now, item.session.last_activity_at)
        );
        let style = if app.attention_focus && start + offset == selected {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(inner.x, inner.y + row, inner.width, 1),
        );
        row += 1;
    }
    if below > 0 && capacity >= 3 {
        frame.render_widget(
            Paragraph::new(format!("+{below} below")),
            Rect::new(inner.x, inner.y + row, inner.width, 1),
        );
    }
}
