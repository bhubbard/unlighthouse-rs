use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::types::WsEvent;

/// Shared WebSocket broadcast channel handle.
pub type WsBroadcast = tokio::sync::broadcast::Sender<String>;

/// Axum handler: upgrades the connection and subscribes to broadcast events.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<super::AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<super::AppState>) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.ws_tx.subscribe();

    // Send current snapshot of all reports on connect
    {
        let reports = state.route_reports.read().await;
        for report in reports.values() {
            let event = WsEvent::TaskComplete(report.clone());
            if let Ok(json) = serde_json::to_string(&event) {
                if sender.send(Message::Text(json.into())).await.is_err() {
                    return;
                }
            }
        }
    }

    // Forward broadcast messages to this client
    let send_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if sender.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("WS client lagged by {n} messages");
                }
            }
        }
    });

    // Drain incoming messages (we don't use client→server messages)
    while receiver.next().await.is_some() {
        debug!("WS incoming message (ignored)");
    }

    send_task.abort();
}

/// Broadcast a WsEvent to all connected WebSocket clients.
pub fn broadcast(tx: &WsBroadcast, event: &WsEvent) {
    if let Ok(json) = serde_json::to_string(event) {
        // Ignore send errors (no listeners)
        let _ = tx.send(json);
    }
}
