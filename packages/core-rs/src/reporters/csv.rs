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
