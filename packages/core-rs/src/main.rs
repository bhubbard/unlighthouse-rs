#![allow(dead_code)]

mod config;
mod discovery;
mod queue;
mod reporters;
mod server;
mod types;
mod util;

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};

use config::{CliOverrides, Config, ReporterType};
use discovery::routes::resolve_reportable_routes;
use queue::browser::{launch_chromiumoxide, launch_headless_chrome, launch_reqwest};
use queue::worker::run_worker_loop;
use reporters::write_report;
use server::{start_server, AppState};
use types::TaskStatus;

// ── CLI argument definition ───────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "unlighthouse-rs",
    about = "Website auditing tool — Rust core",
    version
)]
struct Cli {
    /// The site to audit (e.g. https://example.com)
    #[arg(long, env = "UNLIGHTHOUSE_SITE")]
    site: Option<String>,

    /// Path to save reports and client files
    #[arg(long)]
    output_path: Option<String>,

    /// Enable debug logging
    #[arg(short, long)]
    debug: bool,

    /// Disable caching
    #[arg(long = "no-cache")]
    no_cache: bool,

    /// Path to config file (unlighthouse.config.toml or .json)
    #[arg(long = "config", short = 'c')]
    config_file: Option<PathBuf>,

    /// Device to simulate: mobile | desktop
    #[arg(long)]
    device: Option<String>,

    /// Number of Lighthouse samples per URL
    #[arg(long)]
    samples: Option<usize>,

    /// Enable throttling
    #[arg(long)]
    throttle: bool,

    /// Maximum number of routes to scan
    #[arg(long)]
    max_routes: Option<usize>,

    /// Reporter format: json | csv | jsonExpanded | none
    #[arg(long)]
    reporter: Option<String>,

    /// Build static output (CI mode)
    #[arg(long)]
    build_static: bool,

    /// Score budget (0–100). Exit code 1 if average score is below this.
    #[arg(long)]
    budget: Option<f64>,

    /// Number of concurrent Lighthouse workers
    #[arg(long)]
    workers: Option<usize>,

    /// Run in CI mode (no HTTP server, write report and exit)
    #[arg(long)]
    ci: bool,

    /// HTTP server port
    #[arg(long, default_value = "5678")]
    port: u16,

    /// HTTP server host
    #[arg(long, default_value = "localhost")]
    host: String,

    /// Path to the Lighthouse Node.js process script
    #[arg(long)]
    lighthouse_process_path: Option<String>,

    /// Paths to include (comma-separated)
    #[arg(long, value_delimiter = ',')]
    include: Vec<String>,

    /// Paths to exclude (comma-separated)
    #[arg(long, value_delimiter = ',')]
    exclude: Vec<String>,

    /// Skip JavaScript execution (experimental)
    #[arg(long)]
    skip_javascript: bool,

    /// Navigate to the URL before running Lighthouse
    #[arg(long)]
    warmup: bool,

    /// Block images/fonts to speed up the audit
    #[arg(long)]
    block_assets: bool,

