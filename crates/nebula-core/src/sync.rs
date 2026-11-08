//! Background synchronization:
//!
//! 1. **Outbox flush** — queued direct messages are pushed to a peer as soon
//!    as it reappears (via `/api/p2p/sync`, which acks delivered ids).
//! 2. **Moments gossip** — version-vector reconciliation against every online
//!    peer. A peer relays events it learned from *other* peers, so a moment
//!    survives even if its author went offline.
//! 3. **CAS backfill** — blobs referenced by messages/events but missing on
//!    disk are streamed from a peer and verified against their SHA-256.

use crate::blob::BlobStore;
use crate::db::{now_ms, MessageRecord, MomentEvent, PeerRecord};
use crate::NebulaNode;
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

pub fn start(node: Arc<NebulaNode>) {
    tokio::spawn(sync_loop(node.clone()));
    tokio::spawn(blob_loop(node));
}

async fn sync_loop(node: Arc<NebulaNode>) {
    let mut rx = node.subscribe_shutdown();
    let mut last_sync: std::collections::HashMap<String, Instant> = Default::default();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(node.cfg.sync_interval) => {}
            _ = node.sync_kick.notified() => {}
            _ = wait_shutdown(&mut rx) => break,
        }
        if node.is_shutdown() {
            break;
        }
        let peers = match node.db.list_online_peers() {
            Ok(peers) => peers,
            Err(err) => {
                warn!("sync: cannot list peers: {err:#}");
                continue;
            }
        };
        for peer in peers {
            if peer.peer_id == node.peer_id() {
                continue;
            }
            let now = Instant::now();
            if let Some(prev) = last_sync.get(&peer.peer_id) {
                if now.duration_since(*prev) < Duration::from_millis(400) {
                    continue;
                }
            }
            last_sync.insert(peer.peer_id.clone(), now);
            if let Err(err) = sync_with_peer(&node, &peer).await {
                debug!("sync with {} failed: {err:#}", peer.nickname);
            }
        }
    }
}

/// One `/api/p2p/sync` round trip: flush the outbox and reconcile gossip.
pub async fn sync_with_peer(node: &NebulaNode, peer: &PeerRecord) -> Result<Value> {
    if peer.peer_id == node.peer_id() {
        return Ok(Value::Null);
    }
    let me = node.identity();
    let queued = node
        .db
        .queued_messages_for(&peer.peer_id, &me.peer_id, 200)?;
    let clocks = node.db.known_moment_clocks()?;
    let body = json!({
        "sender_peer": node.peer_hello(),
        "queued_messages": queued,
        "known_moment_clocks": clocks,
    });
    let url = format!(
        "http://{}:{}/api/p2p/sync",
        peer.last_ip, peer.last_p2p_port
    );
    let response = node.http.post_json(&url, &body).await?;

    if let Some(ids) = response.get("acked_msg_ids").and_then(Value::as_array) {
        for id in ids.iter().filter_map(Value::as_str) {
            if node.db.set_message_status(id, "delivered")? {
                node.emit(json!({
                    "type": "MESSAGE_STATUS",
                    "msg_id": id,
                    "delivery_status": "delivered"
                }));
            }
        }
    }

    let mut new_events = 0usize;
    if let Some(events) = response
        .get("missing_moment_events")
        .and_then(Value::as_array)
    {
        for raw in events {
            match serde_json::from_value::<MomentEvent>(raw.clone()) {
                Ok(event) => {
                    if node.ingest_moment_event(&event)? {
                        new_events += 1;
                    }
                }
                Err(err) => debug!("bad moment event in sync response: {err}"),
            }
        }
    }

    if let Some(messages) = response
        .get("missing_lobby_messages")
        .and_then(Value::as_array)
    {
        for raw in messages {
            match serde_json::from_value::<MessageRecord>(raw.clone()) {
                Ok(message) => {
                    node.ingest_message(&message)?;
                }
                Err(err) => debug!("bad lobby message in sync response: {err}"),
            }
        }
    }

    node.db.touch_peer_seen(&peer.peer_id, now_ms())?;

    // Only propagate when the local event log actually grew (pitfall #6:
    // otherwise the fleet would keep re-syncing the same events forever).
    if new_events > 0 {
        node.sync_kick.notify_one();
    }
    Ok(response)
}

