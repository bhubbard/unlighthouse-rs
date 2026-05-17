pub mod csv;
pub mod json;
pub mod markdown;
pub mod lhci;

use anyhow::Result;
use std::path::Path;
use tracing::info;

use crate::config::ReporterType;
use crate::types::RouteReport;

/// Write the configured report to the output path and return the file path written or compare URL.
pub async fn write_report(
    reports: &[RouteReport],
    config: &crate::config::Config,
) -> Result<Option<String>> {
    let reporter = &config.ci.reporter;
    if *reporter == ReporterType::None {
        return Ok(None);
    }

    if *reporter == ReporterType::Lhci {
        let compare_url = lhci::upload_to_lhci_server(reports, config).await?;
        return Ok(Some(compare_url));
    }

    let out_dir = Path::new(&config.output_path);
    tokio::fs::create_dir_all(out_dir).await?;

    let (filename, content) = match reporter {
        ReporterType::CsvSimple => {
            ("report.csv", csv::report_csv_simple(reports)?)
        }
        ReporterType::JsonSimple => {
            ("report.json", json::report_json_simple(reports)?)
        }
        ReporterType::JsonExpanded => {
            ("report-expanded.json", json::report_json_expanded(reports)?)
        }
        ReporterType::Markdown => {
            ("report.md", markdown::report_markdown(reports)?)
        }
        ReporterType::None | ReporterType::Lhci => unreachable!(),
    };

    let file_path = out_dir.join(filename);
    tokio::fs::write(&file_path, &content).await?;
    let path_str = file_path.to_string_lossy().to_string();
    info!("Report written to {path_str}");
    Ok(Some(path_str))
}
