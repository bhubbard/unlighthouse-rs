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

    // Find the last index that has a meaningful value in any series (exclusive upper bound = idx + 1).
    let cls_end = cls.iter().rposition(|p| p.value >= 0.0).map(|i| i + 1).unwrap_or(0);
    let lcp_end = lcp.iter().rposition(|p| p.value > 0.0).map(|i| i + 1).unwrap_or(0);
    let inp_end = inp.iter().rposition(|p| p.value > 0.0).map(|i| i + 1).unwrap_or(0);
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

fn build_monitor_stats(
    state: &Arc<AppState>,
    reports: &std::collections::HashMap<String, crate::types::RouteReport>,
) -> Option<crate::types::WorkerStats> {
    let total = reports.len();
    if total == 0 {
        return None;
    }
    
    let done = reports
        .values()
        .filter(|r| {
            matches!(
                r.tasks.run_lighthouse_task,
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Ignore
            )
        })
        .count();

    let failed = reports
        .values()
        .filter(|r| {
            r.tasks.run_lighthouse_task == TaskStatus::Failed
                || r.tasks.inspect_html_task == TaskStatus::Failed
        })
        .count();

    let all_done = done == total;
    
    let elapsed = state.start_time.elapsed().as_secs();
    let pps = if elapsed > 0 {
        done as f64 / elapsed as f64
    } else {
        0.0
    };
    let pages_per_second = format!("{:.2}", pps);

    let time_remaining = if all_done {
        0
    } else if done > 0 && pps > 0.0 {
        let remaining_targets = total - done;
        (remaining_targets as f64 / pps).round() as i64
    } else {
        -1
    };

    let active = if let Some(ref pool) = state.lighthouse_pool {
        pool.active_workers()
    } else {
        0
    };
    let cpu_pct = if state.config.workers > 0 {
        (active as f64 * 100.0 / state.config.workers as f64).round() as usize
    } else {
        0
    };
    let cpu_usage = Some(format!("{}% ({} / {} cores)", cpu_pct, active, state.config.workers));

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
            done as f64 * 100.0 / total as f64
        ),
        error_perc: format!("{:.2}", failed as f64 * 100.0 / total as f64),
        time_running: elapsed,
        time_remaining,
        pages_per_second,
        workers: state.config.workers,
        cpu_usage,
    })
}

/// Compute the current [`ScanMeta`] snapshot across all routes (supports both full and fast modes).
pub async fn compute_scan_meta(state: &Arc<AppState>) -> ScanMeta {
    let reports = state.route_reports.read().await;
    let total = reports.len();
    let scores: Vec<f64> = reports
        .values()
        .filter_map(|r| {
            r.report
                .as_ref()
                .map(|rep| rep.score)
                .or_else(|| r.web_vitals.as_ref().map(|wv| wv.score))
        })
        .collect();
    let avg_score = if scores.is_empty() {
        0.0
    } else {
        scores.iter().sum::<f64>() / scores.len() as f64
    };

    let monitor = build_monitor_stats(state, &reports);
    drop(reports);

    ScanMeta {
        routes: total,
        score: avg_score,
        monitor,
    }
}

/// GET /api/scan-meta — return current scan statistics
pub async fn get_scan_meta(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(compute_scan_meta(&state).await)
}

