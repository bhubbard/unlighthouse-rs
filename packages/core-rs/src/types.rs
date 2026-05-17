use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Task status ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStatus {
    #[default]
    Waiting,
    InProgress,
    Completed,
    Failed,
    Ignore,
    FailedRetry,
}

// ── Route types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteDefinition {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalisedRoute {
    pub id: String,
    pub path: String,
    pub url: String,
    pub definition: RouteDefinition,
}

// ── Lighthouse report ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LighthouseCategoryScore {
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LighthouseReport {
    /// Aggregate score 0.0–1.0
    pub score: f64,
    pub categories: HashMap<String, LighthouseCategoryScore>,
    /// Full audit results from Lighthouse (screenshot-thumbnails, LCP, CLS, etc.)
    /// Passed through as raw JSON so the Vue client can render each audit cell.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub audits: serde_json::Value,
}

// ── SEO + HTTP health snapshot (from HTML inspection) ─────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SeoData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_links: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_links: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_size: Option<usize>,
    /// Final HTTP status code (200, 301, 404, 500, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    /// Populated when the server redirected the request to a different URL.
    /// Contains the final destination URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_to: Option<String>,
}

impl SeoData {
    /// True when the status code indicates a client or server error.
    pub fn is_error(&self) -> bool {
        self.status_code.map(|s| s >= 400).unwrap_or(false)
    }

    /// True when the server issued a redirect.
    pub fn is_redirect(&self) -> bool {
        self.redirect_to.is_some()
    }
}

// ── Web Vitals snapshot (fast mode — no Lighthouse) ───────────────────────────

/// Core Web Vitals measured natively via the browser's PerformanceObserver API.
/// Populated only when `--mode fast` is active; `None` in full (Lighthouse) mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebVitalsSnapshot {
    /// First Contentful Paint (ms)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fcp: Option<f64>,
    /// Largest Contentful Paint (ms)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lcp: Option<f64>,
    /// Cumulative Layout Shift (unitless)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cls: Option<f64>,
    /// Time to First Byte (ms)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttfb: Option<f64>,
    /// Total Blocking Time (ms) — approximated via Long Tasks API
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tbt: Option<f64>,
    /// Composite performance score (0.0–1.0), computed from the metrics above
    /// using Core Web Vitals thresholds.
    pub score: f64,
}

// ── Per-task status map ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskMap {
    pub inspect_html_task: TaskStatus,
    pub run_lighthouse_task: TaskStatus,
}

// ── Route report (the central data structure) ─────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteReport {
    pub report_id: String,
    pub route: NormalisedRoute,
    pub artifact_path: String,
    pub artifact_url: String,
    pub tasks: TaskMap,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<LighthouseReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seo: Option<SeoData>,
    /// Populated in fast mode (--mode fast) instead of `report`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_vitals: Option<WebVitalsSnapshot>,
}

// ── Worker statistics ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkerStatus {
    Working,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerStats {
    pub status: WorkerStatus,
    pub done_targets: usize,
    pub all_targets: usize,
    pub done_perc_str: String,
    pub error_perc: String,
    pub time_running: u64,
    pub time_remaining: i64,
    pub pages_per_second: String,
    pub workers: usize,
}

// ── Scan meta (sent to dashboard) ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanMeta {
    pub routes: usize,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor: Option<WorkerStats>,
}

// ── WebSocket events ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "event", content = "data")]
pub enum WsEvent {
    TaskAdded(RouteReport),
    TaskStarted(RouteReport),
    TaskComplete(RouteReport),
    ScanMetaUpdate(ScanMeta),
}
