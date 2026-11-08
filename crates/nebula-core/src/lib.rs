//! Nebula (星云) — a zero-server, LAN-only P2P messenger and decentralized
//! moments platform.
//!
//! [`NebulaNode`] is the single runtime controller exported to the web UI
//! server, the TUI and the Tauri host.

pub mod blob;
pub mod client;
pub mod config;
pub mod db;
pub mod discovery;
pub mod moments;
pub mod server;
pub mod sync;

pub use blob::{guess_mime, BlobStore};
pub use client::HttpClient;
pub use config::{default_data_dir, parse_cli, CliOptions, NebulaConfig, CLI_HELP};
pub use db::{
    conversation_id, make_gid, now_ms, BlobRecord, BlobStats, ConversationSummary, DbPool,
    Identity, MessageRecord, MomentEvent, PeerAnnounce, PeerRecord,
};
pub use moments::MomentView;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch, Notify};
use tracing::info;

/// Public node descriptor exchanged during P2P handshakes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerHello {
    pub peer_id: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub bio: String,
    #[serde(default)]
    pub avatar_color: String,
    pub p2p_port: u16,
}

/// Request body for sending a message through the local API / TUI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SendMessageRequest {
    #[serde(default)]
    pub receiver_id: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub msg_type: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub blob_id: Option<String>,
    #[serde(default)]
    pub media_meta: Option<Value>,
}

pub const PROTOCOL: &str = "NEBULA_V1";

/// The heart of a Nebula node: database, CAS store, discovery, HTTP servers
/// and background synchronization.
pub struct NebulaNode {
    pub cfg: NebulaConfig,
    pub db: DbPool,
    pub blobs: BlobStore,
    pub http: HttpClient,
    pub p2p_port: u16,
    pub ui_port: u16,
    identity: RwLock<Identity>,
    events: broadcast::Sender<Value>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    /// Wakes the sync loop (peer appeared / new gossip event / offline flush).
    pub sync_kick: Notify,
    /// Wakes the blob fetcher (a new remote blob reference appeared).
    pub blob_kick: Notify,
    /// Wakes the discovery heartbeat (profile changed).
    pub announce_kick: Notify,
}

impl std::fmt::Debug for NebulaNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NebulaNode")
            .field("p2p_port", &self.p2p_port)
            .field("ui_port", &self.ui_port)
            .field("data_dir", &self.cfg.data_dir)
            .finish()
    }
}

impl NebulaNode {
    /// Boot the node: open storage, bind servers, start discovery + sync.
    pub async fn start(cfg: NebulaConfig) -> Result<Arc<Self>> {
        tokio::fs::create_dir_all(&cfg.data_dir)
            .await
            .with_context(|| format!("creating data dir {}", cfg.data_dir.display()))?;
        let db = DbPool::open(cfg.data_dir.join("nebula.db"))?;
        let identity = db.ensure_identity(cfg.nickname.as_deref(), &cfg.bio)?;
        let blobs = BlobStore::new(cfg.data_dir.join("blobs"))?;

        let p2p_listener = TcpListener::bind(("0.0.0.0", cfg.p2p_port))
            .await
            .with_context(|| format!("binding P2P port {}", cfg.p2p_port))?;
        let p2p_port = p2p_listener.local_addr()?.port();

        // The UI listener is loopback-only. When both ports are explicitly the
        // same we simply reuse the P2P listener.
        let mut ui_listener = None;
        if cfg.ui_port != p2p_port {
            let listener = TcpListener::bind(("127.0.0.1", cfg.ui_port))
                .await
                .with_context(|| format!("binding UI port {}", cfg.ui_port))?;
            let actual = listener.local_addr()?.port();
            if actual != p2p_port {
                ui_listener = Some(listener);
            }
        }
        let ui_port = ui_listener
            .as_ref()
            .and_then(|l| l.local_addr().ok())
            .map(|a| a.port())
            .unwrap_or(p2p_port);

        let (events, _) = broadcast::channel(2048);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let node = Arc::new(Self {
            cfg,
            db,
            blobs,
            http: HttpClient::default(),
            p2p_port,
            ui_port,
            identity: RwLock::new(identity),
            events,
            shutdown_tx,
            shutdown_rx,
            sync_kick: Notify::new(),
            blob_kick: Notify::new(),
            announce_kick: Notify::new(),
        });

        let app = server::router(node.clone());
        let p2p_server = app.clone();
        let node_for_p2p = node.clone();
        tokio::spawn(async move {
            server::serve(p2p_server, p2p_listener, node_for_p2p).await;
        });
        if let Some(listener) = ui_listener {
            let ui_server = app;
            let node_for_ui = node.clone();
            tokio::spawn(async move {
                server::serve(ui_server, listener, node_for_ui).await;
            });
        }

        discovery::start(node.clone());
        sync::start(node.clone());

        let ident = node.identity();
        info!(
            peer_id = %ident.peer_id,
            nickname = %ident.nickname,
            p2p_port,
            ui_port,
            data_dir = %node.cfg.data_dir.display(),
            "Nebula node online — UI at http://127.0.0.1:{}",
            ui_port
        );
        Ok(node)
    }

