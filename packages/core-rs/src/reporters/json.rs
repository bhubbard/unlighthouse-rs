use crate::types::RouteReport;
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

/// Simple JSON report: one entry per route with path + scores.
#[derive(Serialize)]
pub struct SimpleRouteReport {
    pub path: String,
    pub score: Option<f64>,
    #[serde(flatten)]
    pub categories: HashMap<String, Option<f64>>,
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
            SimpleRouteReport {
                path: report.route.path.clone(),
                score: report.report.as_ref().map(|r| r.score),
                categories,
            }
        })
        .collect();

    Ok(serde_json::to_string_pretty(&simple)?)
}

/// Expanded JSON report: full RouteReport objects.
pub fn report_json_expanded(reports: &[RouteReport]) -> Result<String> {
    Ok(serde_json::to_string_pretty(reports)?)
}
