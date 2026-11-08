//! Axum HTTP surface.
//!
//! * `/api/p2p/*` — node-to-node protocol (reachable on the LAN).
//! * `/api/local/*`, `/ws` — loopback UI API shared by the web bootstrap,
//!   the Tauri desktop shell and (optionally) a browser.
//! * `/` — the responsive single-page UI is embedded at compile time so that
//!   the page and the API are always same-origin (critical on Android WebView).

use crate::db::MessageRecord;
use crate::{NebulaNode, PeerHello, SendMessageRequest, PROTOCOL};
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

const INDEX_HTML: &str = include_str!("../../../ui/dist/index.html");
const STYLES_CSS: &str = include_str!("../../../ui/dist/styles.css");
const APP_JS: &str = include_str!("../../../ui/dist/app.js");

const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Build the full router. Call with `Arc<NebulaNode>`.
pub fn router(node: Arc<NebulaNode>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/styles.css", get(styles))
        .route("/app.js", get(app_js))
        .route("/favicon.ico", get(favicon))
        .route("/ws", get(ws_handler))
        .route("/api/local/bootstrap", get(bootstrap))
        .route(
            "/api/local/messages",
            get(list_messages).post(send_message),
        )
        .route("/api/local/blob", post(upload_blob))
        .route("/api/local/blob/{blob_id}", get(serve_blob))
        .route("/api/local/moments/post", post(post_moment))
        .route("/api/local/moments/interact", post(interact_moment))
        .route("/api/local/profile", put(update_profile))
        .route("/api/p2p/chat", post(p2p_chat))
        .route("/api/p2p/sync", post(p2p_sync))
        .route("/api/p2p/blob/{blob_id}", get(serve_blob))
        .fallback(fallback_static)
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .layer(CorsLayer::permissive())
        .with_state(node)
}

/// Serve a bound listener until the node shuts down.
pub async fn serve(app: Router, listener: TcpListener, node: Arc<NebulaNode>) {
    let mut rx = node.subscribe_shutdown();
    let shutdown = async move {
        if *rx.borrow() {
            return;
        }
        let _ = rx.changed().await;
    };
    if let Err(err) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
    {
        tracing::error!("http server terminated: {err:#}");
    }
}

// ----------------------------------------------------------------------
// Errors
// ----------------------------------------------------------------------

pub struct ApiError(StatusCode, String);

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
    fn not_found(message: impl Into<String>) -> Self {
        Self(StatusCode::NOT_FOUND, message.into())
    }
    fn internal(message: impl Into<String>) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, message.into())
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        Self::internal(format!("{err:#}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({"ok": false, "error": self.1})),
        )
            .into_response()
    }
}

// ----------------------------------------------------------------------
// Static UI
// ----------------------------------------------------------------------

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn styles() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        STYLES_CSS,
    )
}

async fn app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
        APP_JS,
    )
}

async fn favicon() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn fallback_static(uri: Uri) -> Response {
    if uri.path().starts_with("/api/") {
        (StatusCode::NOT_FOUND, "not found").into_response()
    } else {
        Html(INDEX_HTML).into_response()
    }
}

// ----------------------------------------------------------------------
// Local UI API
// ----------------------------------------------------------------------

async fn bootstrap(State(node): State<Arc<NebulaNode>>) -> Result<Json<Value>, ApiError> {
    Ok(Json(node.bootstrap()?))
}

#[derive(Debug, Deserialize)]
struct MessageQuery {
    #[serde(default)]
    conversation_id: Option<String>,
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn list_messages(
    State(node): State<Arc<NebulaNode>>,
    Query(query): Query<MessageQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = query.limit.unwrap_or(300).min(2000);
    let messages = match query.search.as_deref().map(str::trim) {
        Some(term) if !term.is_empty() => node.db.search_messages(term, limit)?,
        _ => {
            let conv = query
                .conversation_id
                .as_deref()
                .unwrap_or("lobby")
                .to_string();
            node.db.list_messages(&conv, limit)?
        }
    };
    Ok(Json(json!({"ok": true, "messages": messages})))
}

async fn send_message(
    State(node): State<Arc<NebulaNode>>,
    Json(body): Json<SendMessageRequest>,
) -> Result<Json<Value>, ApiError> {
    let message = node.send_message(body).await?;
    Ok(Json(json!({"ok": true, "message": message})))
}

#[derive(Debug, Deserialize)]
struct BlobJsonBody {
    data_b64: String,
    #[serde(default)]
    mime_type: Option<String>,
}

async fn upload_blob(
    State(node): State<Arc<NebulaNode>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    if body.is_empty() {
        return Err(ApiError::bad_request("empty upload"));
    }
    let header_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let (bytes, mime) = if header_type.starts_with("application/json") {
        let parsed: BlobJsonBody = serde_json::from_slice(&body)
            .map_err(|e| ApiError::bad_request(format!("invalid blob JSON: {e}")))?;
        let raw = parsed.data_b64.rsplit(',').next().unwrap_or_default();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(raw.trim())
            .map_err(|e| ApiError::bad_request(format!("invalid base64: {e}")))?;
        (
            decoded,
            parsed
                .mime_type
                .unwrap_or_else(|| "application/octet-stream".to_string()),
        )
    } else {
        (body.to_vec(), header_type)
    };

    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(ApiError::bad_request("upload too large"));
    }
    let (blob_id, size) = node.store_blob(&bytes, &mime)?;
    Ok(Json(json!({
        "ok": true,
        "blob_id": blob_id,
        "mime_type": mime,
        "size_bytes": size
    })))
}