    // ------------------------------------------------------------------
    // Accessors
    // ------------------------------------------------------------------

    pub fn identity(&self) -> Identity {
        self.identity
            .read()
            .expect("identity lock poisoned")
            .clone()
    }

    pub fn peer_id(&self) -> String {
        self.identity().peer_id
    }

    pub fn is_shutdown(&self) -> bool {
        *self.shutdown_rx.borrow()
    }

    pub fn subscribe_shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    pub fn emit(&self, event: Value) {
        let _ = self.events.send(event);
    }

    pub fn ui_base(&self) -> String {
        format!("http://127.0.0.1:{}", self.ui_port)
    }

    pub fn peer_hello(&self) -> PeerHello {
        let id = self.identity();
        PeerHello {
            peer_id: id.peer_id,
            nickname: id.nickname,
            bio: id.bio,
            avatar_color: id.avatar_color,
            p2p_port: self.p2p_port,
        }
    }

    /// Gracefully stop the node (sends LEAVE, closes listeners).
    pub async fn shutdown(&self) {
        if !self.is_shutdown() {
            let _ = self.shutdown_tx.send(true);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // ------------------------------------------------------------------
    // Aggregated views
    // ------------------------------------------------------------------

    pub fn conversations(&self) -> Result<Vec<ConversationSummary>> {
        let identity = self.identity();
        let peers = self.db.list_peers()?;
        let mut list = self.db.conversation_summaries()?;

        let mut ensure = |conv_id: String, peer: Option<&PeerRecord>| {
            if !list.iter().any(|c| c.conversation_id == conv_id) {
                list.push(ConversationSummary {
                    conversation_id: conv_id,
                    peer_id: peer.map(|p| p.peer_id.clone()),
                    peer_name: peer.map(|p| p.nickname.clone()),
                    peer_status: Some(
                        peer.map(|p| p.status.clone())
                            .unwrap_or_else(|| "offline".to_string()),
                    ),
                    last_message: None,
                    last_at: 0,
                });
            }
        };

        ensure("lobby".to_string(), None);
        for peer in &peers {
            ensure(conversation_id(&identity.peer_id, &peer.peer_id), Some(peer));
        }

        // Backfill peer metadata for summaries that only carry message data.
        for item in list.iter_mut() {
            if item.peer_id.is_none() && item.conversation_id != "lobby" {
                item.peer_id = item
                    .conversation_id
                    .split(':')
                    .find(|part| *part != identity.peer_id)
                    .map(str::to_string);
            }
            if let Some(peer) = item
                .peer_id
                .as_ref()
                .and_then(|id| peers.iter().find(|p| &p.peer_id == id))
            {
                item.peer_name = Some(peer.nickname.clone());
                item.peer_status = Some(peer.status.clone());
            }
        }

        list.sort_by(|a, b| {
            // Pin the lobby first when neither conversation has activity.
            let a_lobby = (a.conversation_id == "lobby") as i64;
            let b_lobby = (b.conversation_id == "lobby") as i64;
            b.last_at
                .cmp(&a.last_at)
                .then(b_lobby.cmp(&a_lobby))
                .then(a.conversation_id.cmp(&b.conversation_id))
        });
        Ok(list)
    }

    pub fn moments_view(&self) -> Result<Vec<MomentView>> {
        let events = self.db.list_moment_events()?;
        Ok(moments::aggregate(&events, &self.identity().peer_id))
    }

    pub fn bootstrap(&self) -> Result<Value> {
        let identity = self.identity();
        let peers = self.db.list_peers()?;
        let conversations = self.conversations()?;
        let moments = self.moments_view()?;
        let blob_stats = self.db.blob_stats()?;
        let media = self.db.present_blobs(200)?;
        let pending_blobs = self.db.pending_blobs(200)?;
        Ok(json!({
            "identity": identity,
            "peers": peers,
            "conversations": conversations,
            "moments": moments,
            "blob_stats": blob_stats,
            "media": media,
            "pending_blobs": pending_blobs,
            "node": {
                "p2p_port": self.p2p_port,
                "ui_port": self.ui_port,
                "ui_base": self.ui_base(),
                "data_dir": self.cfg.data_dir.display().to_string(),
                "protocol": PROTOCOL,
            },
            "server_time": now_ms()
        }))
    }

    // ------------------------------------------------------------------
    // Local actions
    // ------------------------------------------------------------------

    /// Persist a locally authored message and attempt immediate delivery.
    pub async fn send_message(&self, req: SendMessageRequest) -> Result<MessageRecord> {
        let identity = self.identity();
        let receiver = resolve_receiver(&req, &identity.peer_id)?;
        let msg_type = req.msg_type.unwrap_or_else(|| "text".to_string());
        if !matches!(msg_type.as_str(), "text" | "image" | "voice") {
            return Err(anyhow!("invalid msg_type: {msg_type}"));
        }
        let blob_id = req.blob_id.filter(|b| !b.is_empty());
        if msg_type != "text" && blob_id.is_none() {
            return Err(anyhow!("{msg_type} messages require a blob_id"));
        }
        let media_meta_json = req.media_meta.map(|v| v.to_string());
        let lamport = self.db.increment_lamport()?;
        let now = now_ms();
        let conv = if receiver == "lobby" {
            "lobby".to_string()
        } else {
            conversation_id(&identity.peer_id, &receiver)
        };
        let msg = MessageRecord {
            msg_id: make_gid(lamport, &identity.peer_id),
            conversation_id: conv,
            sender_id: identity.peer_id.clone(),
            sender_name: identity.nickname.clone(),
            receiver_id: receiver.clone(),
            msg_type,
            content: req.content.unwrap_or_default(),
            blob_id,
            media_meta_json,
            lamport_ts: lamport,
            created_at: now,
            delivery_status: "queued".to_string(),
        };
        self.db.insert_message(&msg)?;
        self.emit(json!({"type": "NEW_MESSAGE", "message": msg}));

        let targets = if receiver == "lobby" {
            self.db.list_online_peers()?
        } else {
            self.db
                .get_peer(&receiver)?
                .filter(|p| p.status == "online")
                .into_iter()
                .collect()
        };

        if !targets.is_empty() {
            let deliveries = targets.iter().map(|peer| {
                let msg_ref = &msg;
                async move { self.deliver_chat(peer, msg_ref).await.is_ok() }
            });
            let results = futures_util::future::join_all(deliveries).await;
            if results.into_iter().any(|ok| ok) {
                self.mark_delivered(&msg.msg_id, &msg.conversation_id)?;
                let mut delivered = msg.clone();
                delivered.delivery_status = "delivered".to_string();
                return Ok(delivered);
            }
        }
        Ok(msg)
    }

    fn mark_delivered(&self, msg_id: &str, conversation_id: &str) -> Result<()> {
        if self.db.set_message_status(msg_id, "delivered")? {
            self.emit(json!({
                "type": "MESSAGE_STATUS",
                "msg_id": msg_id,
                "conversation_id": conversation_id,
                "delivery_status": "delivered"
            }));
        }
        Ok(())
    }

    /// POST /api/p2p/chat to a single peer.
    pub async fn deliver_chat(&self, peer: &PeerRecord, msg: &MessageRecord) -> Result<()> {
        let url = format!(
            "http://{}:{}/api/p2p/chat",
            peer.last_ip, peer.last_p2p_port
        );
        let body = json!({"message": msg, "sender_peer": self.peer_hello()});
        let resp = self.http.post_json(&url, &body).await?;
        if resp.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            self.db.touch_peer_seen(&peer.peer_id, now_ms())?;
            Ok(())
        } else {
            Err(anyhow!("peer {} rejected chat message", peer.peer_id))
        }
    }

    /// Ingest a message received from the network. Returns `true` when new.
    pub fn ingest_message(&self, msg: &MessageRecord) -> Result<bool> {
        let inserted = self.db.insert_message(msg)?;
        if inserted {
            self.db.observe_lamport(msg.lamport_ts)?;
            if let Some(blob_id) = msg.blob_id.as_deref() {
                self.register_blob(
                    blob_id,
                    msg.media_meta_json.as_deref(),
                    &msg.sender_id,
                );
            }
            self.emit(json!({"type": "NEW_MESSAGE", "message": msg}));
            self.blob_kick.notify_one();
        }
        Ok(inserted)
    }

    /// Register a remote blob reference so the background fetcher can pull it.
    pub fn register_blob(&self, blob_id: &str, media_meta_json: Option<&str>, source: &str) {
        let meta: Value = media_meta_json
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);
        let mime = meta
            .get("mime_type")
            .and_then(Value::as_str)
            .or_else(|| {
                meta.get("mimes")
                    .and_then(|m| m.get(blob_id))
                    .and_then(Value::as_str)
            })
            .unwrap_or("application/octet-stream");
        let size = meta
            .get("size_bytes")
            .and_then(Value::as_i64)
            .or_else(|| {
                meta.get("sizes")
                    .and_then(|m| m.get(blob_id))
                    .and_then(Value::as_i64)
            })
            .unwrap_or(0);
        if let Err(err) = self.db.ensure_blob(blob_id, mime, size, source) {
            tracing::warn!("failed to register blob {blob_id}: {err:#}");
        }
    }

