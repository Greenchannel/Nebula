//! Real three-node end-to-end cluster test.
//!
//! Everything runs over actual UDP sockets and HTTP requests:
//! discovery, chat delivery, CAS blob replication, offline outbox flush and
//! A → B → C moments gossip relay.

use base64::Engine;
use nebula_core::{BlobStore, NebulaConfig, NebulaNode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

async fn boot(dir: &TempDir, name: &str) -> Arc<NebulaNode> {
    let mut cfg = NebulaConfig::new(dir.path());
    cfg.nickname = Some(name.to_string());
    cfg.p2p_port = 0;
    cfg.ui_port = 0;
    // Accelerated timings keep the suite fast while exercising the same loops.
    cfg.heartbeat = Duration::from_millis(300);
    cfg.offline_timeout = Duration::from_secs(2);
    cfg.sweep_interval = Duration::from_millis(300);
    cfg.sync_interval = Duration::from_millis(300);
    cfg.blob_fetch_interval = Duration::from_millis(200);
    NebulaNode::start(cfg).await.expect("node start")
}

fn peer_online(observer: &NebulaNode, target: &NebulaNode) -> bool {
    observer
        .db
        .get_peer(&target.peer_id())
        .ok()
        .flatten()
        .map(|p| p.status == "online")
        .unwrap_or(false)
}

async fn wait_for<F, Fut>(timeout: Duration, mut predicate: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if predicate().await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn api_post(node: &NebulaNode, path: &str, body: Value) -> Value {
    let url = format!("{}{}", node.ui_base(), path);
    node.http
        .post_json(&url, &body)
        .await
        .unwrap_or_else(|err| panic!("POST {path} failed: {err:#}"))
}

async fn upload_blob(node: &NebulaNode, bytes: &[u8], mime: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let response = api_post(
        node,
        "/api/local/blob",
        json!({"data_b64": encoded, "mime_type": mime}),
    )
    .await;
    response["blob_id"]
        .as_str()
        .expect("blob_id in upload response")
        .to_string()
}

async fn send_message(
    node: &NebulaNode,
    receiver: &str,
    msg_type: &str,
    content: &str,
    blob_id: Option<&str>,
    media_meta: Option<Value>,
) -> Value {
    api_post(
        node,
        "/api/local/messages",
        json!({
            "receiver_id": receiver,
            "msg_type": msg_type,
            "content": content,
            "blob_id": blob_id,
            "media_meta": media_meta,
        }),
    )
    .await
}

fn image_bytes() -> Vec<u8> {
    // A tiny but real PNG (1x1 indigo pixel) — CAS only cares about bytes, but
    // a genuine image makes manual debugging in the UI pleasant.
    base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==")
        .unwrap()
}

fn voice_bytes() -> Vec<u8> {
    // 0.5 s of 8 kHz mono 16-bit PCM sine wave wrapped in a WAV container.
    let sample_rate = 8_000u32;
    let samples = sample_rate / 2;
    let data_len = samples * 2;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..samples {
        let t = i as f32 / sample_rate as f32;
        let sample = (t * 440.0 * std::f32::consts::TAU).sin() * 0.4 * i16::MAX as f32;
        out.extend_from_slice(&(sample as i16).to_le_bytes());
    }
    out
}

fn waveform() -> Vec<i64> {
    (0..16)
        .map(|i| {
            let envelope = ((i as f64 / 16.0) * std::f64::consts::PI).sin();
            (10.0 + envelope * 90.0).round() as i64
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nebula_three_node_cluster() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("nebula_core=warn")),
        )
        .try_init();

    let dir_a = tempfile::tempdir().expect("tempdir a");
    let dir_b = tempfile::tempdir().expect("tempdir b");
    let dir_c = tempfile::tempdir().expect("tempdir c");

    let a = boot(&dir_a, "Alpha").await;
    let b = boot(&dir_b, "Beta").await;

    // ------------------------------------------------------------------
    // Step 1 — automatic UDP discovery
    // ------------------------------------------------------------------
    assert!(
        wait_for(CONNECT_TIMEOUT, || async { peer_online(&a, &b) }).await,
        "Node A never discovered Node B"
    );
    assert!(
        wait_for(CONNECT_TIMEOUT, || async { peer_online(&b, &a) }).await,
        "Node B never discovered Node A"
    );
    // The endpoint must be usable, not just a stale IP.
    let a_view_of_b = a.db.get_peer(&b.peer_id()).unwrap().unwrap();
    assert_eq!(a_view_of_b.last_p2p_port as u16, b.p2p_port);
    println!(
        "✔ Step 1 discovery: {} saw {} at {}:{}",
        a.identity().nickname,
        a_view_of_b.nickname,
        a_view_of_b.last_ip,
        a_view_of_b.last_p2p_port
    );

    // ------------------------------------------------------------------
    // Step 2 — rich media over CAS (image + voice)
    // ------------------------------------------------------------------
    let image = image_bytes();
    let voice = voice_bytes();
    let image_sha = BlobStore::sha256_hex(&image);
    let voice_sha = BlobStore::sha256_hex(&voice);

    let image_id = upload_blob(&a, &image, "image/png").await;
    let voice_id = upload_blob(&a, &voice, "audio/wav").await;
    assert_eq!(image_id, image_sha, "CAS id must be the SHA-256 of the bytes");
    assert_eq!(voice_id, voice_sha);

    let image_meta = json!({
        "thumb_b64": "",
        "width": 1,
        "height": 1,
        "mime_type": "image/png",
        "size_bytes": image.len(),
    });
    let voice_meta = json!({
        "duration_ms": 500,
        "waveform": waveform(),
        "mime_type": "audio/wav",
        "size_bytes": voice.len(),
    });

    let image_msg = send_message(
        &a,
        &b.peer_id(),
        "image",
        "Look at this nebula",
        Some(&image_id),
        Some(image_meta),
    )
    .await;
    assert_eq!(
        image_msg["message"]["delivery_status"], "delivered",
        "online peer must receive immediately: {image_msg}"
    );

    let voice_msg = send_message(
        &a,
        &b.peer_id(),
        "voice",
        "voice note",
        Some(&voice_id),
        Some(voice_meta),
    )
    .await;
    assert_eq!(voice_msg["message"]["delivery_status"], "delivered");

    let conversation = nebula_core::conversation_id(&a.peer_id(), &b.peer_id());
    let both_blobs_landed = wait_for(CONNECT_TIMEOUT, || async {
        let present = |id: &str| {
            b.db.get_blob(id)
                .ok()
                .flatten()
                .map(|r| r.is_present)
                .unwrap_or(false)
        };
        present(&image_id) && present(&voice_id)
    })
    .await;
    assert!(both_blobs_landed, "Node B never finished CAS downloads");

    assert_eq!(b.blobs.read(&image_id).unwrap(), image, "image bytes must match");
    assert_eq!(b.blobs.read(&voice_id).unwrap(), voice, "voice bytes must match");
    assert!(b.blobs.verify(&image_id).unwrap());
    assert!(b.blobs.verify(&voice_id).unwrap());

    let received = b.db.list_messages(&conversation, 50).unwrap();
    assert!(
        received.iter().any(|m| m.blob_id.as_deref() == Some(&image_id)),
        "image message missing on B"
    );
    assert!(
        received.iter().any(|m| m.blob_id.as_deref() == Some(&voice_id)),
        "voice message missing on B"
    );
    println!(
        "✔ Step 2 CAS: image {} ({} B) and voice {} ({} B) verified on B",
        &image_id[..12],
        image.len(),
        &voice_id[..12],
        voice.len()
    );

    // ------------------------------------------------------------------
    // Step 3 — offline queue + automatic flush on reconnect
    // ------------------------------------------------------------------
    let b_peer_id = b.peer_id();
    b.shutdown().await;
    drop(b);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let offline_text = "Wait for you in Nebula";
    let queued = send_message(&a, &b_peer_id, "text", offline_text, None, None).await;
    assert_eq!(
        queued["message"]["delivery_status"], "queued",
        "message to an offline peer must be queued: {queued}"
    );
    let queued_id = queued["message"]["msg_id"].as_str().unwrap().to_string();

    // Restart Node B from the very same data directory (identity preserved).
    let b = boot(&dir_b, "Beta").await;
    assert_eq!(
        b.identity().peer_id,
        a.db
            .get_peer(&b.peer_id())
            .unwrap()
            .expect("peer row")
            .peer_id,
        "restarted node keeps its PeerID"
    );

    let flushed = wait_for(CONNECT_TIMEOUT, || async {
        let status = a
            .db
            .get_message(&queued_id)
            .ok()
            .flatten()
            .map(|m| m.delivery_status)
            .unwrap_or_default();
        let b_has_it = b
            .db
            .list_messages(&conversation, 200)
            .map(|msgs| msgs.iter().any(|m| m.msg_id == queued_id))
            .unwrap_or(false);
        status == "delivered" && b_has_it
    })
    .await;
    assert!(flushed, "offline message was never flushed after B reconnected");
    println!("✔ Step 3 outbox: queued message auto-delivered after reconnect");

    // ------------------------------------------------------------------
    // Step 4 — moments gossip relay A -> B -> C (A offline)
    // ------------------------------------------------------------------
    let mut mimes = serde_json::Map::new();
    mimes.insert(image_id.clone(), json!("image/png"));
    let moment = api_post(
        &a,
        "/api/local/moments/post",
        json!({
            "content": "Hello Nebula from Alpha",
            "blob_ids": [image_id.clone()],
            "media_meta": {"thumb_b64": "", "width": 1, "height": 1},
            "mimes": Value::Object(mimes)
        }),
    )
    .await;
    let moment_id = moment["event"]["event_id"]
        .as_str()
        .expect("moment id")
        .to_string();

    assert!(
        wait_for(CONNECT_TIMEOUT, || async {
            b.db.list_moment_events()
                .map(|events| events.iter().any(|e| e.event_id == moment_id))
                .unwrap_or(false)
        })
        .await,
        "Node B never received Alpha's moment"
    );

    api_post(
        &b,
        "/api/local/moments/interact",
        json!({"moment_id": moment_id, "event_type": "LIKE"}),
    )
    .await;
    api_post(
        &b,
        "/api/local/moments/interact",
        json!({
            "moment_id": moment_id,
            "event_type": "COMMENT",
            "content": "Awesome Nebula!"
        }),
    )
    .await;

    // Wait until B has the underlying media so it can *relay* the blob to C.
    assert!(
        wait_for(CONNECT_TIMEOUT, || async {
            b.db.get_blob(&image_id)
                .ok()
                .flatten()
                .map(|r| r.is_present)
                .unwrap_or(false)
        })
        .await,
        "Node B never cached the moment image"
    );

    let b_id = b.peer_id();
    let a_nickname = a.identity().nickname;
    a.shutdown().await;
    drop(a);
    tokio::time::sleep(Duration::from_millis(700)).await;

    let c = boot(&dir_c, "Gamma").await;
    assert!(
        wait_for(CONNECT_TIMEOUT, || async { peer_online(&c, &b) }).await,
        "Node C never discovered Node B"
    );

    let relayed = wait_for(CONNECT_TIMEOUT, || async {
        let has_post = c
            .db
            .list_moment_events()
            .map(|events| events.iter().any(|e| e.event_id == moment_id))
            .unwrap_or(false);
        let has_like = c
            .db
            .list_moment_events()
            .map(|events| {
                events.iter().any(|e| {
                    e.moment_id == moment_id && e.event_type == "LIKE" && e.author_id == b_id
                })
            })
            .unwrap_or(false);
        let has_comment = c
            .db
            .list_moment_events()
            .map(|events| {
                events.iter().any(|e| {
                    e.moment_id == moment_id
                        && e.event_type == "COMMENT"
                        && e.content == "Awesome Nebula!"
                })
            })
            .unwrap_or(false);
        let has_blob = c
            .db
            .get_blob(&image_id)
            .ok()
            .flatten()
            .map(|r| r.is_present)
            .unwrap_or(false);
        has_post && has_like && has_comment && has_blob
    })
    .await;
    assert!(
        relayed,
        "Node C is missing the relayed moment, like, comment or blob"
    );

    let views = c.moments_view().unwrap();
    let view = views
        .iter()
        .find(|v| v.post.event_id == moment_id)
        .expect("relayed moment view");
    assert_eq!(view.post.content, "Hello Nebula from Alpha");
    assert_eq!(view.post.author_name, a_nickname);
    assert!(view.like_count >= 1);
    assert!(view.comments.iter().any(|e| e.content == "Awesome Nebula!"));
    assert_eq!(
        c.blobs.read(&image_id).unwrap(),
        image,
        "relayed blob bytes must match Alpha's original"
    );
    println!(
        "✔ Step 4 gossip: C received Alpha's post + like + comment + {} B blob via Beta relay",
        image.len()
    );

    // Best-effort cleanup.
    c.shutdown().await;
    b.shutdown().await;
}