async fn blob_loop(node: Arc<NebulaNode>) {
    let mut rx = node.subscribe_shutdown();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(node.cfg.blob_fetch_interval) => {}
            _ = node.blob_kick.notified() => {}
            _ = wait_shutdown(&mut rx) => break,
        }
        if node.is_shutdown() {
            break;
        }
        let pending = match node.db.pending_blobs(16) {
            Ok(pending) => pending,
            Err(err) => {
                warn!("blob fetch: cannot list pending blobs: {err:#}");
                continue;
            }
        };
        for blob in pending {
            if node.blobs.has(&blob.blob_id) {
                let size = node.blobs.size(&blob.blob_id).unwrap_or(0) as i64;
                let _ = node
                    .db
                    .mark_blob_present(&blob.blob_id, size, &blob.mime_type);
                continue;
            }
            let candidates = match candidate_peers(&node, &blob) {
                Ok(candidates) => candidates,
                Err(err) => {
                    warn!("blob fetch: peer lookup failed: {err:#}");
                    continue;
                }
            };
            for peer in candidates {
                let url = format!(
                    "http://{}:{}/api/p2p/blob/{}",
                    peer.last_ip, peer.last_p2p_port, blob.blob_id
                );
                match node.http.get_bytes(&url).await {
                    Ok((mime, bytes)) => {
                        let actual = BlobStore::sha256_hex(&bytes);
                        if actual != blob.blob_id {
                            warn!(
                                "blob {} from {} failed SHA-256 verification",
                                blob.blob_id, peer.nickname
                            );
                            continue;
                        }
                        if let Err(err) = node.blobs.put_verified(&blob.blob_id, &bytes) {
                            warn!("blob {} could not be committed: {err:#}", blob.blob_id);
                            continue;
                        }
                        let mime = if mime.starts_with("application/octet-stream") {
                            blob.mime_type.clone()
                        } else {
                            mime
                        };
                        if let Err(err) = node
                            .db
                            .mark_blob_present(&blob.blob_id, bytes.len() as i64, &mime)
                        {
                            warn!("blob {} metadata not persisted: {err:#}", blob.blob_id);
                            continue;
                        }
                        if let Err(err) = node.db.set_blob_source(&blob.blob_id, &peer.peer_id) {
                            warn!("blob {} source not updated: {err:#}", blob.blob_id);
                        }
                        node.emit(json!({
                            "type": "BLOB_READY",
                            "blob_id": blob.blob_id,
                            "mime_type": mime,
                            "size_bytes": bytes.len()
                        }));
                        debug!(
                            "blob {} downloaded from {} ({} bytes)",
                            blob.blob_id,
                            peer.nickname,
                            bytes.len()
                        );
                        break;
                    }
                    Err(err) => {
                        debug!(
                            "blob {} not available from {}: {err:#}",
                            blob.blob_id, peer.nickname
                        );
                    }
                }
            }
        }
    }
}

/// Prefer the advertised source peer, then fall back to any online peer.
fn candidate_peers(
    node: &NebulaNode,
    blob: &crate::db::BlobRecord,
) -> Result<Vec<PeerRecord>> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    if let Some(source) = blob.source_peer_id.as_deref() {
        if let Some(peer) = node.db.get_peer(source)? {
            if peer.status == "online" && seen.insert(peer.peer_id.clone()) {
                result.push(peer);
            }
        }
    }
    for peer in node.db.list_online_peers()? {
        if peer.peer_id == node.peer_id() {
            continue;
        }
        if seen.insert(peer.peer_id.clone()) {
            result.push(peer);
        }
    }
    Ok(result)
}

async fn wait_shutdown(rx: &mut tokio::sync::watch::Receiver<bool>) {
    if *rx.borrow() {
        return;
    }
    let _ = rx.changed().await;
}
