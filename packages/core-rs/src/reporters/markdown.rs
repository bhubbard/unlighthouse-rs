use crate::types::RouteReport;
use anyhow::Result;

pub fn report_markdown(reports: &[RouteReport]) -> Result<String> {
    let mut md = String::new();
    md.push_str("# Unlighthouse Report\n\n");
    
    md.push_str("| Path | Score | Performance | Accessibility | Best Practices | SEO | PWA |\n");
    md.push_str("| --- | --- | --- | --- | --- | --- | --- |\n");

    for report in reports {
        let path = &report.route.path;
        let score = report.report.as_ref().map(|r| format!("{:.0}", r.score * 100.0)).unwrap_or_else(|| "-".to_string());
        
        let mut cats = vec!["-".to_string(); 5];
        if let Some(rep) = &report.report {
            let cat_order = ["performance", "accessibility", "best-practices", "seo", "pwa"];
            for (i, cat_id) in cat_order.iter().enumerate() {
                if let Some(cat) = rep.categories.get(*cat_id) {
                    cats[i] = cat.score.map(|s| format!("{:.0}", s * 100.0)).unwrap_or_else(|| "-".to_string());
                }
            }
        }
        
        md.push_str(&format!(
            "| `{}` | **{}** | {} | {} | {} | {} | {} |\n",
            path, score, cats[0], cats[1], cats[2], cats[3], cats[4]
        ));
    }

    Ok(md)
}
