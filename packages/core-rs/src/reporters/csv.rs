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
            .or_else(|| {
                report
                    .web_vitals
                    .as_ref()
                    .map(|wv| format!("{}", (wv.score * 100.0).round() as i64))
            })
            .unwrap_or_default();

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

/// Generate expanded CSV output including Lighthouse category scores, SEO metadata,
/// HTTP status/redirects, and Core Web Vitals metrics.
pub fn report_csv_expanded(reports: &[RouteReport]) -> Result<String> {
    let mut rows: Vec<String> = Vec::new();

    let mut category_keys: Vec<String> = reports
        .iter()
        .find_map(|r| {
            r.report
                .as_ref()
                .map(|rep| rep.categories.keys().cloned().collect())
        })
        .unwrap_or_default();
    category_keys.sort();

    let mut header = vec!["URL".to_string(), "Score".to_string()];
    for key in &category_keys {
        header.push(key_to_title(key));
    }
    header.extend([
        "Status".to_string(),
        "Redirect".to_string(),
        "Title".to_string(),
        "Description".to_string(),
        "Canonical".to_string(),
        "HTML-Bytes".to_string(),
        "Internal-Links".to_string(),
        "External-Links".to_string(),
        "FCP-ms".to_string(),
        "LCP-ms".to_string(),
        "CLS".to_string(),
        "TTFB-ms".to_string(),
        "TBT-ms".to_string(),
    ]);
    rows.push(header.join(","));

    for report in reports {
        let path = escape_csv(&report.route.path);
        let score = report
            .report
            .as_ref()
            .map(|r| format!("{}", (r.score * 100.0).round() as i64))
            .or_else(|| {
                report
                    .web_vitals
                    .as_ref()
                    .map(|wv| format!("{}", (wv.score * 100.0).round() as i64))
            })
            .unwrap_or_default();

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

        let status = report
            .seo
            .as_ref()
            .and_then(|s| s.status_code)
            .map(|c| c.to_string())
            .unwrap_or_default();
        let redirect = report
            .seo
            .as_ref()
            .and_then(|s| s.redirect_to.as_deref())
            .map(escape_csv)
            .unwrap_or_default();
        let title = report
            .seo
            .as_ref()
            .and_then(|s| s.title.as_deref())
            .map(escape_csv)
            .unwrap_or_default();
        let description = report
            .seo
            .as_ref()
            .and_then(|s| s.description.as_deref())
            .map(escape_csv)
            .unwrap_or_default();
        let canonical = report
            .seo
            .as_ref()
            .and_then(|s| s.canonical_url.as_deref())
            .map(escape_csv)
            .unwrap_or_default();
        let html_bytes = report
            .seo
            .as_ref()
            .and_then(|s| s.html_size)
            .map(|n| n.to_string())
            .unwrap_or_default();
        let internal_links = report
            .seo
            .as_ref()
            .and_then(|s| s.internal_links)
            .map(|n| n.to_string())
            .unwrap_or_default();
        let external_links = report
            .seo
            .as_ref()
            .and_then(|s| s.external_links)
            .map(|n| n.to_string())
            .unwrap_or_default();

        let fmt_ms = |v: Option<f64>| v.map(|x| format!("{x:.0}")).unwrap_or_default();
        let fmt_cls = |v: Option<f64>| v.map(|x| format!("{x:.3}")).unwrap_or_default();

        let fcp = fmt_ms(report.web_vitals.as_ref().and_then(|w| w.fcp));
        let lcp = fmt_ms(report.web_vitals.as_ref().and_then(|w| w.lcp));
        let cls = fmt_cls(report.web_vitals.as_ref().and_then(|w| w.cls));
        let ttfb = fmt_ms(report.web_vitals.as_ref().and_then(|w| w.ttfb));
        let tbt = fmt_ms(report.web_vitals.as_ref().and_then(|w| w.tbt));

        row.extend([
            status,
            redirect,
            title,
            description,
            canonical,
            html_bytes,
            internal_links,
            external_links,
            fcp,
            lcp,
            cls,
            ttfb,
            tbt,
        ]);
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

        let csv_str = report_csv_simple(&[r1.clone(), r2]).unwrap();
        let lines: Vec<&str> = csv_str.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "URL,Score,PERFORMANCE,SEO");
        assert_eq!(lines[1], "\"/\",91,92,89");
        assert_eq!(lines[2], "\"/missing\",,,");

        let exp_str = report_csv_expanded(&[r1]).unwrap();
        let exp_lines: Vec<&str> = exp_str.lines().collect();
        assert_eq!(exp_lines.len(), 2);
        assert!(exp_lines[0].starts_with("URL,Score,PERFORMANCE,SEO,Status,Redirect,Title"));
        assert!(exp_lines[1].starts_with("\"/\",91,92,89,"));
    }
}
