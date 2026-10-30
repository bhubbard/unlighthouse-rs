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
                if sender.send(Message::Text(json)).await.is_err() {
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
                    if sender.send(Message::Text(msg)).await.is_err() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ScanMeta;

    #[test]
    fn test_ws_broadcast_delivers_serialized_event() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        let event = WsEvent::ScanMetaUpdate(ScanMeta {
            routes: 3,
            score: 0.91,
            monitor: None,
        });

        broadcast(&tx, &event);
        let msg = rx.try_recv().unwrap();
        let val: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(val["event"], "scan-meta-update");
        assert_eq!(val["data"]["routes"], 3);
    }
}