    /// Store locally produced media into the CAS and return `(blob_id, size)`.
    pub fn store_blob(&self, bytes: &[u8], mime: &str) -> Result<(String, usize)> {
        let (blob_id, size) = self.blobs.put_bytes(bytes)?;
        self.db.mark_blob_present(&blob_id, size as i64, mime)?;
        Ok((blob_id, size))
    }

    pub fn get_blob_record(&self, blob_id: &str) -> Result<Option<BlobRecord>> {
        self.db.get_blob(blob_id)
    }

    pub fn create_moment_post(
        &self,
        content: &str,
        blob_ids: Vec<String>,
        media_meta: Option<Value>,
        mime_map: Option<Value>,
    ) -> Result<MomentEvent> {
        let identity = self.identity();
        let lamport = self.db.increment_lamport()?;
        let event_id = make_gid(lamport, &identity.peer_id);
        let mut meta = media_meta.unwrap_or_else(|| json!({}));
        if let (Some(map), Value::Object(obj)) = (mime_map, &mut meta) {
            obj.insert("mimes".to_string(), map);
        }
        let event = MomentEvent {
            event_id: event_id.clone(),
            moment_id: event_id,
            author_id: identity.peer_id.clone(),
            author_name: identity.nickname.clone(),
            event_type: "POST".to_string(),
            content: content.to_string(),
            blob_ids_json: serde_json::to_string(&blob_ids)?,
            media_meta_json: meta.to_string(),
            lamport_ts: lamport,
            created_at: now_ms(),
        };
        self.db.insert_moment_event(&event)?;
        self.emit(json!({"type": "NEW_MOMENT_EVENT", "event": event}));
        self.sync_kick.notify_one();
        Ok(event)
    }

