use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::discovery::routes::normalise_route;
use crate::server::api::broadcast_and_update;
use crate::server::AppState;
use crate::types::{
    LighthouseCategoryScore, LighthouseReport, RouteReport, TaskMap, TaskStatus, WsEvent,
};

use super::browser::BrowserHandle;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build an initial RouteReport with all tasks set to Waiting.
pub fn create_route_report(route: &crate::types::NormalisedRoute, config: &Config) -> RouteReport {
    let artifact_path = PathBuf::from(&config.output_path)
        .join(sanitise_hostname(&config.site))
        .join(&route.id);
    let artifact_url = format!("/{}/{}/", sanitise_hostname(&config.site), &route.id);

    RouteReport {
        report_id: route.id.clone(),
        route: route.clone(),
        artifact_path: artifact_path.to_string_lossy().to_string(),
        artifact_url,
        tasks: TaskMap {
            inspect_html_task: TaskStatus::Waiting,
            run_lighthouse_task: TaskStatus::Waiting,
        },
        report: None,
        seo: None,
    }
}

fn sanitise_hostname(site: &str) -> String {
    url::Url::parse(site)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
        .replace(':', "_")
}

// ── Per-route processing ──────────────────────────────────────────────────────

/// Process a single route end-to-end: HTML inspection then Lighthouse audit.
///
/// Steps:
/// 1. Acquire a concurrency permit from `AppState::semaphore` (blocks until a
///    worker slot is free).
/// 2. Fetch the page via the selected [`BrowserHandle`] backend to extract SEO
///    metadata and discover internal links for further crawling.
/// 3. Invoke the Lighthouse Node.js subprocess to produce performance,
///    accessibility, best-practices, and SEO scores.
/// 4. Broadcast status updates over WebSocket after each step so the dashboard
///    reflects live progress.
///
/// Newly discovered links are fed back into `AppState::work_tx`, subject to the
/// `max_routes` cap. The cap is enforced under a single write-lock acquisition
/// to avoid TOCTOU races when multiple workers discover links simultaneously.
///
/// # Panics
/// Panics if the semaphore is closed, which should never happen during normal
/// operation (the semaphore lives as long as `AppState`).
pub async fn process_route(
    route: crate::types::NormalisedRoute,
    state: Arc<AppState>,
    browser: BrowserHandle,
) {
    let config = Arc::clone(&state.config);

    // ── Register the route ────────────────────────────────────────────────────
    let mut report = create_route_report(&route, &config);
    {
        let mut reports = state.route_reports.write().await;
        reports.insert(route.id.clone(), report.clone());
    }
    broadcast_and_update(&state, WsEvent::TaskAdded(report.clone())).await;

    // ── Step 1: HTML inspection (High Concurrency) ───────────────────────────
    {
        let _permit = state.discovery_semaphore.acquire().await.expect("semaphore closed");
        
        report.tasks.inspect_html_task = TaskStatus::InProgress;
        broadcast_and_update(&state, WsEvent::TaskStarted(report.clone())).await;

        match browser.inspect_html(&route.url).await {
            Ok((seo, discovered_links)) => {
                report.seo = Some(seo);
                report.tasks.inspect_html_task = TaskStatus::Completed;

                if config.scanner.crawler {
                    let max = config.scanner.max_routes.unwrap_or(usize::MAX);
                    let mut reports = state.route_reports.write().await;
                    for href in discovered_links {
                        if reports.len() >= max {
                            break;
                        }
                        let new_route = normalise_route(&href, &config.site);
                        if crate::discovery::routes::passes_filters(&new_route.path, &config.scanner.include, &config.scanner.exclude) {
                            if !reports.contains_key(&new_route.id) {
                                reports.insert(new_route.id.clone(), create_route_report(&new_route, &config));
                                let _ = state.work_tx.send(new_route).await;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warn!(url = %route.url, error = %e, "HTML inspection failed");
                report.tasks.inspect_html_task = TaskStatus::Failed;
            }
        }

        {
            let mut reports = state.route_reports.write().await;
            reports.insert(route.id.clone(), report.clone());
        }
        broadcast_and_update(&state, WsEvent::TaskComplete(report.clone())).await;
    }

    // ── Step 2: Lighthouse audit (Controlled Concurrency via Pool) ────────────
    {
        let _permit = state.lighthouse_semaphore.acquire().await.expect("semaphore closed");
        
        report.tasks.run_lighthouse_task = TaskStatus::InProgress;
        broadcast_and_update(&state, WsEvent::TaskStarted(report.clone())).await;

        let artifact_path = PathBuf::from(&report.artifact_path);

        if let Err(e) = tokio::fs::create_dir_all(&artifact_path).await {
            error!("Cannot create artifact dir {:?}: {e}", artifact_path);
            report.tasks.run_lighthouse_task = TaskStatus::Failed;
        } else {
            // Use the pool if available, otherwise fallback to one-off
            let res = if let Some(pool) = &state.lighthouse_pool {
                // Determine a worker index (could be random or based on route id)
                let worker_idx = route.id.chars().next().unwrap_or('0') as usize;
                let worker = pool.get_worker(worker_idx).await;
                let mut worker = worker.lock().await;
                
                let device_str = match config.scanner.device {
                    crate::config::Device::Desktop => "desktop",
                    _ => "mobile",
                };
                
                worker.audit(crate::queue::pool::AuditTask {
                    url: route.url.clone(),
                    output_dir: report.artifact_path.clone(),
                    device: device_str.to_string(),
                    throttle: config.scanner.throttle,
                    skip_javascript: config.scanner.skip_javascript,
                    block_assets: config.scanner.block_assets,
                    warmup: config.scanner.warmup,
                    auth: config.auth.clone(),
                    cookies: config.cookies.clone(),
                    local_storage: config.local_storage.clone(),
                    session_storage: config.session_storage.clone(),
                    extra_headers: config.extra_headers.clone(),
                    user_agent: config.user_agent.clone(),
                }).await
            } else {
                // Fallback to one-off process (backward compatibility)
                run_lighthouse_fallback(&route.url, &artifact_path, &config).await
                    .map(|r| crate::queue::pool::AuditResult {
                        success: true,
                        url: route.url.clone(),
                        scores: Some(r.categories.iter().map(|(k, v)| (k.clone(), v.score.unwrap_or(0.0))).collect()),
                        error: None,
                    })
                    .map_err(|e| e)
            };

            match res {
                Ok(audit_res) if audit_res.success => {
                    // Re-parse the report.json to get full audits
                    let json_path = artifact_path.join("report.json");
                    if let Ok(json) = tokio::fs::read_to_string(json_path).await {
                        if let Ok(lh_report) = parse_lighthouse_report(&serde_json::from_str(&json).unwrap()) {
                             report.report = Some(lh_report);
                        }
                    }
                    report.tasks.run_lighthouse_task = TaskStatus::Completed;
                    info!(
                        path  = %route.path,
                        score = report.report.as_ref().map(|r| r.score * 100.0).unwrap_or(0.0),
                        "Lighthouse complete"
                    );
                }
                Ok(audit_res) => {
                    warn!(url = %route.url, error = ?audit_res.error, "Lighthouse failed");
                    report.tasks.run_lighthouse_task = TaskStatus::Failed;
                }
                Err(e) => {
                    warn!(url = %route.url, error = %e, "Lighthouse pool audit failed");
                    report.tasks.run_lighthouse_task = TaskStatus::Failed;
                }
            }
        }
    }

    {
        let mut reports = state.route_reports.write().await;
        reports.insert(route.id.clone(), report.clone());
    }
    broadcast_and_update(&state, WsEvent::TaskComplete(report)).await;
}

// ── Lighthouse subprocess ─────────────────────────────────────────────────────

/// Invoke the Lighthouse Node.js worker (one-off fallback) and parse its JSON output.
async fn run_lighthouse_fallback(
    url: &str,
    artifact_path: &PathBuf,
    config: &Config,
) -> Result<LighthouseReport> {
    if config.lighthouse_process_path.is_empty() {
        return Err(anyhow::anyhow!("lighthouse_process_path is not configured"));
    }

    let device_str = match config.scanner.device {
        crate::config::Device::Desktop => "desktop",
        _ => "mobile",
    };

    let mut cmd = Command::new("node");
    cmd.arg(&config.lighthouse_process_path)
        .arg("--url").arg(url)
        .arg("--output-dir").arg(artifact_path)
        .arg("--device").arg(device_str);
    if config.scanner.throttle {
        cmd.arg("--throttle");
    }
    if config.scanner.skip_javascript {
        cmd.arg("--skip-javascript");
    }
    if config.scanner.warmup {
        cmd.arg("--warmup");
    }
    if config.scanner.block_assets {
        cmd.arg("--block-assets");
    }

    let output = cmd.output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "lighthouse exited with {}: {stderr}",
            output.status
        ));
    }

    let json_path = artifact_path.join("report.json");
    if !json_path.exists() {
        return Err(anyhow::anyhow!(
            "Lighthouse finished successfully but 'report.json' is missing in {:?}. Check worker logs.",
            artifact_path
        ));
    }

    let json = tokio::fs::read_to_string(json_path).await?;
    parse_lighthouse_report(&serde_json::from_str(&json)?)
}

/// Extract scores and full audit data from the Lighthouse JSON output.
fn parse_lighthouse_report(raw: &serde_json::Value) -> Result<LighthouseReport> {
    let cats = raw
        .get("categories")
        .ok_or_else(|| anyhow::anyhow!("Missing 'categories' in lighthouse report"))?
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("'categories' is not an object"))?;

    let mut categories = std::collections::HashMap::new();
    let mut score_sum = 0.0f64;
    let mut score_count = 0usize;

    for (key, cat) in cats {
        let score = cat.get("score").and_then(|s| s.as_f64());
        let title = cat.get("title").and_then(|t| t.as_str()).map(str::to_string);

        if let Some(s) = score {
            score_sum += s;
            score_count += 1;
        }

        categories.insert(
            key.clone(),
            LighthouseCategoryScore {
                score,
                title,
                key: Some(key.clone()),
            },
        );
    }

    let aggregate = if score_count > 0 { score_sum / score_count as f64 } else { 0.0 };

    // Pass the full audits object through so the client can render
    // screenshot-thumbnails, LCP, CLS, colour-contrast, etc.
    let audits = raw
        .get("audits")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    Ok(LighthouseReport { score: aggregate, categories, audits })
}

