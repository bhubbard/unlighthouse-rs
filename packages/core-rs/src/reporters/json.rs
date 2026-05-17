use crate::types::RouteReport;
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

/// Simple JSON report: one entry per route with path, composite score,
/// per-category Lighthouse scores (full mode), Web Vitals (fast mode),
/// and HTTP health fields from SEO inspection.
#[derive(Serialize)]
pub struct SimpleRouteReport {
    pub path: String,
    /// Composite score 0.0–1.0.  Sourced from Lighthouse in full mode,
    /// or from the Web Vitals composite in fast mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// Per-category Lighthouse scores 0.0–1.0 (full mode only).
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub categories: HashMap<String, Option<f64>>,
    /// HTTP status code returned by the page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    /// Final URL when the server issued a redirect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_to: Option<String>,
    /// Web Vitals composite score 0.0–1.0 (fast mode only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vitals_score: Option<f64>,
    /// Largest Contentful Paint in milliseconds (fast mode only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lcp: Option<f64>,
    /// Cumulative Layout Shift (unitless, fast mode only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cls: Option<f64>,
    /// First Contentful Paint in milliseconds (fast mode only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fcp: Option<f64>,
    /// Time to First Byte in milliseconds (fast mode only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttfb: Option<f64>,
    /// Total Blocking Time in milliseconds (fast mode only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tbt: Option<f64>,
}

pub fn report_json_simple(reports: &[RouteReport]) -> Result<String> {
    let simple: Vec<SimpleRouteReport> = reports
        .iter()
        .map(|report| {
            let mut categories = HashMap::new();
            if let Some(rep) = &report.report {
                for (key, cat) in &rep.categories {
                    categories.insert(key.clone(), cat.score);
                }
            }

            // Composite score: prefer Lighthouse, then Web Vitals.
            let score = report.report.as_ref().map(|r| r.score)
                .or_else(|| report.web_vitals.as_ref().map(|wv| wv.score));

            // HTTP health fields from SEO inspection.
            let status_code  = report.seo.as_ref().and_then(|s| s.status_code);
            let redirect_to  = report.seo.as_ref().and_then(|s| s.redirect_to.clone());

            // Web Vitals (fast mode).
            let vitals_score = report.web_vitals.as_ref().map(|wv| wv.score);
            let lcp          = report.web_vitals.as_ref().and_then(|wv| wv.lcp);
            let cls          = report.web_vitals.as_ref().and_then(|wv| wv.cls);
            let fcp          = report.web_vitals.as_ref().and_then(|wv| wv.fcp);
            let ttfb         = report.web_vitals.as_ref().and_then(|wv| wv.ttfb);
            let tbt          = report.web_vitals.as_ref().and_then(|wv| wv.tbt);

            SimpleRouteReport {
                path: report.route.path.clone(),
                score,
                categories,
                status_code,
                redirect_to,
                vitals_score,
                lcp,
                cls,
                fcp,
                ttfb,
                tbt,
            }
        })
        .collect();

    Ok(serde_json::to_string_pretty(&simple)?)
}

/// Expanded JSON report: full RouteReport objects (includes all raw data).
pub fn report_json_expanded(reports: &[RouteReport]) -> Result<String> {
    Ok(serde_json::to_string_pretty(reports)?)
}
