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
    let mut groups: Vec<_> = projects
        .iter()
        .filter_map(|project| {
            let mut sessions: Vec<_> = project
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
                .collect();
            sessions.sort_by(|a, b| {
                rank(a)
                    .cmp(&rank(b))
                    .then_with(|| b.last_activity_at.cmp(&a.last_activity_at))
                    .then_with(|| a.id.cmp(&b.id))
                    .then_with(|| a.transcript_path.cmp(&b.transcript_path))
            });
            (!sessions.is_empty()).then_some((project, sessions))
        })
        .collect();
    groups.sort_by(|(a, sa), (b, sb)| {
        rank(sa[0])
            .cmp(&rank(sb[0]))
            .then_with(|| sb[0].last_activity_at.cmp(&sa[0].last_activity_at))
            .then_with(|| a.root.cmp(&b.root))
    });
    groups
        .into_iter()
        .flat_map(|(project, sessions)| {
            sessions
                .into_iter()
                .map(move |session| AttentionItem { project, session })
        })
        .collect()
}