    /// Backend to use for HTML inspection.
    /// reqwest (default) is fast and needs no Chrome for step 1.
    /// headless_chrome is stable with all Chrome versions.
    /// chromiumoxide is async/tokio-native but may crash on some Chrome builds.
    #[arg(long, default_value = "reqwest",
          value_parser = ["reqwest", "headless_chrome", "chromiumoxide"])]
    browser: String,

    /// Start as an MCP (Model Context Protocol) server
    #[arg(long)]
    mcp: bool,

    /// LHCI server host (e.g. https://lhci.example.com)
    #[arg(long)]
    lhci_host: Option<String>,

    /// LHCI build token
    #[arg(long)]
    lhci_build_token: Option<String>,

    /// LHCI server basic auth token
    #[arg(long)]
    lhci_auth: Option<String>,

    /// Google CrUX History API key (env: CRUX_API_TOKEN).
    /// Required to fetch CrUX history data directly from the Google API.
    #[arg(long, env = "CRUX_API_TOKEN")]
    crux_api_token: Option<String>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up tracing
    let log_level = if cli.debug { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
        )
        .init();

    // Build config
    let overrides = CliOverrides {
        site: cli.site.clone(),
        output_path: cli.output_path.clone(),
        debug: cli.debug.then_some(true),
        no_cache: cli.no_cache.then_some(true),
        device: cli.device.clone(),
        samples: cli.samples,
        throttle: cli.throttle.then_some(true),
        max_routes: cli.max_routes,
        reporter: cli.reporter.clone(),
        build_static: cli.build_static.then_some(true),
        budget: cli.budget,
        workers: cli.workers,
        ci: cli.ci.then_some(true),
        port: Some(cli.port),
        host: Some(cli.host.clone()),
        lighthouse_process_path: cli.lighthouse_process_path.clone(),
        include: (!cli.include.is_empty()).then(|| cli.include.clone()),
        exclude: (!cli.exclude.is_empty()).then(|| cli.exclude.clone()),
        skip_javascript: cli.skip_javascript.then_some(true),
        warmup: cli.warmup.then_some(true),
        block_assets: cli.block_assets.then_some(true),
        lhci_host: cli.lhci_host.clone(),
        lhci_build_token: cli.lhci_build_token.clone(),
        lhci_auth: cli.lhci_auth.clone(),
        crux_api_token: cli.crux_api_token.clone(),
    };

    let config = config::load_config(cli.config_file.as_ref(), overrides)
        .context("Failed to load configuration")?;

    if config.site.is_empty() {
        anyhow::bail!("--site is required. Provide a URL like https://example.com");
    }

    // If running in normal server mode, bind to the port early to fail fast
    // if the address is already in use, preventing unnecessary heavy initialization.
    let listener = if !config.ci.enabled && !cli.mcp {
        let addr = tokio::net::lookup_host(format!("{}:{}", config.host, config.port))
            .await
            .context("Failed to resolve host")?
            .next()
            .ok_or_else(|| anyhow::anyhow!("No address found for {}:{}", config.host, config.port))?;

        let l = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("Failed to bind to address {addr}"))?;
        Some(l)
    } else {
        None
    };

    info!("unlighthouse-rs starting — site: {}", config.site);
    info!(
        "Output path: {} | Workers: {} | CI mode: {}",
        config.output_path, config.workers, config.ci.enabled
    );

    // Validate output path before doing any work.
    let output_path = std::path::Path::new(&config.output_path);
    if output_path.is_absolute() {
        // Guard against obvious system-path accidents on Unix.
        for forbidden in &["/sys", "/proc", "/dev", "/etc"] {
            if config.output_path.starts_with(forbidden) {
                anyhow::bail!("Refusing to write to system path: {}", config.output_path);
            }
        }
    }

    let config = Arc::new(config);

    // Create output directory
    tokio::fs::create_dir_all(&config.output_path).await?;

    // ── Launch inspection backend ─────────────────────────────────────────────
    info!(backend = %cli.browser, "Initialising HTML inspection backend...");
    let browser = match cli.browser.as_str() {
        "chromiumoxide" => {
            launch_chromiumoxide(&config)
                .await
                .context("Failed to launch Chrome via chromiumoxide")?
        }
        "headless_chrome" => {
            launch_headless_chrome(&config)
                .context("Failed to launch Chrome via headless_chrome")?
        }
        _ => {
            launch_reqwest(&config)
                .context("Failed to create reqwest client")?
        }
    };
    info!(backend = %cli.browser, "HTML inspection backend ready");

    // Work channel (routes to process)
    let (work_tx, work_rx) = tokio::sync::mpsc::channel::<types::NormalisedRoute>(1024);

    // App state (shared with server + workers)
    // Initialize the persistent worker pool if configured
    let pool = if !config.lighthouse_process_path.is_empty() && config.workers > 0 {
        match queue::pool::LighthousePool::new(&config, config.workers).await {
            Ok(p) => Some(Arc::new(p)),
            Err(e) => {
                warn!("Failed to initialize persistent worker pool: {e}. Falling back to one-off processes.");
                None
            }
        }
    } else {
        None
    };

    // App state (shared with server + workers)
    let state = Arc::new(server::AppState::new(Arc::clone(&config), work_tx.clone(), pool));

    // ── Route discovery (uses reqwest — no browser needed for sitemaps/robots) ─
    info!("Starting route discovery...");
    let (initial_routes, _disc_client, _robots_rules) =
        resolve_reportable_routes(&config).await.context("Route discovery failed")?;

    info!("Discovered {} initial routes", initial_routes.len());

    // Queue all discovered routes
    for route in &initial_routes {
        let _ = work_tx.send(route.clone()).await;
    }

    // ── Worker loop (uses chromiumoxide for HTML inspection) ──────────────────
    let worker_state = state.clone();
    let worker_browser = browser.clone();
    let worker_handle = tokio::spawn(async move {
        run_worker_loop(worker_state, worker_browser, work_rx).await;
    });

    // ── CI mode: no server, wait for completion, write report, exit ──────────
    if config.ci.enabled {
        return run_ci_mode(state, config, worker_handle).await;
    }

    // ── Server mode: start HTTP server or MCP server + keep running ───────────
    if cli.mcp {
        info!("Starting MCP server...");
        server::mcp::run_mcp_server(state.clone()).await?;
    } else {
        let listener = listener.ok_or_else(|| anyhow::anyhow!("TCP listener was not initialized"))?;
        let addr = listener.local_addr()?;
        info!(
            "Dashboard: http://{}{}",
            addr,
            if config.router_prefix.is_empty() {
                String::new()
            } else {
                config.router_prefix.clone()
            }
        );
        start_server(state, listener).await?;
    }
    Ok(())
}