    pub fn create_moment_interaction(
        &self,
        moment_id: &str,
        event_type: &str,
        content: &str,
    ) -> Result<MomentEvent> {
        if !matches!(event_type, "LIKE" | "COMMENT") {
            return Err(anyhow!("invalid event_type: {event_type}"));
        }
        let identity = self.identity();
        if event_type == "LIKE" {
            if let Some(existing) = self.db.find_event(moment_id, &identity.peer_id, "LIKE")? {
                return Ok(existing);
            }
        }
        let lamport = self.db.increment_lamport()?;
        let event = MomentEvent {
            event_id: make_gid(lamport, &identity.peer_id),
            moment_id: moment_id.to_string(),
            author_id: identity.peer_id.clone(),
            author_name: identity.nickname.clone(),
            event_type: event_type.to_string(),
            content: content.to_string(),
            blob_ids_json: "[]".to_string(),
            media_meta_json: "{}".to_string(),
            lamport_ts: lamport,
            created_at: now_ms(),
        };
        self.db.insert_moment_event(&event)?;
        self.emit(json!({"type": "NEW_MOMENT_EVENT", "event": event}));
        self.sync_kick.notify_one();
        Ok(event)
    }

    /// Ingest a gossip event received from a peer. Returns `true` when new.
    pub fn ingest_moment_event(&self, event: &MomentEvent) -> Result<bool> {
        let inserted = self.db.insert_moment_event(event)?;
        if inserted {
            self.db.observe_lamport(event.lamport_ts)?;
            let ids: Vec<String> =
                serde_json::from_str(&event.blob_ids_json).unwrap_or_default();
            for blob_id in &ids {
                self.register_blob(blob_id, Some(&event.media_meta_json), &event.author_id);
            }
            self.emit(json!({"type": "NEW_MOMENT_EVENT", "event": event}));
            self.blob_kick.notify_one();
            self.sync_kick.notify_one();
        }
        Ok(inserted)
    }

    pub fn update_profile(&self, nickname: &str, bio: &str) -> Result<Identity> {
        let nickname = nickname.trim();
        if nickname.is_empty() {
            return Err(anyhow!("nickname must not be empty"));
        }
        let identity = self.db.update_profile(nickname, bio)?;
        *self.identity.write().expect("identity lock poisoned") = identity.clone();
        self.announce_kick.notify_one();
        self.emit(json!({"type": "IDENTITY_UPDATED", "identity": identity}));
        Ok(identity)
    }
}

fn resolve_receiver(req: &SendMessageRequest, self_id: &str) -> Result<String> {
    if let Some(receiver) = req.receiver_id.as_ref().filter(|r| !r.trim().is_empty()) {
        return Ok(receiver.trim().to_string());
    }
    if let Some(conv) = req.conversation_id.as_ref().filter(|c| !c.trim().is_empty()) {
        if conv == "lobby" {
            return Ok("lobby".to_string());
        }
        if let Some(other) = conv.split(':').find(|part| *part != self_id) {
            return Ok(other.to_string());
        }
        return Ok(conv.clone());
    }
    Err(anyhow!("receiver_id or conversation_id is required"))
}
