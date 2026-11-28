use crate::{
    model::{Project, Session},
    state::State,
};
use serde::Serialize;
use serde_json::Value;
use std::{
    borrow::Cow,
    io::{self, Write},
    path::Path,
};

#[derive(Serialize)]
struct SessionRow<'a> {
    #[serde(flatten)]
    session: &'a Session,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<Cow<'a, str>>,
    unread: bool,
    archived: bool,
}

impl<'a> SessionRow<'a> {
    fn new(session: &'a Session, project: &'a Project, state: &State) -> Self {
        let root = project.root.to_string_lossy();
        let group =
            (project.root.starts_with("untagged:") || root.starts_with("tag:")).then_some(root);
        Self {
            session,
            group,
            unread: state.unread(session),
            archived: state.session_archived(session),
        }
    }
}

#[derive(Serialize)]
struct ProjectRow<'a> {
    root: &'a Path,
    name: &'a str,
    sessions: Vec<SessionRow<'a>>,
    pinned: bool,
    tags: &'a [String],
    archived: bool,
    unread_count: usize,
}

#[derive(Serialize)]
struct AttentionRow<'a> {
    #[serde(flatten)]
    session: SessionRow<'a>,
    pinned: bool,
    tags: &'a [String],
}

pub(super) fn list(out: &mut impl Write, projects: &[Project], state: &State) -> io::Result<()> {
    let rows: Vec<_> = projects
        .iter()
        .map(|project| ProjectRow {
            root: &project.root,
            name: &project.name,
            sessions: project
                .sessions
                .iter()
                .map(|session| SessionRow::new(session, project, state))
                .collect(),
            pinned: project.pinned,
            tags: &project.tags,
            archived: project.archived,
            unread_count: project.unread_count,
        })
        .collect();
    write(out, &rows)
}

pub(super) fn attention(
    out: &mut impl Write,
    projects: &[Project],
    state: &State,
) -> io::Result<()> {
    let rows: Vec<_> = crate::attention::attention(projects, state)
        .into_iter()
        .map(|item| AttentionRow {
            session: SessionRow::new(item.session, item.project, state),
            pinned: state.pins.contains(&item.session.project_root),
            tags: state
                .tags
                .get(&item.session.project_root)
                .map(Vec::as_slice)
                .unwrap_or_default(),
        })
        .collect();
    write(out, &rows)
}

fn write(out: &mut impl Write, rows: &impl Serialize) -> io::Result<()> {
    let value = sorted(serde_json::to_value(rows)?);
    writeln!(out, "{}", serde_json::to_string_pretty(&value)?)
}

fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_sorted_whatever_order_serde_produced() {
        let mut map = serde_json::Map::new();
        map.insert("zeta".into(), serde_json::json!([{"b": 1, "a": 2}]));
        map.insert("alpha".into(), Value::Null);
        let mut out = Vec::new();
        write(&mut out, &Value::Object(map)).unwrap();
        let text = String::from_utf8(out).unwrap();
        let order = ["\"alpha\"", "\"zeta\"", "\"a\"", "\"b\""].map(|k| text.find(k).unwrap());
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{text}");
    }
}