// ── Worker loop ───────────────────────────────────────────────────────────────

/// Drain the work channel indefinitely, spawning one `process_route` task per
/// route received.
///
/// The [`BrowserHandle`] is shared across all spawned tasks; it is cheaply
/// cloneable and internally reference-counted. Each task opens/closes its own
/// browser tab (or makes its own HTTP request) rather than its own browser.
///
/// Deduplication is applied here: routes already registered in
/// `AppState::route_reports` are skipped. Note that routes discovered during
/// crawling are pre-registered under the write lock in `process_route`, so
/// this check acts as a secondary guard against channel duplicates.
///
/// Returns when the work channel sender is dropped (i.e., at program shutdown).
pub async fn run_worker_loop(
    state: Arc<AppState>,
    browser: BrowserHandle,
    mut work_rx: tokio::sync::mpsc::Receiver<crate::types::NormalisedRoute>,
) {
    info!(concurrency = state.config.workers, "Worker loop started");

    while let Some(route) = work_rx.recv().await {
        // Skip if already registered (dedup)
        if state.route_reports.read().await.contains_key(&route.id) {
            debug!(path = %route.path, "Skipping already-queued route");
            continue;
        }

        let state2 = state.clone();
        let browser2 = browser.clone();
        tokio::spawn(async move {
            process_route(route, state2, browser2).await;
        });
    }
}
