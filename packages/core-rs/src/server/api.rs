use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use std::collections::HashMap as QueryMap;
use std::sync::Arc;
use tracing::info;

use crate::types::{ScanMeta, TaskStatus, WorkerStatus, WsEvent};

use super::AppState;

// ── CrUX types ────────────────────────────────────────────────────────────────

/// Mirrors the CrUX History API response shape (only the fields we use).
#[derive(serde::Deserialize)]
struct CruxHistoryResponse {
    record: CruxRecord,
}

#[derive(serde::Deserialize)]
struct CruxRecord {
    metrics: CruxMetrics,
    #[serde(rename = "collectionPeriods")]
    collection_periods: Vec<CollectionPeriod>,
}

#[derive(serde::Deserialize)]
struct CruxMetrics {
    #[serde(default)]
    largest_contentful_paint: Option<CruxMetric>,
    #[serde(default)]
    cumulative_layout_shift: Option<CruxMetric>,
    #[serde(default)]
    interaction_to_next_paint: Option<CruxMetric>,
}

#[derive(serde::Deserialize)]
struct CruxMetric {
    #[serde(rename = "percentilesTimeseries")]
    percentiles_timeseries: PercentilesTimeseries,
}

#[derive(serde::Deserialize)]
struct PercentilesTimeseries {
    p75s: Vec<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct CollectionPeriod {
    #[serde(rename = "firstDate")]
    first_date: CruxDate,
}

#[derive(serde::Deserialize)]
struct CruxDate {
    year: i32,
    month: u32,
    day: u32,
}

#[derive(serde::Serialize)]
struct DataPoint {
    value: f64,
    time: i64,
}

/// Port of crux-api's `normaliseCruxHistory`. Produces `{ dates, cls, lcp, inp }`.
fn normalise_crux_history(record: CruxRecord) -> serde_json::Value {
    // Build timestamp array from collection periods (month is 1-based from the API).
    let dates: Vec<i64> = record
        .collection_periods
        .iter()
        .filter_map(|p| {
            chrono::NaiveDate::from_ymd_opt(p.first_date.year, p.first_date.month, p.first_date.day)
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|dt| dt.and_utc().timestamp_millis())
        })
        .collect();

    let parse_p75 = |metric: &Option<CruxMetric>| -> Vec<DataPoint> {
        let p75s = match metric {
            Some(m) => &m.percentiles_timeseries.p75s,
            None => return vec![],
        };
        p75s.iter()
            .enumerate()
            .map(|(i, v)| DataPoint {
                value: v.as_f64().unwrap_or(0.0),
                time: dates.get(i).copied().unwrap_or(0),
            })
            .collect()
    };

    let cls = parse_p75(&record.metrics.cumulative_layout_shift);
    let lcp = parse_p75(&record.metrics.largest_contentful_paint);
    let inp = parse_p75(&record.metrics.interaction_to_next_paint);

    // Find the first index where each series has a meaningful value.
    let cls_start = cls.iter().position(|p| p.value >= 0.0);
    let lcp_start = lcp.iter().position(|p| p.value > 0.0);
    let inp_start = inp.iter().position(|p| p.value > 0.0);

    let indexes: Vec<usize> = [cls_start, lcp_start, inp_start]
        .iter()
        .flatten()
        .copied()
        .collect();

    if indexes.is_empty() {
        return serde_json::json!({ "dates": [], "cls": null, "lcp": null, "inp": null });
    }

    let start = *indexes.iter().min().unwrap();

    // Find the last index that has a meaningful value in any series.
    let cls_end = cls.iter().rposition(|p| p.value >= 0.0).unwrap_or(0);
    let lcp_end = lcp.iter().rposition(|p| p.value > 0.0).unwrap_or(0);
    let inp_end = inp.iter().rposition(|p| p.value > 0.0).unwrap_or(0);
    let end = [cls_end, lcp_end, inp_end].iter().copied().max().unwrap_or(0);

    // Guard against degenerate data that would cause a slice panic.
    let end = end.min(dates.len());
    if start >= end {
        return serde_json::json!({ "dates": [], "cls": null, "lcp": null, "inp": null });
    }

    let sliced_dates: Vec<i64> = dates[start..end].to_vec();
    let cls_out: Option<Vec<&DataPoint>> = cls_start.map(|_| cls[start..end.min(cls.len())].iter().collect());
    let lcp_out: Option<Vec<&DataPoint>> = lcp_start.map(|_| lcp[start..end.min(lcp.len())].iter().collect());
    let inp_out: Option<Vec<&DataPoint>> = inp_start.map(|_| inp[start..end.min(inp.len())].iter().collect());

    serde_json::json!({
        "dates": sliced_dates,
        "cls":   cls_out,
        "lcp":   lcp_out,
        "inp":   inp_out,
    })
}

