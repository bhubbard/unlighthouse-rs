use crate::types::RouteReport;
use anyhow::Result;

fn escape_csv(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// Generate CSV output in the simple format:
/// URL,Score,Performance,Accessibility,Best-Practices,SEO
pub fn report_csv_simple(reports: &[RouteReport]) -> Result<String> {
    let mut rows: Vec<String> = Vec::new();

    // Determine category order from the first report that has categories.
    // Sort for deterministic column ordering across runs.
    let mut category_keys: Vec<String> = reports
        .iter()
        .find_map(|r| r.report.as_ref().map(|rep| rep.categories.keys().cloned().collect()))
        .unwrap_or_default();
    category_keys.sort();

    // Build header
    let mut header = vec!["URL".to_string(), "Score".to_string()];
    for key in &category_keys {
        // Convert camelCase key to Title Case for the header
        header.push(key_to_title(key));
    }
    rows.push(header.join(","));

    // Build data rows
    for report in reports {
        let path = escape_csv(&report.route.path);
        let score = report
            .report
            .as_ref()
            .map(|r| format!("{}", (r.score * 100.0).round() as i64))
            .unwrap_or_else(|| "".to_string());

        let mut row = vec![path, score];

        if let Some(rep) = &report.report {
            for key in &category_keys {
                let cat_score = rep
                    .categories
                    .get(key)
                    .and_then(|c| c.score)
                    .map(|s| format!("{}", (s * 100.0).round() as i64))
                    .unwrap_or_default();
                row.push(cat_score);
            }
        } else {
            for _ in &category_keys {
                row.push(String::new());
            }
        }

        rows.push(row.join(","));
    }

    Ok(rows.join("\n"))
}

fn key_to_title(key: &str) -> String {
    // Convert "bestPractices" -> "Best-Practices", "performance" -> "Performance"
    let mut out = String::new();
    let mut prev_upper = false;
    for (i, c) in key.chars().enumerate() {
        if c.is_uppercase() && i > 0 && !prev_upper {
            out.push('-');
        }
        out.push(c.to_ascii_uppercase());
        prev_upper = c.is_uppercase();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        LighthouseCategoryScore, LighthouseReport, NormalisedRoute, RouteDefinition, TaskMap,
    };
    use std::collections::HashMap;

    #[test]
    fn test_escape_csv_and_key_to_title() {
        assert_eq!(escape_csv("/about"), "\"/about\"");
        assert_eq!(escape_csv("/path\"with\"quotes"), "\"/path\"\"with\"\"quotes\"");
        assert_eq!(key_to_title("bestPractices"), "BEST-PRACTICES");
        assert_eq!(key_to_title("performance"), "PERFORMANCE");
    }

    #[test]
    fn test_report_csv_simple() {
        let mut categories = HashMap::new();
        categories.insert(
            "performance".to_string(),
            LighthouseCategoryScore {
                score: Some(0.924),
                title: None,
                key: None,
            },
        );
        categories.insert(
            "seo".to_string(),
            LighthouseCategoryScore {
                score: Some(0.886),
                title: None,
                key: None,
            },
        );

        let r1 = RouteReport {
            report_id: "1".to_string(),
            route: NormalisedRoute {
                id: "1".to_string(),
                path: "/".to_string(),
                url: "https://example.com/".to_string(),
                definition: RouteDefinition {
                    name: "/".to_string(),
                    path: "/".to_string(),
                },
            },
            artifact_path: "/tmp/1".to_string(),
            artifact_url: "/1/".to_string(),
            tasks: TaskMap::default(),
            report: Some(LighthouseReport {
                score: 0.905,
                categories,
                audits: serde_json::Value::Null,
            }),
            seo: None,
            web_vitals: None,
        };

        let r2 = RouteReport {
            report_id: "2".to_string(),
            route: NormalisedRoute {
                id: "2".to_string(),
                path: "/missing".to_string(),
                url: "https://example.com/missing".to_string(),
                definition: RouteDefinition {
                    name: "/missing".to_string(),
                    path: "/missing".to_string(),
                },
            },
            artifact_path: "/tmp/2".to_string(),
            artifact_url: "/2/".to_string(),
            tasks: TaskMap::default(),
            report: None,
            seo: None,
            web_vitals: None,
        };

        let csv_str = report_csv_simple(&[r1, r2]).unwrap();
        let lines: Vec<&str> = csv_str.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "URL,Score,PERFORMANCE,SEO");
        assert_eq!(lines[1], "\"/\",91,92,89");
        assert_eq!(lines[2], "\"/missing\",,,");
    }
}
