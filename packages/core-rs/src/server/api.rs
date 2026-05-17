use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use std::sync::Arc;
use tracing::info;

use crate::types::{ScanMeta, TaskStatus, WorkerStatus, WsEvent};

use super::AppState;

/// GET /api/reports — return all completed route reports
pub async fn get_reports(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let reports = state.route_reports.read().await;
    let completed: Vec<_> = reports
        .values()
        .filter(|r| r.tasks.inspect_html_task == TaskStatus::Completed)
        .cloned()
        .collect();
    Json(completed)
}

/// GET /api/scan-meta — return current scan statistics
pub async fn get_scan_meta(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let reports = state.route_reports.read().await;
    let total = reports.len();
    let scores: Vec<f64> = reports
        .values()
        .filter_map(|r| r.report.as_ref().map(|rep| rep.score))
        .collect();
    let avg_score = if scores.is_empty() {
        0.0
    } else {
        scores.iter().sum::<f64>() / scores.len() as f64
    };

    // Build stats snapshot
    let done = reports
        .values()
        .filter(|r| r.tasks.run_lighthouse_task == TaskStatus::Completed)
        .count();

    let all_done = done == total && total > 0;
    let monitor = if total > 0 {
        Some(crate::types::WorkerStats {
            status: if all_done {
                WorkerStatus::Completed
            } else {
                WorkerStatus::Working
            },
            done_targets: done,
            all_targets: total,
            done_perc_str: format!(
                "{:.0}",
                if total > 0 { done as f64 * 100.0 / total as f64 } else { 0.0 }
            ),
            error_perc: "0.00".to_string(),
            time_running: 0,
            time_remaining: -1,
            pages_per_second: "0.00".to_string(),
            workers: state.config.workers,
        })
    } else {
        None
    };

    Json(ScanMeta {
        routes: total,
        score: avg_score,
        monitor,
    })
}

/// POST /api/reports/rescan — clear all reports and re-queue everything
pub async fn rescan_all(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut reports = state.route_reports.write().await;
    let count = reports.len();
    info!("Rescan all: clearing {count} reports");

    // Collect routes to re-queue
    let routes: Vec<_> = reports.values().map(|r| r.route.clone()).collect();
    reports.clear();
    drop(reports);

    // Re-queue via the work channel
    for route in routes {
        let _ = state.work_tx.send(route).await;
    }

    StatusCode::OK
}

/// POST /api/reports/:id/rescan — re-queue a single report
pub async fn rescan_one(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let mut reports = state.route_reports.write().await;
    if let Some(report) = reports.remove(&id) {
        info!("Rescan: re-queuing {}", report.route.path);
        let route = report.route.clone();
        drop(reports);
        let _ = state.work_tx.send(route).await;
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

/// GET /api/crux/*site — proxy to crux.unlighthouse.dev
pub async fn get_crux_history(
    Path(site): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // Axum's wildcard matcher decodes percent encoding, so we get something like:
    // "https:/www.calljacob.com/history" or "https://www.calljacob.com/history"
    // Let's strip the "/history" suffix.
    let site = site.strip_suffix("/history").unwrap_or(&site).to_string();

    // Reconstruct consecutive slashes if they were collapsed during routing/decoding
    let site = if site.starts_with("https:/") && !site.starts_with("https://") {
        site.replacen("https:/", "https://", 1)
    } else if site.starts_with("http:/") && !site.starts_with("http://") {
        site.replacen("http:/", "http://", 1)
    } else {
        site
    };

    // Percent-encode the site URL for the target API request
    let encoded_site = urlencoding::encode(&site);
    let url = format!("https://crux.unlighthouse.dev/api/{}/crux/history", encoded_site);
    info!("Proxying CrUX request for: {} (encoded: {})", site, encoded_site);

    match state.http_client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                let body = resp.bytes().await.unwrap_or_default();
                (status, body).into_response()
            } else {
                info!("CrUX proxy returned non-success code {} for site: {}. Gracefully returning empty crux data JSON.", status, site);
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"exists":false}"#))
                    .unwrap()
            }
        }
        Err(e) => {
            tracing::error!("CrUX proxy error: {}", e);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"exists":false}"#))
                .unwrap()
        }
    }
}

/// Broadcast a WsEvent to all WS subscribers and update scan meta.
pub async fn broadcast_and_update(state: &Arc<AppState>, event: WsEvent) {
    super::websocket::broadcast(&state.ws_tx, &event);

    // Also broadcast updated scan-meta
    let reports = state.route_reports.read().await;
    let total = reports.len();
    let scores: Vec<f64> = reports
        .values()
        .filter_map(|r| r.report.as_ref().map(|rep| rep.score))
        .collect();
    drop(reports);

    let avg_score = if scores.is_empty() {
        0.0
    } else {
        scores.iter().sum::<f64>() / scores.len() as f64
    };

    let meta_event = WsEvent::ScanMetaUpdate(ScanMeta {
        routes: total,
        score: avg_score,
        monitor: None,
    });
    super::websocket::broadcast(&state.ws_tx, &meta_event);
}
