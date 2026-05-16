use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
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
