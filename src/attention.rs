//! Shared attention membership and project/session ordering.
use crate::{
    model::{Project, Session, Status},
    state::State,
};

#[derive(Debug)]
pub struct AttentionItem<'a> {
    pub project: &'a Project,
    pub session: &'a Session,
}

pub fn attention<'a>(projects: &'a [Project], state: &State) -> Vec<AttentionItem<'a>> {
    let rank = |s: &Session| match s.status {
        Status::Waiting => 0,
        Status::Error => 1,
        Status::Done => 2,
        _ => 3,
    };
    let mut items: Vec<_> = projects
        .iter()
        .flat_map(|project| {
            project
                .sessions
                .iter()
                .filter(|s| {
                    rank(s) < 2
                        || (s.status == Status::Done
                            && state
                                .read
                                .get(&s.id)
                                .is_none_or(|t| s.last_activity_at > *t))
                })
                .map(move |session| AttentionItem { project, session })
        })
        .collect();
    items.sort_by(|a, b| {
        rank(a.session)
            .cmp(&rank(b.session))
            .then_with(|| b.session.last_activity_at.cmp(&a.session.last_activity_at))
            .then_with(|| a.session.id.cmp(&b.session.id))
            .then_with(|| a.session.transcript_path.cmp(&b.session.transcript_path))
            .then_with(|| a.project.root.cmp(&b.project.root))
    });
    items
}
