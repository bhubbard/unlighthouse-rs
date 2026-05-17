use anyhow::{Result, Context};
use tracing::{info, warn, error};
use std::path::Path;
use crate::config::Config;
use crate::types::RouteReport;

async fn run_git_cmd(args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .output()
        .await
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

pub async fn upload_to_lhci_server(
    reports: &[RouteReport],
    config: &Config,
) -> Result<String> {
    let lhci_config = &config.ci;
    let host = lhci_config.lhci_host.as_deref().unwrap_or("").trim_end_matches('/');
    if host.is_empty() {
        return Err(anyhow::anyhow!("LHCI host is not configured"));
    }
    let token = lhci_config.lhci_build_token.as_deref().unwrap_or("");
    if token.is_empty() {
        return Err(anyhow::anyhow!("LHCI build token is not configured"));
    }

    info!("Preparing git metadata for LHCI upload...");
    let hash = run_git_cmd(&["rev-parse", "HEAD"]).await
        .or_else(|| std::env::var("GITHUB_SHA").ok())
        .unwrap_or_else(|| "0000000000000000000000000000000000000000".to_string());

    let branch = run_git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"]).await
        .or_else(|| std::env::var("GITHUB_REF_NAME").ok())
        .unwrap_or_else(|| "main".to_string());

    let author = run_git_cmd(&["log", "-1", "--format=%an <%ae>"]).await
        .unwrap_or_else(|| "Unlighthouse User <user@unlighthouse.dev>".to_string());

    let message = run_git_cmd(&["log", "-1", "--format=%s"]).await
        .unwrap_or_else(|| "Unlighthouse CI scan".to_string());

    let committed_at = run_git_cmd(&["log", "-1", "--format=%cI"]).await
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

    let run_at = chrono::Utc::now().to_rfc3339();

    let client_builder = reqwest::Client::builder();
    // Add custom basic authentication if configured
    let client = if let Some(ref auth) = lhci_config.lhci_auth {
        client_builder.default_headers({
            let mut headers = reqwest::header::HeaderMap::new();
            let base64_auth = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, auth.as_bytes());
            if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Basic {base64_auth}")) {
                headers.insert(reqwest::header::AUTHORIZATION, val);
            }
            headers
        })
    } else {
        client_builder
    }.build()?;

    // 1. Project Lookup
    info!("Looking up LHCI project with build token...");
    let lookup_url = format!("{}/api/projects/lookup", host);
    let lookup_resp: serde_json::Value = client.post(&lookup_url)
        .json(&serde_json::json!({ "token": token }))
        .send()
        .await
        .context("Failed to look up project on LHCI server")?
        .json()
        .await
        .context("Failed to parse LHCI project lookup response")?;

    let project_id = lookup_resp["id"].as_str()
        .ok_or_else(|| anyhow::anyhow!("Project lookup returned no project ID. Check your build token."))?;

    info!("LHCI Project ID resolved: {}", project_id);

    // 2. Create Build
    info!("Creating build on LHCI server...");
    let build_url = format!("{}/api/projects/{}/builds", host, project_id);
    let build_payload = serde_json::json!({
        "projectId": project_id,
        "hash": hash,
        "branch": branch,
        "author": author,
        "avatarUrl": "",
        "commitMessage": message,
        "runAt": run_at,
        "committedAt": committed_at
    });

    let build_resp: serde_json::Value = client.post(&build_url)
        .header("x-lhci-token", token)
        .json(&build_payload)
        .send()
        .await
        .context("Failed to create build on LHCI server")?
        .json()
        .await
        .context("Failed to parse LHCI build creation response")?;

    let build_id = build_resp["id"].as_str()
        .ok_or_else(|| anyhow::anyhow!("Build creation returned no build ID."))?;

    info!("LHCI Build ID created: {}", build_id);

    // 3. Upload runs
    let mut uploaded_count = 0;
    for report in reports {
        // Read report.json from artifact path if it exists
        let report_json_path = Path::new(&report.artifact_path).join("report.json");
        if !report_json_path.exists() {
            warn!("Lighthouse report not found for {}, skipping LHCI upload", report.route.url);
            continue;
        }

        let lhr_str = match tokio::fs::read_to_string(&report_json_path).await {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to read report.json from {:?}: {}", report_json_path, e);
                continue;
            }
        };

        let lhr: serde_json::Value = match serde_json::from_str(&lhr_str) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to parse report.json for {}: {}", report.route.url, e);
                continue;
            }
        };

        // Construct representativeResult with key metrics for quick display in LHCI dashboard
        let mut representative_result = serde_json::Map::new();
        if let Some(categories) = lhr["categories"].as_object() {
            for (cat_key, cat_val) in categories {
                if let Some(score) = cat_val["score"].as_f64() {
                    representative_result.insert(format!("category_{}", cat_key), serde_json::Value::Number(serde_json::Number::from_f64(score).unwrap()));
                }
            }
        }

        let run_payload = serde_json::json!({
            "projectId": project_id,
            "buildId": build_id,
            "representativeResult": representative_result,
            "lhr": lhr_str,
            "url": report.route.url
        });

        let run_url = format!("{}/api/projects/{}/builds/{}/runs", host, project_id, build_id);
        let run_resp = client.post(&run_url)
            .header("x-lhci-token", token)
            .json(&run_payload)
            .send()
            .await;

        match run_resp {
            Ok(resp) if resp.status().is_success() => {
                uploaded_count += 1;
                info!("Successfully uploaded LHCI run for {}", report.route.url);
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                error!("Failed to upload LHCI run for {}: status {}, body: {}", report.route.url, status, text);
            }
            Err(e) => {
                error!("Failed to upload LHCI run request for {}: {}", report.route.url, e);
            }
        }
    }

    info!("LHCI upload completed: {}/{} runs uploaded", uploaded_count, reports.len());
    Ok(format!("{}/app/projects/{}/compare", host, project_id))
}