async fn serve_blob(
    State(node): State<Arc<NebulaNode>>,
    Path(blob_id): Path<String>,
) -> Result<Response, ApiError> {
    let record = node
        .get_blob_record(&blob_id)?
        .ok_or_else(|| ApiError::not_found("unknown blob"))?;
    if !node.blobs.has(&blob_id) {
        return Err(ApiError::not_found("blob not downloaded yet"));
    }
    let bytes = node.blobs.read(&blob_id)?;
    let mime = if record.mime_type.trim().is_empty() {
        "application/octet-stream".to_string()
    } else {
        record.mime_type
    };
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            (
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable".to_string(),
            ),
        ],
        bytes,
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
struct MomentPostBody {
    #[serde(default)]
    content: String,
    #[serde(default)]
    blob_ids: Vec<String>,
    #[serde(default)]
    media_meta: Option<Value>,
    /// Optional `{ "<blob_id>": "<mime>" }` map produced by the UI.
    #[serde(default)]
    mimes: Option<Value>,
}

async fn post_moment(
    State(node): State<Arc<NebulaNode>>,
    Json(body): Json<MomentPostBody>,
) -> Result<Json<Value>, ApiError> {
    if body.blob_ids.len() > 9 {
        return Err(ApiError::bad_request("a moment supports at most 9 media items"));
    }
    if body.content.trim().is_empty() && body.blob_ids.is_empty() {
        return Err(ApiError::bad_request("moment needs text or media"));
    }
    let event = node.create_moment_post(
        body.content.trim(),
        body.blob_ids,
        body.media_meta,
        body.mimes,
    )?;
    Ok(Json(json!({"ok": true, "event": event})))
}

#[derive(Debug, Deserialize)]
struct MomentInteractBody {
    moment_id: String,
    event_type: String,
    #[serde(default)]
    content: String,
}

async fn interact_moment(
    State(node): State<Arc<NebulaNode>>,
    Json(body): Json<MomentInteractBody>,
) -> Result<Json<Value>, ApiError> {
    if body.moment_id.trim().is_empty() {
        return Err(ApiError::bad_request("moment_id is required"));
    }
    let event = node.create_moment_interaction(
        body.moment_id.trim(),
        &body.event_type,
        body.content.trim(),
    )?;
    Ok(Json(json!({"ok": true, "event": event})))
}

#[derive(Debug, Deserialize)]
struct ProfileBody {
    nickname: String,
    #[serde(default)]
    bio: String,
}

async fn update_profile(
    State(node): State<Arc<NebulaNode>>,
    Json(body): Json<ProfileBody>,
) -> Result<Json<Value>, ApiError> {
    let identity = node.update_profile(&body.nickname, &body.bio)?;
    Ok(Json(json!({"ok": true, "identity": identity})))
}

// ----------------------------------------------------------------------
// P2P API
// ----------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct P2pChatBody {
    message: MessageRecord,
    #[serde(default)]
    sender_peer: Option<PeerHello>,
}

async fn p2p_chat(
    State(node): State<Arc<NebulaNode>>,
    Json(body): Json<P2pChatBody>,
) -> Result<Json<Value>, ApiError> {
    let me = node.peer_id();
    if let Some(hello) = &body.sender_peer {
        if hello.peer_id != me {
            node.db
                .update_peer_endpoint(&hello.peer_id, hello.p2p_port, crate::now_ms())?;
        }
    }
    if body.message.sender_id != me {
        node.ingest_message(&body.message)?;
    }
    Ok(Json(json!({
        "ok": true,
        "ack_msg_id": body.message.msg_id,
        "receiver_id": me
    })))
}

#[derive(Debug, Deserialize)]
struct SyncBody {
    #[serde(default)]
    sender_peer: Option<PeerHello>,
    #[serde(default)]
    queued_messages: Vec<MessageRecord>,
    #[serde(default)]
    known_moment_clocks: HashMap<String, i64>,
}

async fn p2p_sync(
    State(node): State<Arc<NebulaNode>>,
    Json(body): Json<SyncBody>,
) -> Result<Json<Value>, ApiError> {
    let me = node.peer_id();
    if let Some(hello) = &body.sender_peer {
        if hello.peer_id != me {
            node.db
                .update_peer_endpoint(&hello.peer_id, hello.p2p_port, crate::now_ms())?;
        }
    }

    let mut acked = Vec::with_capacity(body.queued_messages.len());
    for message in &body.queued_messages {
        // Ignore crafted messages claiming to be authored by us.
        if message.sender_id != me {
            node.ingest_message(message)?;
        }
        acked.push(message.msg_id.clone());
    }

    let missing_moment_events = node.db.events_missing_for(&body.known_moment_clocks)?;
    let missing_lobby_messages = node.db.recent_lobby_messages(100)?;

    Ok(Json(json!({
        "ok": true,
        "acked_msg_ids": acked,
        "missing_moment_events": missing_moment_events,
        "missing_lobby_messages": missing_lobby_messages,
        "known_moment_clocks": node.db.known_moment_clocks()?,
        "protocol": PROTOCOL
    })))
}

// ----------------------------------------------------------------------
// WebSocket bus
// ----------------------------------------------------------------------

async fn ws_handler(
    State(node): State<Arc<NebulaNode>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, node))
}

async fn handle_socket(mut socket: WebSocket, node: Arc<NebulaNode>) {
    let mut rx = node.subscribe_events();
    let hello = json!({
        "type": "HELLO",
        "identity": node.identity(),
        "node": {
            "p2p_port": node.p2p_port,
            "ui_port": node.ui_port,
            "protocol": PROTOCOL
        }
    });
    if socket
        .send(Message::Text(hello.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(value) => {
                        if socket.send(Message::Text(value.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!("websocket lagged by {n} events");
                    }
                    Err(_) => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let text = text.as_str();
                        if text == "ping" {
                            let _ = socket.send(Message::Text("pong".into())).await;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = socket.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}
