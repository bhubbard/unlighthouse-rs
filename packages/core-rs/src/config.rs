use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, info};

// ── Reporter enum ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum ReporterType {
    #[default]
    None,
    JsonSimple,
    JsonExpanded,
    CsvSimple,
}

impl std::str::FromStr for ReporterType {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "jsonSimple" | "json-simple" | "json" => Ok(Self::JsonSimple),
            "jsonExpanded" | "json-expanded" => Ok(Self::JsonExpanded),
            "csvSimple" | "csv-simple" | "csv" => Ok(Self::CsvSimple),
            "none" | "false" => Ok(Self::None),
            other => anyhow::bail!("Unknown reporter: {other}"),
        }
    }
}

// ── Device enum ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    #[default]
    Mobile,
    Desktop,
}

impl std::str::FromStr for Device {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "mobile" => Ok(Self::Mobile),
            "desktop" => Ok(Self::Desktop),
            other => anyhow::bail!("Unknown device: {other:?}. Expected: mobile | desktop"),
        }
    }
}

// ── Scanner sub-config ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScannerConfig {
    pub max_routes: Option<usize>,
    pub crawler: bool,
    pub sitemap: bool,
    pub robots_txt: bool,
    pub dynamic_sampling: Option<usize>,
    pub samples: usize,
    pub throttle: bool,
    pub device: Device,
    pub skip_javascript: bool,
    pub exclude: Vec<String>,
    pub include: Vec<String>,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            max_routes: Some(200),
            crawler: true,
            sitemap: true,
            robots_txt: true,
            dynamic_sampling: Some(5),
            samples: 1,
            throttle: false,
            device: Device::Mobile,
            skip_javascript: false,
            exclude: vec![],
            include: vec![],
        }
    }
}

// ── CI sub-config ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CiConfig {
    pub budget: Option<f64>,
    pub build_static: bool,
    pub reporter: ReporterType,
    pub enabled: bool,
}

// ── Top-level config (file + CLI merged) ──────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub site: String,
    pub output_path: String,
    pub debug: bool,
    pub cache: bool,
    pub router_prefix: String,
    pub api_prefix: String,
    pub port: u16,
    pub host: String,
    pub scanner: ScannerConfig,
    pub lighthouse_process_path: String,
    pub ci: CiConfig,
    pub workers: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            site: String::new(),
            output_path: ".unlighthouse".to_string(),
            debug: false,
            cache: true,
            router_prefix: String::new(),
            api_prefix: "/api/".to_string(),
            port: 5678,
            host: "localhost".to_string(),
            scanner: ScannerConfig::default(),
            lighthouse_process_path: String::new(),
            ci: CiConfig::default(),
            workers: (num_cpus::get() / 2).max(1),
        }
    }
}

