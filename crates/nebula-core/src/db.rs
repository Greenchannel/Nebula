//! SQLite storage layer.
//!
//! `rusqlite::Connection` is `Send` but **not** `Sync`, so it can never be put
//! directly inside an Axum `State`. It is wrapped in a `std::sync::Mutex`
//! (see [`DbPool`]) and only ever locked for microsecond-scale statements —
//! no `.await` is ever performed while the lock is held.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub const AVATAR_PALETTE: &[&str] = &[
    "#6366f1", "#8b5cf6", "#06b6d4", "#10b981", "#f59e0b", "#f43f5e", "#3b82f6", "#a855f7",
    "#14b8a6", "#ec4899",
];

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Deterministic globally unique id: `{lamport:016}-{peer_id}-{rand:08x}`.
pub fn make_gid(lamport: i64, peer_id: &str) -> String {
    format!("{:016}-{}-{:08x}", lamport.max(0), peer_id, rand::random::<u32>())
}

/// Canonical 1v1 conversation id: lexicographically sorted `a:b`.
pub fn conversation_id(a: &str, b: &str) -> String {
    if a <= b {
        format!("{a}:{b}")
    } else {
        format!("{b}:{a}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub peer_id: String,
    pub nickname: String,
    pub bio: String,
    pub avatar_color: String,
    pub lamport_clock: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRecord {
    pub peer_id: String,
    pub nickname: String,
    pub bio: String,
    pub avatar_color: String,
    pub last_ip: String,
    pub last_p2p_port: i64,
    pub status: String,
    pub moments_head_clock: i64,
    pub last_seen_at: i64,
}

/// Inbound ANNOUNCE payload (the discovery envelope has more fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerAnnounce {
    pub peer_id: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub bio: String,
    #[serde(default)]
    pub avatar_color: String,
    pub p2p_port: u16,
    #[serde(default)]
    pub moments_head_clock: i64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PeerTouch {
    /// Peer transitioned from unknown/offline to online.
    pub became_online: bool,
    /// Peer advertised a moments head clock we did not know about.
    pub head_clock_advanced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub msg_id: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub receiver_id: String,
    pub msg_type: String,
    pub content: String,
    #[serde(default)]
    pub blob_id: Option<String>,
    #[serde(default)]
    pub media_meta_json: Option<String>,
    pub lamport_ts: i64,
    pub created_at: i64,
    pub delivery_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobRecord {
    pub blob_id: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub is_present: bool,
    pub source_peer_id: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MomentEvent {
    pub event_id: String,
    pub moment_id: String,
    pub author_id: String,
    pub author_name: String,
    pub event_type: String,
    #[serde(default)]
    pub content: String,
    #[serde(default = "empty_array_json")]
    pub blob_ids_json: String,
    #[serde(default = "empty_object_json")]
    pub media_meta_json: String,
    pub lamport_ts: i64,
    pub created_at: i64,
}

fn empty_array_json() -> String {
    "[]".to_string()
}
fn empty_object_json() -> String {
    "{}".to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationSummary {
    pub conversation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message: Option<MessageRecord>,
    pub last_at: i64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct BlobStats {
    pub count: i64,
    pub bytes: i64,
}

/// Thread-safe SQLite handle shared across the node.
#[derive(Clone)]
pub struct DbPool {
    conn: Arc<Mutex<Connection>>,
    path: Arc<std::path::PathBuf>,
}

impl DbPool {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating database dir {}", parent.display()))?;
        }
        let conn = Connection::open(&path)
            .with_context(|| format!("opening database {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA foreign_keys = ON;",
        )
        .context("applying sqlite pragmas")?;
        let pool = Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Arc::new(path),
        };
        pool.init_schema()?;
        Ok(pool)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run a closure while holding the connection lock. Never `.await` inside.
    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let guard = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("nebula database mutex poisoned"))?;
        f(&guard)
    }

    fn init_schema(&self) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute_batch(
                r#"
CREATE TABLE IF NOT EXISTS identity (
    peer_id TEXT PRIMARY KEY,
    nickname TEXT NOT NULL,
    bio TEXT NOT NULL DEFAULT 'Exploring the Nebula LAN...',
    avatar_color TEXT NOT NULL DEFAULT '#6366f1',
    lamport_clock INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS peers (
    peer_id TEXT PRIMARY KEY,
    nickname TEXT NOT NULL,
    bio TEXT NOT NULL DEFAULT '',
    avatar_color TEXT NOT NULL DEFAULT '#6366f1',
    last_ip TEXT NOT NULL,
    last_p2p_port INTEGER NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('online', 'offline')),
    moments_head_clock INTEGER NOT NULL DEFAULT 0,
    last_seen_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS blobs (
    blob_id TEXT PRIMARY KEY,
    mime_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    is_present INTEGER NOT NULL DEFAULT 0,
    source_peer_id TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    msg_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    sender_id TEXT NOT NULL,
    sender_name TEXT NOT NULL,
    receiver_id TEXT NOT NULL,
    msg_type TEXT NOT NULL CHECK(msg_type IN ('text', 'image', 'voice')),
    content TEXT NOT NULL,
    blob_id TEXT,
    media_meta_json TEXT,
    lamport_ts INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    delivery_status TEXT NOT NULL CHECK(delivery_status IN ('queued', 'sent', 'delivered'))
);
CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conversation_id, lamport_ts ASC, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_messages_queued ON messages(receiver_id, delivery_status) WHERE delivery_status = 'queued';

CREATE TABLE IF NOT EXISTS moment_events (
    event_id TEXT PRIMARY KEY,
    moment_id TEXT NOT NULL,
    author_id TEXT NOT NULL,
    author_name TEXT NOT NULL,
    event_type TEXT NOT NULL CHECK(event_type IN ('POST', 'LIKE', 'COMMENT')),
    content TEXT NOT NULL DEFAULT '',
    blob_ids_json TEXT NOT NULL DEFAULT '[]',
    media_meta_json TEXT NOT NULL DEFAULT '{}',
    lamport_ts INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_moment_events_author ON moment_events(author_id, lamport_ts ASC);
CREATE INDEX IF NOT EXISTS idx_moment_events_target ON moment_events(moment_id, created_at ASC);
"#,
            )
            .context("initializing nebula schema")?;
            Ok(())
        })
    }

    // ------------------------------------------------------------------
    // Identity
    // ------------------------------------------------------------------

    /// Return the persisted identity, creating it on first boot.
    pub fn ensure_identity(&self, nickname: Option<&str>, bio: &str) -> Result<Identity> {
        self.with_conn(|conn| {
            let existing = conn
                .query_row(
                    "SELECT peer_id, nickname, bio, avatar_color, lamport_clock, created_at \
                     FROM identity LIMIT 1",
                    [],
                    map_identity,
                )
                .optional()?;
            if let Some(identity) = existing {
                return Ok(identity);
            }
            let peer_id = format!("{:016x}", rand::random::<u64>());
            let seed = u8::from_str_radix(&peer_id[0..2], 16).unwrap_or(0) as usize;
            let avatar_color = AVATAR_PALETTE[seed % AVATAR_PALETTE.len()].to_string();
            let nick = nickname
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("Nebula-{}", &peer_id[0..4].to_uppercase()));
            let bio = if bio.trim().is_empty() {
                "Exploring the Nebula LAN..."
            } else {
                bio
            };
            let created = now_ms();
            conn.execute(
                "INSERT INTO identity (peer_id, nickname, bio, avatar_color, lamport_clock, created_at) \
                 VALUES (?1, ?2, ?3, ?4, 1, ?5)",
                params![peer_id, nick, bio, avatar_color, created],
            )?;
            Ok(Identity {
                peer_id,
                nickname: nick,
                bio: bio.to_string(),
                avatar_color,
                lamport_clock: 1,
                created_at: created,
            })
        })
    }

    pub fn identity(&self) -> Result<Identity> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT peer_id, nickname, bio, avatar_color, lamport_clock, created_at \
                 FROM identity LIMIT 1",
                [],
                map_identity,
            )
            .context("identity row missing")
        })
    }

    pub fn update_profile(&self, nickname: &str, bio: &str) -> Result<Identity> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE identity SET nickname = ?1, bio = ?2",
                params![nickname, bio],
            )?;
            conn.query_row(
                "SELECT peer_id, nickname, bio, avatar_color, lamport_clock, created_at \
                 FROM identity LIMIT 1",
                [],
                map_identity,
            )
            .context("identity row missing")
        })
    }

    /// Increment and return the local Lamport clock.
    pub fn increment_lamport(&self) -> Result<i64> {
        self.with_conn(|conn| {
            conn.execute("UPDATE identity SET lamport_clock = lamport_clock + 1", [])?;
            let clock = conn.query_row(
                "SELECT lamport_clock FROM identity LIMIT 1",
                [],
                |row| row.get::<_, i64>(0),
            )?;
            Ok(clock)
        })
    }

    /// Merge a remote Lamport timestamp into the local clock.
    pub fn observe_lamport(&self, remote: i64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE identity SET lamport_clock = MAX(lamport_clock, ?1)",
                params![remote],
            )?;
            Ok(())
        })
    }

    // ------------------------------------------------------------------
    // Peers
    // ------------------------------------------------------------------

    pub fn touch_peer(&self, ann: &PeerAnnounce, src_ip: &str, now: i64) -> Result<PeerTouch> {
        self.with_conn(|conn| {
            let existing: Option<(String, String, i64)> = conn
                .query_row(
                    "SELECT status, last_ip, moments_head_clock FROM peers WHERE peer_id = ?1",
                    params![ann.peer_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;

            let mut touch = PeerTouch::default();
            let nickname = if ann.nickname.trim().is_empty() {
                format!("Nebula-{}", acc_short(&ann.peer_id))
            } else {
                ann.nickname.clone()
            };
            let avatar = if ann.avatar_color.trim().is_empty() {
                "#6366f1".to_string()
            } else {
                ann.avatar_color.clone()
            };

            match existing {
                Some((status, last_ip, head)) => {
                    touch.became_online = status != "online";
                    touch.head_clock_advanced = ann.moments_head_clock > head;
                    // Prefer a routable LAN address over 127.0.0.1.
                    let ip = if is_loopback_ip(src_ip) && !is_loopback_ip(&last_ip) {
                        last_ip.clone()
                    } else {
                        src_ip.to_string()
                    };
                    conn.execute(
                        "UPDATE peers SET nickname=?2, bio=?3, avatar_color=?4, last_ip=?5, \
                         last_p2p_port=?6, status='online', moments_head_clock=?7, last_seen_at=?8 \
                         WHERE peer_id=?1",
                        params![
                            ann.peer_id,
                            nickname,
                            ann.bio,
                            avatar,
                            ip,
                            ann.p2p_port as i64,
                            ann.moments_head_clock.max(head),
                            now
                        ],
                    )?;
                }
                None => {
                    touch.became_online = true;
                    touch.head_clock_advanced = ann.moments_head_clock > 0;
                    conn.execute(
                        "INSERT OR IGNORE INTO peers (peer_id, nickname, bio, avatar_color, last_ip, \
                         last_p2p_port, status, moments_head_clock, last_seen_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'online', ?7, ?8)",
                        params![
                            ann.peer_id,
                            nickname,
                            ann.bio,
                            avatar,
                            src_ip,
                            ann.p2p_port as i64,
                            ann.moments_head_clock,
                            now
                        ],
                    )?;
                }
            }
            Ok(touch)
        })
    }

    pub fn set_peer_offline(&self, peer_id: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let updated = conn.execute(
                "UPDATE peers SET status = 'offline' WHERE peer_id = ?1 AND status = 'online'",
                params![peer_id],
            )?;
            Ok(updated > 0)
        })
    }

    /// Mark peers whose heartbeat is older than `timeout_ms` as offline.
    pub fn sweep_stale_peers(&self, now: i64, timeout_ms: i64) -> Result<Vec<String>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT peer_id FROM peers WHERE status = 'online' AND last_seen_at < ?1",
            )?;
            let ids: Vec<String> = stmt
                .query_map(params![now - timeout_ms], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);
            if !ids.is_empty() {
                conn.execute(
                    "UPDATE peers SET status = 'offline' \
                     WHERE status = 'online' AND last_seen_at < ?1",
                    params![now - timeout_ms],
                )?;
            }
            Ok(ids)
        })
    }

    pub fn touch_peer_seen(&self, peer_id: &str, now: i64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE peers SET last_seen_at = ?2, status = 'online' WHERE peer_id = ?1",
                params![peer_id, now],
            )?;
            Ok(())
        })
    }

    /// Refresh the transient p2p endpoint of a known peer after a handshake.
    pub fn update_peer_endpoint(&self, peer_id: &str, p2p_port: u16, now: i64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE peers SET last_p2p_port = ?2, last_seen_at = ?3, status = 'online' \
                 WHERE peer_id = ?1",
                params![peer_id, p2p_port as i64, now],
            )?;
            Ok(())
        })
    }

    pub fn get_peer(&self, peer_id: &str) -> Result<Option<PeerRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT peer_id, nickname, bio, avatar_color, last_ip, last_p2p_port, status, \
                 moments_head_clock, last_seen_at FROM peers WHERE peer_id = ?1",
                params![peer_id],
                map_peer,
            )
            .optional()
            .map_err(Into::into)
        })
    }

    pub fn list_peers(&self) -> Result<Vec<PeerRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT peer_id, nickname, bio, avatar_color, last_ip, last_p2p_port, status, \
                 moments_head_clock, last_seen_at FROM peers \
                 ORDER BY CASE status WHEN 'online' THEN 0 ELSE 1 END, nickname COLLATE NOCASE",
            )?;
            let rows = stmt
                .query_map([], map_peer)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn list_online_peers(&self) -> Result<Vec<PeerRecord>> {
        Ok(self
            .list_peers()?
            .into_iter()
            .filter(|p| p.status == "online")
            .collect())
    }

    // ------------------------------------------------------------------
    // Messages
    // ------------------------------------------------------------------

    pub fn insert_message(&self, m: &MessageRecord) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                "INSERT OR IGNORE INTO messages (msg_id, conversation_id, sender_id, sender_name, \
                 receiver_id, msg_type, content, blob_id, media_meta_json, lamport_ts, created_at, \
                 delivery_status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    m.msg_id,
                    m.conversation_id,
                    m.sender_id,
                    m.sender_name,
                    m.receiver_id,
                    m.msg_type,
                    m.content,
                    m.blob_id,
                    m.media_meta_json,
                    m.lamport_ts,
                    m.created_at,
                    m.delivery_status
                ],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn set_message_status(&self, msg_id: &str, status: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                "UPDATE messages SET delivery_status = ?2 WHERE msg_id = ?1 AND delivery_status <> ?2",
                params![msg_id, status],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn get_message(&self, msg_id: &str) -> Result<Option<MessageRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                &format!("{} WHERE msg_id = ?1", MESSAGE_SELECT),
                params![msg_id],
                map_message,
            )
            .optional()
            .map_err(Into::into)
        })
    }

    pub fn list_messages(&self, conversation_id: &str, limit: usize) -> Result<Vec<MessageRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT * FROM ({} WHERE conversation_id = ?1 \
                 ORDER BY lamport_ts DESC, created_at DESC LIMIT ?2) \
                 ORDER BY lamport_ts ASC, created_at ASC",
                MESSAGE_SELECT
            ))?;
            let rows = stmt
                .query_map(params![conversation_id, limit as i64], map_message)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn search_messages(&self, query: &str, limit: usize) -> Result<Vec<MessageRecord>> {
        self.with_conn(|conn| {
            let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
            let mut stmt = conn.prepare(&format!(
                "{} WHERE content LIKE ?1 ESCAPE '\\' \
                 ORDER BY created_at DESC LIMIT ?2",
                MESSAGE_SELECT
            ))?;
            let rows = stmt
                .query_map(params![pattern, limit as i64], map_message)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn recent_lobby_messages(&self, limit: usize) -> Result<Vec<MessageRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT * FROM ({} WHERE conversation_id = 'lobby' \
                 ORDER BY created_at DESC LIMIT ?1) ORDER BY created_at ASC",
                MESSAGE_SELECT
            ))?;
            let rows = stmt
                .query_map(params![limit as i64], map_message)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Queued messages that should be flushed to `peer_id`: direct messages
    /// addressed to that peer plus our own queued lobby broadcasts.
    pub fn queued_messages_for(&self, peer_id: &str, self_id: &str, limit: usize) -> Result<Vec<MessageRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "{} WHERE delivery_status = 'queued' \
                 AND (receiver_id = ?1 OR (receiver_id = 'lobby' AND sender_id = ?2)) \
                 ORDER BY lamport_ts ASC LIMIT ?3",
                MESSAGE_SELECT
            ))?;
            let rows = stmt
                .query_map(params![peer_id, self_id, limit as i64], map_message)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn conversation_summaries(&self) -> Result<Vec<ConversationSummary>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT conversation_id, MAX(lamport_ts), MAX(created_at) \
                 FROM messages GROUP BY conversation_id",
            )?;
            let raw = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);

            let mut result = Vec::with_capacity(raw.len());
            for (conv_id, lamport, created) in raw {
                let last = conn
                    .query_row(
                        &format!(
                            "{} WHERE conversation_id = ?1 ORDER BY lamport_ts DESC, created_at DESC LIMIT 1",
                            MESSAGE_SELECT
                        ),
                        params![conv_id],
                        map_message,
                    )
                    .optional()?;
                let peer_id = if conv_id == "lobby" {
                    None
                } else {
                    let ident = conn
                        .query_row("SELECT peer_id FROM identity LIMIT 1", [], |r| {
                            r.get::<_, String>(0)
                        })
                        .optional()?;
                    ident.and_then(|me| {
                        conv_id
                            .split(':')
                            .find(|part| *part != me)
                            .map(str::to_string)
                    })
                };
                let peer = match &peer_id {
                    Some(id) => conn
                        .query_row(
                            "SELECT nickname, status FROM peers WHERE peer_id = ?1",
                            params![id],
                            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                        )
                        .optional()?,
                    None => None,
                };
                let (peer_name, peer_status) = match peer {
                    Some((name, status)) => (Some(name), Some(status)),
                    None => (
                        last.as_ref().map(|m| m.sender_name.clone()),
                        Some("offline".to_string()),
                    ),
                };
                result.push(ConversationSummary {
                    conversation_id: conv_id,
                    peer_id,
                    peer_name,
                    peer_status,
                    last_message: last,
                    last_at: created.max(lamport),
                });
            }
            result.sort_by(|a, b| b.last_at.cmp(&a.last_at));
            Ok(result)
        })
    }

    // ------------------------------------------------------------------
    // Blobs
    // ------------------------------------------------------------------

    pub fn ensure_blob(&self, blob_id: &str, mime: &str, size: i64, source: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let created = conn.execute(
                "INSERT OR IGNORE INTO blobs (blob_id, mime_type, size_bytes, is_present, \
                 source_peer_id, created_at) VALUES (?1, ?2, ?3, 0, ?4, ?5)",
                params![blob_id, mime, size, source, now_ms()],
            )?;
            if created == 0 && !source.is_empty() {
                conn.execute(
                    "UPDATE blobs SET source_peer_id = COALESCE(source_peer_id, ?2), \
                     mime_type = CASE WHEN mime_type = 'application/octet-stream' THEN ?3 ELSE mime_type END \
                     WHERE blob_id = ?1",
                    params![blob_id, source, mime],
                )?;
            }
            Ok(created > 0)
        })
    }

    pub fn mark_blob_present(&self, blob_id: &str, size: i64, mime: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO blobs (blob_id, mime_type, size_bytes, is_present, source_peer_id, created_at) \
                 VALUES (?1, ?2, ?3, 1, NULL, ?4) \
                 ON CONFLICT(blob_id) DO UPDATE SET is_present = 1, size_bytes = ?3, mime_type = ?2",
                params![blob_id, mime, size, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn set_blob_source(&self, blob_id: &str, source: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE blobs SET source_peer_id = ?2 WHERE blob_id = ?1",
                params![blob_id, source],
            )?;
            Ok(())
        })
    }

    pub fn get_blob(&self, blob_id: &str) -> Result<Option<BlobRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT blob_id, mime_type, size_bytes, is_present, source_peer_id, created_at \
                 FROM blobs WHERE blob_id = ?1",
                params![blob_id],
                map_blob,
            )
            .optional()
            .map_err(Into::into)
        })
    }

    pub fn pending_blobs(&self, limit: usize) -> Result<Vec<BlobRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT blob_id, mime_type, size_bytes, is_present, source_peer_id, created_at \
                 FROM blobs WHERE is_present = 0 ORDER BY created_at ASC LIMIT ?1",
            )?;
            let rows = stmt
                .query_map(params![limit as i64], map_blob)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn present_blobs(&self, limit: usize) -> Result<Vec<BlobRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT blob_id, mime_type, size_bytes, is_present, source_peer_id, created_at \
                 FROM blobs WHERE is_present = 1 ORDER BY created_at DESC LIMIT ?1",
            )?;
            let rows = stmt
                .query_map(params![limit as i64], map_blob)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn blob_stats(&self) -> Result<BlobStats> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM blobs WHERE is_present = 1",
            )?;
            let stats = stmt.query_row([], |row| {
                Ok(BlobStats {
                    count: row.get(0)?,
                    bytes: row.get(1)?,
                })
            })?;
            Ok(stats)
        })
    }

    // ------------------------------------------------------------------
    // Moments
    // ------------------------------------------------------------------

    pub fn insert_moment_event(&self, e: &MomentEvent) -> Result<bool> {
        self.with_conn(|conn| {
            let changed = conn.execute(
                "INSERT OR IGNORE INTO moment_events (event_id, moment_id, author_id, author_name, \
                 event_type, content, blob_ids_json, media_meta_json, lamport_ts, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    e.event_id,
                    e.moment_id,
                    e.author_id,
                    e.author_name,
                    e.event_type,
                    e.content,
                    e.blob_ids_json,
                    e.media_meta_json,
                    e.lamport_ts,
                    e.created_at
                ],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn list_moment_events(&self) -> Result<Vec<MomentEvent>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "{MOMENT_SELECT} ORDER BY created_at ASC, lamport_ts ASC"
            ))?;
            let rows = stmt
                .query_map([], map_moment)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Find a specific interaction event (used to keep likes idempotent).
    pub fn find_event(
        &self,
        moment_id: &str,
        author_id: &str,
        event_type: &str,
    ) -> Result<Option<MomentEvent>> {
        self.with_conn(|conn| {
            conn.query_row(
                &format!(
                    "{MOMENT_SELECT} WHERE moment_id = ?1 AND author_id = ?2 AND event_type = ?3 LIMIT 1"
                ),
                params![moment_id, author_id, event_type],
                map_moment,
            )
            .optional()
            .map_err(Into::into)
        })
    }

    pub fn known_moment_clocks(&self) -> Result<HashMap<String, i64>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT author_id, MAX(lamport_ts) FROM moment_events GROUP BY author_id",
            )?;
            let mut map = HashMap::new();
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let author: String = row.get(0)?;
                let clock: i64 = row.get(1)?;
                map.insert(author, clock);
            }
            Ok(map)
        })
    }

    /// Events the requester is missing, judging by their version vector.
    pub fn events_missing_for(&self, known: &HashMap<String, i64>) -> Result<Vec<MomentEvent>> {
        let all = self.list_moment_events()?;
        Ok(all
            .into_iter()
            .filter(|e| e.lamport_ts > *known.get(&e.author_id).unwrap_or(&0))
            .take(2000)
            .collect())
    }

    /// Max Lamport timestamp of moments authored by `peer_id` (0 if none).
    pub fn local_moments_head(&self, peer_id: &str) -> Result<i64> {
        self.with_conn(|conn| {
            let head = conn.query_row(
                "SELECT COALESCE(MAX(lamport_ts), 0) FROM moment_events WHERE author_id = ?1",
                params![peer_id],
                |row| row.get::<_, i64>(0),
            )?;
            Ok(head)
        })
    }
}

