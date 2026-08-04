use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Clone)]
struct RelayState {
    sessions: Arc<RwLock<HashMap<String, Vec<mpsc::UnboundedSender<String>>>>>,
    buffers: Arc<RwLock<HashMap<String, VecDeque<RelayNotify>>>>,
    relay_token: String,
    max_buffer_per_account: usize,
}

#[derive(Debug, Deserialize)]
struct WsQuery {
    account: String,
    token: String,
}

#[derive(Debug, Deserialize)]
struct PubSubPushRequest {
    message: PubSubMessage,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PubSubMessage {
    data: String,
    message_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GmailNotification {
    email_address: String,
    history_id: String,
}

#[derive(Debug, Clone, Serialize)]
struct RelayNotify {
    #[serde(rename = "type")]
    kind: &'static str,
    notification_id: String,
    account_email: String,
    history_id: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let bind = std::env::var("RELAY_BIND").unwrap_or_else(|_| "127.0.0.1:8787".to_string());
    let relay_token = std::env::var("RELAY_TOKEN").unwrap_or_default();
    let max_buffer_per_account = std::env::var("RELAY_MAX_BUFFER")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(200);

    let state = RelayState {
        sessions: Arc::new(RwLock::new(HashMap::new())),
        buffers: Arc::new(RwLock::new(HashMap::new())),
        relay_token,
        max_buffer_per_account,
    };

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/ws", get(ws_handler))
        .route("/pubsub/push", post(pubsub_push))
        .with_state(state);

    let addr: SocketAddr = bind.parse().expect("invalid RELAY_BIND");
    info!("relay listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind failed");
    axum::serve(listener, app).await.expect("server failed");
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
    State(state): State<RelayState>,
) -> Result<impl IntoResponse, StatusCode> {
    if !is_authorized(&state.relay_token, &query.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(ws.on_upgrade(move |socket| handle_socket(socket, query.account, state)))
}

async fn handle_socket(mut socket: WebSocket, account_email: String, state: RelayState) {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    {
        let mut sessions = state.sessions.write().await;
        sessions.entry(account_email.clone()).or_default().push(tx);
    }

    if let Some(buffered) = state.buffers.write().await.get_mut(&account_email) {
        for msg in buffered.iter() {
            if let Ok(payload) = serde_json::to_string(msg) {
                if socket.send(Message::Text(payload)).await.is_err() {
                    return;
                }
            }
        }
    }

    loop {
        tokio::select! {
            maybe_payload = rx.recv() => {
                let payload = match maybe_payload {
                    Some(payload) => payload,
                    None => break,
                };
                if socket.send(Message::Text(payload)).await.is_err() {
                    break;
                }
            }
            maybe_msg = socket.next() => {
                match maybe_msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        warn!("websocket read error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn pubsub_push(
    State(state): State<RelayState>,
    headers: HeaderMap,
    Json(body): Json<PubSubPushRequest>,
) -> Result<StatusCode, StatusCode> {
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if auth.starts_with("Bearer ") {
            let token = auth.trim_start_matches("Bearer ").trim();
            if !is_authorized(&state.relay_token, token) {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    }

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(body.message.data.as_bytes())
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let notif: GmailNotification =
        serde_json::from_slice(&decoded).map_err(|_| StatusCode::BAD_REQUEST)?;

    let relay = RelayNotify {
        kind: "NOTIFY",
        notification_id: body
            .message
            .message_id
            .unwrap_or_else(|| Uuid::new_v4().to_string()),
        account_email: notif.email_address,
        history_id: notif.history_id,
    };

    deliver(state, relay).await;
    Ok(StatusCode::OK)
}

async fn deliver(state: RelayState, notif: RelayNotify) {
    let payload = match serde_json::to_string(&notif) {
        Ok(payload) => payload,
        Err(e) => {
            error!("failed to serialize relay notification: {}", e);
            return;
        }
    };

    let mut delivered = 0usize;
    {
        let mut sessions = state.sessions.write().await;
        if let Some(peers) = sessions.get_mut(&notif.account_email) {
            peers.retain(|peer| match peer.send(payload.clone()) {
                Ok(()) => {
                    delivered += 1;
                    true
                }
                Err(_) => false,
            });
        }
    }

    if delivered == 0 {
        let mut buffers = state.buffers.write().await;
        let queue = buffers.entry(notif.account_email.clone()).or_default();
        queue.push_back(notif);
        while queue.len() > state.max_buffer_per_account {
            queue.pop_front();
        }
    }
}

fn is_authorized(expected: &str, provided: &str) -> bool {
    expected.is_empty() || expected == provided
}