/// Call the Google CrUX History API directly and return normalised JSON.
async fn fetch_crux_direct(
    client: &reqwest::Client,
    token: &str,
    origin: &str,
) -> Result<serde_json::Value, String> {
    let url = "https://chromeuxreport.googleapis.com/v1/records:queryHistoryRecord";
    // Ensure origin has https scheme and trailing slash (mirrors crux-api's withHttps + withTrailingSlash).
    let origin_url = if origin.starts_with("http://") || origin.starts_with("https://") {
        format!("{}/", origin.trim_end_matches('/'))
    } else {
        format!("https://{}/", origin.trim_end_matches('/'))
    };

    let resp = client
        .post(url)
        .query(&[("key", token)])
        .json(&serde_json::json!({ "origin": origin_url, "formFactor": "PHONE" }))
        .send()
        .await
        .map_err(|e| format!("CrUX request failed: {e}"))?;

    let status = resp.status();

    // 404 → no data for this origin (not an error).
    if status.as_u16() == 404 {
        return Ok(serde_json::json!({ "exists": false }));
    }

    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("CrUX API error {status}: {body}"));
    }

    let crux: CruxHistoryResponse = resp
        .json()
        .await
        .map_err(|e| format!("CrUX parse error: {e}"))?;

    let normalised = normalise_crux_history(crux.record);

    // If normalisation produced no dates, surface as { exists: false }.
    if normalised["dates"].as_array().map(|a| a.is_empty()).unwrap_or(true) {
        return Ok(serde_json::json!({ "exists": false }));
    }

    Ok(normalised)
}

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

/// GET /api/crux/*site — fetch CrUX history data.
///
/// Calls the Google CrUX History API directly and normalises the response.
/// Requires a configured `crux_api_token`.
pub async fn get_crux_history(
    Path(site): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // Axum's wildcard matcher decodes percent encoding, so we may receive:
    //   "https:/www.example.com/history" (single slash, double-slash collapsed)
    // Strip the trailing "/history" segment first.
    let site = site.strip_suffix("/history").unwrap_or(&site).to_string();

    // Restore any double-slash that the router collapsed.
    let site = if site.starts_with("https:/") && !site.starts_with("https://") {
        site.replacen("https:/", "https://", 1)
    } else if site.starts_with("http:/") && !site.starts_with("http://") {
        site.replacen("http:/", "http://", 1)
    } else {
        site
    };

    let empty_json = r#"{"exists":false}"#;

    // ── Direct path: call Google CrUX API ────────────────────────────────────
    if let Some(ref token) = state.config.crux_api_token {
        info!("Fetching CrUX data directly for: {}", site);
        return match fetch_crux_direct(&state.http_client, token, &site).await {
            Ok(data) => {
                let body = serde_json::to_string(&data).unwrap_or_else(|_| empty_json.to_string());
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap()
            }
            Err(e) => {
                tracing::warn!("CrUX direct fetch failed for {}: {}", site, e);
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(empty_json))
                    .unwrap()
            }
        };
    }

    tracing::debug!(
        "Google CrUX API token is not configured; skipping CrUX fetch for: {}",
        site
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(empty_json))
        .unwrap()
}

// ── Historical score trending ─────────────────────────────────────────────────

/// GET /api/runs — list the most recent 50 scan runs for this site, newest first.
///
/// Each entry contains the run ID, timestamps, scan mode and route count so the
/// client can display a run-picker or label the X-axis of a trend chart.
pub async fn list_runs(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match crate::db::list_runs(&state.db, &state.config.site, 50).await {
        Ok(runs) => Json(runs).into_response(),
        Err(e) => {
            tracing::error!("DB error listing runs: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// GET /api/runs/:run_id/scores — all per-route scores for one historical run.
///
/// Returns an array of `RouteScoreRecord` objects (one per route), sorted by
/// path.  Scores are in the 0.0–1.0 range (multiply by 100 for display).
pub async fn get_run_scores(
    Path(run_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match crate::db::get_run_scores(&state.db, &run_id).await {
        Ok(scores) => Json(scores).into_response(),
        Err(e) => {
            tracing::error!("DB error fetching run scores for {run_id}: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// GET /api/history/route?path=/some/page — score trend for one route path.
///
/// Returns an array of `RouteTrendPoint` objects ordered oldest-to-newest,
/// each containing the run ID, its `started_at` timestamp, and all numeric
/// score/metric fields.  Only finished runs are included.
///
/// The dashboard can plot these directly as a time-series (the recharts shape
/// already used for CrUX history is compatible).
pub async fn get_route_history(
    Query(params): Query<QueryMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let path = match params.get("path") {
        Some(p) => p.clone(),
        None => {
            return (StatusCode::BAD_REQUEST, "Missing ?path= query parameter").into_response();
        }
    };

    match crate::db::get_route_trend(&state.db, &state.config.site, &path).await {
        Ok(trend) => Json(trend).into_response(),
        Err(e) => {
            tracing::error!("DB error fetching trend for {path}: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────

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