const MESSAGE_SELECT: &str = "SELECT msg_id, conversation_id, sender_id, sender_name, receiver_id, \
    msg_type, content, blob_id, media_meta_json, lamport_ts, created_at, delivery_status FROM messages";

const MOMENT_SELECT: &str = "SELECT event_id, moment_id, author_id, author_name, event_type, content, \
    blob_ids_json, media_meta_json, lamport_ts, created_at FROM moment_events";

fn map_identity(row: &rusqlite::Row<'_>) -> rusqlite::Result<Identity> {
    Ok(Identity {
        peer_id: row.get(0)?,
        nickname: row.get(1)?,
        bio: row.get(2)?,
        avatar_color: row.get(3)?,
        lamport_clock: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn map_peer(row: &rusqlite::Row<'_>) -> rusqlite::Result<PeerRecord> {
    Ok(PeerRecord {
        peer_id: row.get(0)?,
        nickname: row.get(1)?,
        bio: row.get(2)?,
        avatar_color: row.get(3)?,
        last_ip: row.get(4)?,
        last_p2p_port: row.get(5)?,
        status: row.get(6)?,
        moments_head_clock: row.get(7)?,
        last_seen_at: row.get(8)?,
    })
}

fn map_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRecord> {
    Ok(MessageRecord {
        msg_id: row.get(0)?,
        conversation_id: row.get(1)?,
        sender_id: row.get(2)?,
        sender_name: row.get(3)?,
        receiver_id: row.get(4)?,
        msg_type: row.get(5)?,
        content: row.get(6)?,
        blob_id: row.get(7)?,
        media_meta_json: row.get(8)?,
        lamport_ts: row.get(9)?,
        created_at: row.get(10)?,
        delivery_status: row.get(11)?,
    })
}

fn map_blob(row: &rusqlite::Row<'_>) -> rusqlite::Result<BlobRecord> {
    Ok(BlobRecord {
        blob_id: row.get(0)?,
        mime_type: row.get(1)?,
        size_bytes: row.get(2)?,
        is_present: row.get::<_, i64>(3)? != 0,
        source_peer_id: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn map_moment(row: &rusqlite::Row<'_>) -> rusqlite::Result<MomentEvent> {
    Ok(MomentEvent {
        event_id: row.get(0)?,
        moment_id: row.get(1)?,
        author_id: row.get(2)?,
        author_name: row.get(3)?,
        event_type: row.get(4)?,
        content: row.get(5)?,
        blob_ids_json: row.get(6)?,
        media_meta_json: row.get(7)?,
        lamport_ts: row.get(8)?,
        created_at: row.get(9)?,
    })
}

fn is_loopback_ip(ip: &str) -> bool {
    ip == "127.0.0.1" || ip.starts_with("127.") || ip == "::1"
}

fn acc_short(peer_id: &str) -> String {
    peer_id.chars().take(4).collect::<String>().to_uppercase()
}
