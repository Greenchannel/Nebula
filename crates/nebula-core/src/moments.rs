//! Nebula Moments aggregation.
//!
//! The wire/storage model is an append-only event log (POST / LIKE / COMMENT).
//! This module folds that log into the view the UI consumes.

use crate::db::MomentEvent;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
pub struct MomentView {
    pub moment_id: String,
    pub post: MomentEvent,
    pub blob_ids: Vec<String>,
    pub media_meta: Value,
    pub likes: Vec<MomentEvent>,
    pub comments: Vec<MomentEvent>,
    pub like_count: usize,
    pub comment_count: usize,
    pub liked_by_me: bool,
}

/// Fold a chronological event log into a newest-first list of moment cards.
pub fn aggregate(events: &[MomentEvent], self_id: &str) -> Vec<MomentView> {
    let mut posts: Vec<&MomentEvent> = Vec::new();
    let mut likes: HashMap<String, Vec<MomentEvent>> = HashMap::new();
    let mut comments: HashMap<String, Vec<MomentEvent>> = HashMap::new();

    for event in events {
        match event.event_type.as_str() {
            "POST" => posts.push(event),
            "LIKE" => likes
                .entry(event.moment_id.clone())
                .or_default()
                .push(event.clone()),
            "COMMENT" => comments
                .entry(event.moment_id.clone())
                .or_default()
                .push(event.clone()),
            _ => {}
        }
    }

    let mut views: Vec<MomentView> = posts
        .into_iter()
        .map(|post| {
            let like_list = likes.remove(&post.event_id).unwrap_or_default();
            let mut comment_list = comments.remove(&post.event_id).unwrap_or_default();
            comment_list.sort_by_key(|e| (e.created_at, e.lamport_ts));
            let blob_ids: Vec<String> =
                serde_json::from_str(&post.blob_ids_json).unwrap_or_default();
            let media_meta: Value =
                serde_json::from_str(&post.media_meta_json).unwrap_or(Value::Object(Default::default()));
            let liked_by_me = like_list.iter().any(|e| e.author_id == self_id);
            MomentView {
                moment_id: post.event_id.clone(),
                post: post.clone(),
                blob_ids,
                media_meta,
                like_count: like_list.len(),
                comment_count: comment_list.len(),
                likes: like_list,
                comments: comment_list,
                liked_by_me,
            }
        })
        .collect();

    views.sort_by(|a, b| {
        b.post
            .created_at
            .cmp(&a.post.created_at)
            .then(b.post.lamport_ts.cmp(&a.post.lamport_ts))
    });
    views
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str, moment: &str, author: &str, kind: &str, ts: i64) -> MomentEvent {
        MomentEvent {
            event_id: id.into(),
            moment_id: moment.into(),
            author_id: author.into(),
            author_name: author.into(),
            event_type: kind.into(),
            content: String::new(),
            blob_ids_json: "[]".into(),
            media_meta_json: "{}".into(),
            lamport_ts: ts,
            created_at: ts,
        }
    }

    #[test]
    fn folds_posts_likes_comments() {
        let events = vec![
            ev("p1", "p1", "alice", "POST", 10),
            ev("p2", "p2", "bob", "POST", 20),
            ev("l1", "p1", "bob", "LIKE", 21),
            ev("c1", "p1", "bob", "COMMENT", 22),
            ev("l2", "p1", "alice", "LIKE", 23),
        ];
        let views = aggregate(&events, "alice");
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].moment_id, "p2");
        assert_eq!(views[1].moment_id, "p1");
        assert_eq!(views[1].like_count, 2);
        assert_eq!(views[1].comment_count, 1);
        assert!(views[1].liked_by_me);
    }
}