// ── File-based partial config (for TOML/JSON) ─────────────────────────────────
// Uses Option<> everywhere so missing keys don't clobber defaults.

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct FileConfig {
    site: Option<String>,
    output_path: Option<String>,
    debug: Option<bool>,
    cache: Option<bool>,
    router_prefix: Option<String>,
    api_prefix: Option<String>,
    port: Option<u16>,
    host: Option<String>,
    lighthouse_process_path: Option<String>,
    workers: Option<usize>,
    scanner: Option<FileScannerConfig>,
    ci: Option<FileCiConfig>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct FileScannerConfig {
    max_routes: Option<usize>,
    crawler: Option<bool>,
    sitemap: Option<bool>,
    robots_txt: Option<bool>,
    dynamic_sampling: Option<usize>,
    samples: Option<usize>,
    throttle: Option<bool>,
    device: Option<String>,
    skip_javascript: Option<bool>,
    exclude: Option<Vec<String>>,
    include: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct FileCiConfig {
    budget: Option<f64>,
    build_static: Option<bool>,
    reporter: Option<String>,
    enabled: Option<bool>,
}

// ── CLI args (passed in after parsing) ────────────────────────────────────────

#[derive(Debug, Default)]
pub struct CliOverrides {
    pub site: Option<String>,
    pub output_path: Option<String>,
    pub debug: bool,
    pub no_cache: bool,
    pub device: Option<String>,
    pub samples: Option<usize>,
    pub throttle: bool,
    pub max_routes: Option<usize>,
    pub reporter: Option<String>,
    pub build_static: bool,
    pub budget: Option<f64>,
    pub workers: Option<usize>,
    pub ci: bool,
    pub port: Option<u16>,
    pub host: Option<String>,
    pub lighthouse_process_path: Option<String>,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

// ── Config loading logic ──────────────────────────────────────────────────────

pub fn load_config(config_file: Option<&PathBuf>, overrides: CliOverrides) -> Result<Config> {
    let mut config = Config::default();

    // 1. Try loading from file
    let candidates = config_file
        .map(|p| vec![p.clone()])
        .unwrap_or_else(|| {
            vec![
                PathBuf::from("unlighthouse.config.toml"),
                PathBuf::from("unlighthouse.config.json"),
            ]
        });

    for candidate in &candidates {
        if candidate.exists() {
            info!("Loading config from {}", candidate.display());
            let text = std::fs::read_to_string(candidate)
                .with_context(|| format!("reading config file {}", candidate.display()))?;

            let file_cfg: FileConfig = if candidate.extension().and_then(|e| e.to_str()) == Some("toml") {
                toml::from_str(&text).with_context(|| "parsing TOML config")?
            } else {
                serde_json::from_str(&text).with_context(|| "parsing JSON config")?
            };

            apply_file_config(&mut config, file_cfg)
                .with_context(|| format!("Invalid value in config file {}", candidate.display()))?;
            break;
        }
    }

    // 2. Apply CLI overrides (highest priority)
    apply_cli_overrides(&mut config, overrides)?;

    debug!("Resolved config: {:?}", config);
    Ok(config)
}

fn apply_file_config(config: &mut Config, fc: FileConfig) -> Result<()> {
    if let Some(v) = fc.site { config.site = v; }
    if let Some(v) = fc.output_path { config.output_path = v; }
    if let Some(v) = fc.debug { config.debug = v; }
    if let Some(v) = fc.cache { config.cache = v; }
    if let Some(v) = fc.router_prefix { config.router_prefix = v; }
    if let Some(v) = fc.api_prefix { config.api_prefix = v; }
    if let Some(v) = fc.port { config.port = v; }
    if let Some(v) = fc.host { config.host = v; }
    if let Some(v) = fc.lighthouse_process_path { config.lighthouse_process_path = v; }
    if let Some(v) = fc.workers { config.workers = v; }

    if let Some(sc) = fc.scanner {
        if let Some(v) = sc.max_routes { config.scanner.max_routes = Some(v); }
        if let Some(v) = sc.crawler { config.scanner.crawler = v; }
        if let Some(v) = sc.sitemap { config.scanner.sitemap = v; }
        if let Some(v) = sc.robots_txt { config.scanner.robots_txt = v; }
        if let Some(v) = sc.dynamic_sampling { config.scanner.dynamic_sampling = Some(v); }
        if let Some(v) = sc.samples { config.scanner.samples = v; }
        if let Some(v) = sc.throttle { config.scanner.throttle = v; }
        if let Some(v) = sc.skip_javascript { config.scanner.skip_javascript = v; }
        if let Some(v) = sc.exclude { config.scanner.exclude = v; }
        if let Some(v) = sc.include { config.scanner.include = v; }
        if let Some(v) = sc.device {
            config.scanner.device = v.parse()
                .with_context(|| format!("Invalid device value in config file: {v:?}. Expected: mobile | desktop"))?;
        }
    }

    if let Some(ci) = fc.ci {
        if let Some(v) = ci.budget { config.ci.budget = Some(v); }
        if let Some(v) = ci.build_static { config.ci.build_static = v; }
        if let Some(v) = ci.enabled { config.ci.enabled = v; }
        if let Some(v) = ci.reporter {
            config.ci.reporter = v.parse()
                .with_context(|| format!("Invalid reporter value in config file: {v:?}. Expected: json | csv | jsonExpanded | none"))?;
        }
    }

    Ok(())
}

fn apply_cli_overrides(config: &mut Config, cli: CliOverrides) -> Result<()> {
    if let Some(v) = cli.site { config.site = v; }
    if let Some(v) = cli.output_path { config.output_path = v; }
    if cli.debug { config.debug = true; }
    if cli.no_cache { config.cache = false; }
    if let Some(v) = cli.samples { config.scanner.samples = v; }
    if cli.throttle { config.scanner.throttle = true; }
    if let Some(v) = cli.max_routes { config.scanner.max_routes = Some(v); }
    if let Some(v) = cli.workers { config.workers = v; }
    if let Some(v) = cli.budget { config.ci.budget = Some(v); }
    if cli.build_static { config.ci.build_static = true; }
    if cli.ci { config.ci.enabled = true; }
    if let Some(v) = cli.port { config.port = v; }
    if let Some(v) = cli.host { config.host = v; }
    if let Some(v) = cli.lighthouse_process_path { config.lighthouse_process_path = v; }
    if !cli.include.is_empty() { config.scanner.include = cli.include; }
    if !cli.exclude.is_empty() { config.scanner.exclude = cli.exclude; }

    if let Some(device_str) = cli.device {
        config.scanner.device = device_str.parse()?;
    }
    if let Some(reporter_str) = cli.reporter {
        config.ci.reporter = reporter_str.parse()?;
    }

    Ok(())
}
