use crate::types::RouteReport;
use anyhow::Result;

pub fn report_markdown(reports: &[RouteReport]) -> Result<String> {
    let mut md = String::new();
    md.push_str("# Unlighthouse Report\n\n");

    // Detect whether any report has Web Vitals (fast mode) vs Lighthouse (full mode).
    let has_vitals  = reports.iter().any(|r| r.web_vitals.is_some());
    let has_lh      = reports.iter().any(|r| r.report.is_some());
    let has_status  = reports.iter().any(|r| r.seo.as_ref().and_then(|s| s.status_code).is_some());

    // ── Fast-mode table (Web Vitals) ─────────────────────────────────────────
    if has_vitals && !has_lh {
        let status_col = if has_status { " Status |" } else { "" };
        md.push_str(&format!(
            "| Path | Score | FCP (ms) | LCP (ms) | CLS | TTFB (ms) | TBT (ms) |{}\n",
            status_col
        ));
        md.push_str(&format!(
            "| --- | --- | --- | --- | --- | --- | --- |{}\n",
            if has_status { " --- |" } else { "" }
        ));

        for report in reports {
            let path   = &report.route.path;
            let score  = report.web_vitals.as_ref()
                .map(|wv| format!("{:.0}", wv.score * 100.0))
                .unwrap_or_else(|| "-".to_string());

            let fmt_ms  = |v: Option<f64>| v.map(|x| format!("{x:.0}")).unwrap_or_else(|| "-".to_string());
            let fmt_cls = |v: Option<f64>| v.map(|x| format!("{x:.3}")).unwrap_or_else(|| "-".to_string());

            let (fcp, lcp, cls, ttfb, tbt) = report.web_vitals.as_ref()
                .map(|wv| (
                    fmt_ms(wv.fcp),
                    fmt_ms(wv.lcp),
                    fmt_cls(wv.cls),
                    fmt_ms(wv.ttfb),
                    fmt_ms(wv.tbt),
                ))
                .unwrap_or_else(|| ("-".into(), "-".into(), "-".into(), "-".into(), "-".into()));

            let status_cell = if has_status {
                let code = report.seo.as_ref()
                    .and_then(|s| s.status_code)
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "-".to_string());
                let redirect = report.seo.as_ref()
                    .and_then(|s| s.redirect_to.as_deref())
                    .map(|r| format!(" → {r}"))
                    .unwrap_or_default();
                format!(" {code}{redirect} |")
            } else {
                String::new()
            };

            md.push_str(&format!(
                "| `{path}` | **{score}** | {fcp} | {lcp} | {cls} | {ttfb} | {tbt} |{status_cell}\n"
            ));
        }

        md.push('\n');
        md.push_str("_Scores measured natively via the PerformanceObserver API (fast mode — no Lighthouse)._\n");
        return Ok(md);
    }

    // ── Full-mode table (Lighthouse) ─────────────────────────────────────────
    let status_col = if has_status { " Status |" } else { "" };
    md.push_str(&format!(
        "| Path | Score | Performance | Accessibility | Best Practices | SEO | PWA |{}\n",
        status_col
    ));
    md.push_str(&format!(
        "| --- | --- | --- | --- | --- | --- | --- |{}\n",
        if has_status { " --- |" } else { "" }
    ));

    for report in reports {
        let path  = &report.route.path;
        let score = report.report.as_ref()
            .map(|r| format!("{:.0}", r.score * 100.0))
            .unwrap_or_else(|| "-".to_string());

        let mut cats = vec!["-".to_string(); 5];
        if let Some(rep) = &report.report {
            let cat_order = ["performance", "accessibility", "best-practices", "seo", "pwa"];
            for (i, cat_id) in cat_order.iter().enumerate() {
                if let Some(cat) = rep.categories.get(*cat_id) {
                    cats[i] = cat.score
                        .map(|s| format!("{:.0}", s * 100.0))
                        .unwrap_or_else(|| "-".to_string());
                }
            }
        }

        let status_cell = if has_status {
            let code = report.seo.as_ref()
                .and_then(|s| s.status_code)
                .map(|c| c.to_string())
                .unwrap_or_else(|| "-".to_string());
            let redirect = report.seo.as_ref()
                .and_then(|s| s.redirect_to.as_deref())
                .map(|r| format!(" → {r}"))
                .unwrap_or_default();
            format!(" {code}{redirect} |")
        } else {
            String::new()
        };

        md.push_str(&format!(
            "| `{}` | **{}** | {} | {} | {} | {} | {} |{}\n",
            path, score, cats[0], cats[1], cats[2], cats[3], cats[4], status_cell
        ));
    }

    Ok(md)
}
