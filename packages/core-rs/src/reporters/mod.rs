pub mod csv;
pub mod json;
pub mod markdown;

#[cfg(feature = "native")]
pub mod lhci;

#[cfg(feature = "native")]
use anyhow::Result;

#[cfg(feature = "native")]
use std::path::Path;
#[cfg(feature = "native")]
use tracing::info;

#[cfg(feature = "native")]
use crate::config::ReporterType;
#[cfg(feature = "native")]
use crate::types::RouteReport;

/// Write the configured report to the output path and return the file path written or compare URL.
#[cfg(feature = "native")]
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

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::types::{NormalisedRoute, RouteDefinition, TaskMap};

    fn dummy_report() -> RouteReport {
        RouteReport {
            report_id: "r1".to_string(),
            route: NormalisedRoute {
                id: "r1".to_string(),
                path: "/".to_string(),
                url: "https://example.com/".to_string(),
                definition: RouteDefinition {
                    name: "/".to_string(),
                    path: "/".to_string(),
                },
            },
            artifact_path: "/tmp/r1".to_string(),
            artifact_url: "/r1/".to_string(),
            tasks: TaskMap::default(),
            report: None,
            seo: None,
            web_vitals: None,
        }
    }

    #[tokio::test]
    async fn test_write_report_all_file_formats() {
        let dir = tempfile::tempdir().unwrap();
        let reports = vec![dummy_report()];

        let mut config = Config {
            output_path: dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        };

        // 1. ReporterType::None
        config.ci.reporter = ReporterType::None;
        assert!(write_report(&reports, &config).await.unwrap().is_none());

        // 2. JsonSimple -> report.json
        config.ci.reporter = ReporterType::JsonSimple;
        let p_json = write_report(&reports, &config).await.unwrap().unwrap();
        assert!(p_json.ends_with("report.json"));
        assert!(Path::new(&p_json).exists());

        // 3. JsonExpanded -> report-expanded.json
        config.ci.reporter = ReporterType::JsonExpanded;
        let p_exp = write_report(&reports, &config).await.unwrap().unwrap();
        assert!(p_exp.ends_with("report-expanded.json"));
        assert!(Path::new(&p_exp).exists());

        // 4. CsvSimple -> report.csv
        config.ci.reporter = ReporterType::CsvSimple;
        let p_csv = write_report(&reports, &config).await.unwrap().unwrap();
        assert!(p_csv.ends_with("report.csv"));
        assert!(Path::new(&p_csv).exists());

        // 5. Markdown -> report.md
        config.ci.reporter = ReporterType::Markdown;
        let p_md = write_report(&reports, &config).await.unwrap().unwrap();
        assert!(p_md.ends_with("report.md"));
        assert!(Path::new(&p_md).exists());
    }
}
