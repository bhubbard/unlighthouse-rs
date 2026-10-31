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
    /// Populated when the page defines a canonical URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_url: Option<String>,
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

impl WebVitalsSnapshot {
    /// Computes the Lighthouse v11-aligned weighted composite score (`0.0..=1.0`)
    /// from FCP (10%), LCP (25%), TBT (30%), CLS (25%), and TTFB (10%) piecewise
    /// linear threshold curves.
    pub fn compute_weighted_score(&self) -> f64 {
        fn metric_score(val: f64, good: f64, poor: f64) -> f64 {
            if val <= good {
                1.0 - 0.10 * (val / good.max(1e-6)).clamp(0.0, 1.0)
            } else if val >= poor {
                (0.49 * (poor / val.max(1e-6))).clamp(0.0, 0.49)
            } else {
                let t = (val - good) / (poor - good);
                0.90 - 0.40 * t
            }
        }

        let s_fcp = self
            .fcp
            .map(|v| metric_score(v, 1800.0, 3000.0))
            .unwrap_or(0.90);
        let s_lcp = self
            .lcp
            .map(|v| metric_score(v, 2500.0, 4000.0))
            .unwrap_or(0.90);
        let s_tbt = self
            .tbt
            .map(|v| metric_score(v, 200.0, 600.0))
            .unwrap_or(0.90);
        let s_cls = self.cls.map(|v| metric_score(v, 0.1, 0.25)).unwrap_or(0.95);
        let s_ttfb = self
            .ttfb
            .map(|v| metric_score(v, 800.0, 1800.0))
            .unwrap_or(0.90);

        (0.10 * s_fcp + 0.25 * s_lcp + 0.30 * s_tbt + 0.25 * s_cls + 0.10 * s_ttfb).clamp(0.0, 1.0)
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_usage: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seo_data_health_helpers() {
        let mut seo = SeoData::default();
        assert!(!seo.is_error());
        assert!(!seo.is_redirect());

        seo.status_code = Some(200);
        assert!(!seo.is_error());

        seo.status_code = Some(301);
        seo.redirect_to = Some("https://example.com/new".to_string());
        assert!(!seo.is_error());
        assert!(seo.is_redirect());

        seo.status_code = Some(404);
        assert!(seo.is_error());

        seo.status_code = Some(503);
        assert!(seo.is_error());
    }

    #[test]
    fn test_task_status_serde_kebab_case() {
        assert_eq!(TaskStatus::default(), TaskStatus::Waiting);
        assert_eq!(serde_json::to_string(&TaskStatus::InProgress).unwrap(), "\"in-progress\"");
        assert_eq!(serde_json::to_string(&TaskStatus::FailedRetry).unwrap(), "\"failed-retry\"");
        let parsed: TaskStatus = serde_json::from_str("\"completed\"").unwrap();
        assert_eq!(parsed, TaskStatus::Completed);
    }

    #[test]
    fn test_ws_event_serde_tag_and_content() {
        let meta = ScanMeta {
            routes: 5,
            score: 0.92,
            monitor: None,
        };
        let event = WsEvent::ScanMetaUpdate(meta);
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event"], "scan-meta-update");
        assert_eq!(json["data"]["routes"], 5);
        assert!((json["data"]["score"].as_f64().unwrap() - 0.92).abs() < 1e-6);
    }

    #[test]
    fn test_web_vitals_weighted_score() {
        let fast = WebVitalsSnapshot {
            fcp: Some(900.0),
            lcp: Some(1400.0),
            cls: Some(0.01),
            ttfb: Some(250.0),
            tbt: Some(40.0),
            score: 0.95,
        };
        assert!(fast.compute_weighted_score() >= 0.90);
    }
}