// ── CI mode implementation ────────────────────────────────────────────────────

async fn run_ci_mode(
    state: Arc<AppState>,
    config: Arc<Config>,
    worker_handle: tokio::task::JoinHandle<()>,
) -> Result<()> {
    info!("CI mode: waiting for all scans to complete...");

    // Poll until all routes are done
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let reports = state.route_reports.read().await;
        if reports.is_empty() {
            // Nothing queued yet — wait a bit
            drop(reports);
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            continue;
        }

        let all_done = reports.values().all(|r| {
            matches!(
                r.tasks.run_lighthouse_task,
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Ignore
            )
        });

        if all_done {
            info!("All {} routes scanned.", reports.len());
            break;
        }

        let done = reports
            .values()
            .filter(|r| {
                matches!(
                    r.tasks.run_lighthouse_task,
                    TaskStatus::Completed | TaskStatus::Failed
                )
            })
            .count();
        let total = reports.len();
        drop(reports);
        info!("Progress: {done}/{total}");
    }

    worker_handle.abort();

    let reports: Vec<_> = {
        let r = state.route_reports.read().await;
        r.values().cloned().collect()
    };

    // Write report
    if config.ci.reporter != ReporterType::None {
        match write_report(&reports, &config).await {
            Ok(Some(path)) => {
                if config.ci.reporter == ReporterType::Lhci {
                    info!("LHCI Upload complete. Build comparison URL: {path}");
                } else {
                    info!("Report written: {path}");
                }
            }
            Ok(None) => {}
            Err(e) => error!("Failed to write report: {e}"),
        }
    }

    // Check budget
    if let Some(budget) = config.ci.budget {
        let scores: Vec<f64> = reports
            .iter()
            .filter_map(|r| r.report.as_ref().map(|rep| rep.score * 100.0))
            .collect();

        if scores.is_empty() {
            warn!("No scores available to check budget");
        } else {
            let avg = scores.iter().sum::<f64>() / scores.len() as f64;
            info!("Average score: {avg:.1} (budget: {budget})");
            if avg < budget {
                error!("Budget exceeded: average score {avg:.1} is below budget {budget}");
                std::process::exit(1);
            } else {
                info!("Budget check passed: {avg:.1} >= {budget}");
            }
        }
    }

    Ok(())
}