/// POST /api/reports/rescan — clear all reports and re-queue everything
pub async fn rescan_all(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut reports = state.route_reports.write().await;
    let count = reports.len();
    info!("Rescan all: clearing {count} reports");

    // Collect routes and artifact paths to invalidate cached report.json before re-queuing
    let items: Vec<_> = reports
        .values()
        .map(|r| (r.route.clone(), r.artifact_path.clone()))
        .collect();
    reports.clear();
    drop(reports);

    // Remove cached report.json and re-queue via the work channel
    for (route, artifact_path) in items {
        let cached_report = std::path::PathBuf::from(&artifact_path).join("report.json");
        let _ = tokio::fs::remove_file(cached_report).await;
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
        let artifact_path = report.artifact_path.clone();
        drop(reports);
        let cached_report = std::path::PathBuf::from(&artifact_path).join("report.json");
        let _ = tokio::fs::remove_file(cached_report).await;
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
    let monitor = build_monitor_stats(state, &reports);
    drop(reports);

    let avg_score = if scores.is_empty() {
        0.0
    } else {
        scores.iter().sum::<f64>() / scores.len() as f64
    };

    let meta_event = WsEvent::ScanMetaUpdate(ScanMeta {
        routes: total,
        score: avg_score,
        monitor,
    });
    super::websocket::broadcast(&state.ws_tx, &meta_event);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::types::{
        LighthouseCategoryScore, LighthouseReport, NormalisedRoute, RouteDefinition, RouteReport,
        TaskMap,
    };
    use axum::{
        body::to_bytes,
        routing::{get, post},
        Router,
    };
    use serde_json::json;
    use std::collections::HashMap;
    use tower::ServiceExt;

    async fn test_app_state() -> (Arc<AppState>, tokio::sync::mpsc::Receiver<NormalisedRoute>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("api_test.db");
        let db = crate::db::open(db_path.to_str().unwrap()).await.unwrap();
        crate::db::insert_run(&db, "run-1", "https://example.com", "full")
            .await
            .unwrap();

        let config = Arc::new(Config {
            site: "https://example.com".to_string(),
            output_path: dir.path().to_str().unwrap().to_string(),
            workers: 2,
            ..Default::default()
        });
        let (work_tx, work_rx) = tokio::sync::mpsc::channel(32);
        let state = Arc::new(AppState::new(
            config,
            work_tx,
            None,
            db,
            "run-1".to_string(),
        ));
        (state, work_rx, dir)
    }

    fn make_report(id: &str, path: &str, score: f64, done: bool) -> RouteReport {
        let mut categories = HashMap::new();
        categories.insert(
            "performance".to_string(),
            LighthouseCategoryScore {
                score: Some(score),
                title: None,
                key: None,
            },
        );
        RouteReport {
            report_id: id.to_string(),
            route: NormalisedRoute {
                id: id.to_string(),
                path: path.to_string(),
                url: format!("https://example.com{path}"),
                definition: RouteDefinition {
                    name: path.to_string(),
                    path: path.to_string(),
                },
            },
            artifact_path: format!("/tmp/{id}"),
            artifact_url: format!("/{id}/"),
            tasks: TaskMap {
                inspect_html_task: TaskStatus::Completed,
                run_lighthouse_task: if done {
                    TaskStatus::Completed
                } else {
                    TaskStatus::InProgress
                },
            },
            report: Some(LighthouseReport {
                score,
                categories,
                audits: serde_json::Value::Null,
            }),
            seo: None,
            web_vitals: None,
        }
    }

    #[test]
    fn test_normalise_crux_history_includes_last_period() {
        let raw_json = json!({
            "record": {
                "collectionPeriods": [
                    { "firstDate": { "year": 2026, "month": 10, "day": 1 } },
                    { "firstDate": { "year": 2026, "month": 10, "day": 8 } }
                ],
                "metrics": {
                    "largest_contentful_paint": {
                        "percentilesTimeseries": { "p75s": [1800.0, 1750.0] }
                    },
                    "cumulative_layout_shift": {
                        "percentilesTimeseries": { "p75s": [0.05, 0.04] }
                    }
                }
            }
        });

        let parsed: CruxHistoryResponse = serde_json::from_value(raw_json).unwrap();
        let norm = normalise_crux_history(parsed.record);
        let dates = norm["dates"].as_array().unwrap();
        assert_eq!(dates.len(), 2);
        let lcp = norm["lcp"].as_array().unwrap();
        assert_eq!(lcp.len(), 2);
        assert_eq!(lcp[0]["value"], 1800.0);
        assert_eq!(lcp[1]["value"], 1750.0);
        assert!(norm["inp"].is_null());
    }

    #[tokio::test]
    async fn test_api_endpoints_reports_meta_rescan_and_history() {
        let (state, mut work_rx, _dir) = test_app_state().await;

        let r1 = make_report("id1", "/", 0.90, true);
        let r2 = make_report("id2", "/about", 0.80, false);

        {
            let mut map = state.route_reports.write().await;
            map.insert("id1".to_string(), r1.clone());
            map.insert("id2".to_string(), r2.clone());
        }

        // Persist r1 and finish run so history endpoints return data
        crate::db::insert_route_score(&state.db, "run-1", &r1)
            .await
            .unwrap();
        crate::db::finish_run(&state.db, "run-1", 2).await.unwrap();

        let router = Router::new()
            .route("/api/reports", get(get_reports))
            .route("/api/scan-meta", get(get_scan_meta))
            .route("/api/reports/rescan", post(rescan_all))
            .route("/api/reports/:id/rescan", post(rescan_one))
            .route("/api/crux/*site", get(get_crux_history))
            .route("/api/runs", get(list_runs))
            .route("/api/runs/:run_id/scores", get(get_run_scores))
            .route("/api/history/route", get(get_route_history))
            .with_state(Arc::clone(&state));

        // 1. GET /api/reports
        let resp = router
            .clone()
            .oneshot(axum::http::Request::builder().uri("/api/reports").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let reports_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(reports_json.as_array().unwrap().len(), 2);

        // 2. GET /api/scan-meta
        let resp = router
            .clone()
            .oneshot(axum::http::Request::builder().uri("/api/scan-meta").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let meta_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(meta_json["routes"], 2);
        assert!((meta_json["score"].as_f64().unwrap() - 0.85).abs() < 1e-6);
        assert_eq!(meta_json["monitor"]["doneTargets"], 1);
        assert_eq!(meta_json["monitor"]["allTargets"], 2);

        // 3. GET /api/runs, /api/runs/run-1/scores, /api/history/route
        let resp = router
            .clone()
            .oneshot(axum::http::Request::builder().uri("/api/runs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/runs/run-1/scores")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let scores_val: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(scores_val.as_array().unwrap().len(), 1);

        // Missing ?path= parameter -> 400 Bad Request
        let resp_bad = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/history/route")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_bad.status(), StatusCode::BAD_REQUEST);

        // Valid ?path=/ parameter -> 200 OK with trend point
        let resp_ok = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/history/route?path=/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_ok.status(), StatusCode::OK);
        let body = to_bytes(resp_ok.into_body(), usize::MAX).await.unwrap();
        let trend_val: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(trend_val.as_array().unwrap().len(), 1);

        // 4. GET /api/crux/https:/example.com/history without token -> {"exists":false}
        let resp_crux = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/crux/https:/example.com/history")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_crux.status(), StatusCode::OK);
        let body = to_bytes(resp_crux.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], br#"{"exists":false}"#);

        // 5. POST /api/reports/id1/rescan (200 OK) and unknown ID (404)
        let resp_rescan_one = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/reports/id1/rescan")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_rescan_one.status(), StatusCode::OK);
        let requeued = work_rx.try_recv().unwrap();
        assert_eq!(requeued.id, "id1");

        let resp_404 = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/reports/nonexistent/rescan")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_404.status(), StatusCode::NOT_FOUND);

        // 6. POST /api/reports/rescan clears remaining reports and re-queues them
        let resp_rescan_all = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/reports/rescan")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_rescan_all.status(), StatusCode::OK);
        let requeued2 = work_rx.try_recv().unwrap();
        assert_eq!(requeued2.id, "id2");
        assert!(state.route_reports.read().await.is_empty());
    }
}
