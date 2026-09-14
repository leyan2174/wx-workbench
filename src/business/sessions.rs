//! Conversation summaries are not a directory of retained message evidence.
use super::{
    contacts::ContactKind,
    messages::{Error, Kind, Result},
};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct Session {
    pub username: String,
    pub kind: ContactKind,
    pub unread: i64,
    pub timestamp: i64,
    pub last_kind: Kind,
    pub sender: Option<String>,
    pub sender_display_hint: Option<String>,
    pub summary: String,
}
pub struct Query {
    pub limit: usize,
    pub unread_only: bool,
    pub kinds: Vec<ContactKind>,
}

pub fn select(sessions: &[Session], query: &Query) -> Result<Vec<usize>> {
    let mut identities = HashSet::new();
    let mut rows = Vec::new();
    for (index, session) in sessions.iter().enumerate() {
        if !identities.insert(&session.username) {
            return Err(Error::Ambiguous);
        }
        if session.unread < 0 {
            return Err(Error::InvalidData);
        }
        if query.unread_only && session.unread == 0 {
            continue;
        }
        if !query.unread_only && session.timestamp <= 0 {
            continue;
        }
        if !query.kinds.is_empty() && !query.kinds.contains(&session.kind) {
            continue;
        }
        rows.push(index);
    }
    rows.sort_by(|a, b| {
        sessions[*b]
            .timestamp
            .cmp(&sessions[*a].timestamp)
            .then(sessions[*a].username.cmp(&sessions[*b].username))
    });
    rows.truncate(query.limit);
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filtering_precedes_limiting_and_same_time_order_is_total() {
        let make = |name: &str, kind| Session {
            username: name.into(),
            kind,
            unread: 1,
            timestamp: 10,
            last_kind: Kind::Text,
            sender: None,
            sender_display_hint: None,
            summary: String::new(),
        };
        let sessions = vec![
            make("z", ContactKind::Person),
            make("b", ContactKind::Official),
            make("a", ContactKind::Official),
        ];
        assert_eq!(
            select(
                &sessions,
                &Query {
                    limit: 1,
                    unread_only: true,
                    kinds: vec![ContactKind::Official]
                }
            )
            .unwrap(),
            [2]
        );
        assert_eq!(
            select(
                &[sessions[0].clone(), sessions[0].clone()],
                &Query {
                    limit: 1,
                    unread_only: true,
                    kinds: vec![]
                }
            ),
            Err(Error::Ambiguous)
        );
    }
}
