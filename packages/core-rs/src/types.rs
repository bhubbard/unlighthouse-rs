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

// ── SEO snapshot (from HTML inspection) ──────────────────────────────────────

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
