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
    Markdown,
    Lhci,
}

impl std::str::FromStr for ReporterType {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "jsonSimple" | "json-simple" | "json" => Ok(Self::JsonSimple),
            "jsonExpanded" | "json-expanded" => Ok(Self::JsonExpanded),
            "csvSimple" | "csv-simple" | "csv" => Ok(Self::CsvSimple),
            "markdown" | "md" => Ok(Self::Markdown),
            "lhci" | "lhciServer" | "lhci-server" => Ok(Self::Lhci),
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
    pub warmup: bool,
    pub block_assets: bool,
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
            warmup: false,
            block_assets: false,
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
    pub lhci_host: Option<String>,
    pub lhci_build_token: Option<String>,
    pub lhci_auth: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuthConfig {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CookieConfig {
    pub name: String,
    pub value: String,
    pub domain: Option<String>,
    pub path: Option<String>,
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
    pub auth: Option<AuthConfig>,
    pub cookies: Option<Vec<CookieConfig>>,
    pub local_storage: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub session_storage: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub extra_headers: Option<std::collections::HashMap<String, String>>,
    pub user_agent: Option<String>,
    /// Google CrUX History API key. When set the Rust binary calls the CrUX API
    /// directly; when absent it falls back to proxying crux.unlighthouse.dev.
    pub crux_api_token: Option<String>,
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
            auth: None,
            cookies: None,
            local_storage: None,
            session_storage: None,
            extra_headers: None,
            user_agent: None,
            crux_api_token: None,
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
    auth: Option<AuthConfig>,
    cookies: Option<Vec<CookieConfig>>,
    local_storage: Option<std::collections::HashMap<String, serde_json::Value>>,
    session_storage: Option<std::collections::HashMap<String, serde_json::Value>>,
    extra_headers: Option<std::collections::HashMap<String, String>>,
    user_agent: Option<String>,
    crux_api_token: Option<String>,
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
    warmup: Option<bool>,
    block_assets: Option<bool>,
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
    lhci_host: Option<String>,
    lhci_build_token: Option<String>,
    lhci_auth: Option<String>,
}

// ── CLI args (passed in after parsing) ────────────────────────────────────────

#[derive(Debug, Default)]
pub struct CliOverrides {
    pub site: Option<String>,
    pub output_path: Option<String>,
    pub debug: Option<bool>,
    pub no_cache: Option<bool>,
    pub device: Option<String>,
    pub samples: Option<usize>,
    pub throttle: Option<bool>,
    pub max_routes: Option<usize>,
    pub reporter: Option<String>,
    pub build_static: Option<bool>,
    pub budget: Option<f64>,
    pub workers: Option<usize>,
    pub ci: Option<bool>,
    pub port: Option<u16>,
    pub host: Option<String>,
    pub lighthouse_process_path: Option<String>,
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    pub skip_javascript: Option<bool>,
    pub warmup: Option<bool>,
    pub block_assets: Option<bool>,
    pub lhci_host: Option<String>,
    pub lhci_build_token: Option<String>,
    pub lhci_auth: Option<String>,
    pub crux_api_token: Option<String>,
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

macro_rules! merge_opt {
    ($target:expr, $source:expr) => {
        if let Some(v) = $source {
            $target = v.into();
        }
    };
}

macro_rules! merge_parse {
    ($target:expr, $source:expr, $err_msg:expr) => {
        if let Some(v) = $source {
            $target = v.parse().with_context(|| $err_msg)?;
        }
    };
}

fn apply_file_config(config: &mut Config, fc: FileConfig) -> Result<()> {
    merge_opt!(config.site, fc.site);
    merge_opt!(config.output_path, fc.output_path);
    merge_opt!(config.debug, fc.debug);
    merge_opt!(config.cache, fc.cache);
    merge_opt!(config.router_prefix, fc.router_prefix);
    merge_opt!(config.api_prefix, fc.api_prefix);
    merge_opt!(config.port, fc.port);
    merge_opt!(config.host, fc.host);
    merge_opt!(config.lighthouse_process_path, fc.lighthouse_process_path);
    merge_opt!(config.workers, fc.workers);
    merge_opt!(config.auth, fc.auth);
    merge_opt!(config.cookies, fc.cookies);
    merge_opt!(config.local_storage, fc.local_storage);
    merge_opt!(config.session_storage, fc.session_storage);
    merge_opt!(config.extra_headers, fc.extra_headers);
    merge_opt!(config.user_agent, fc.user_agent);
    merge_opt!(config.crux_api_token, fc.crux_api_token);

    if let Some(sc) = fc.scanner {
        merge_opt!(config.scanner.max_routes, sc.max_routes);
        merge_opt!(config.scanner.crawler, sc.crawler);
        merge_opt!(config.scanner.sitemap, sc.sitemap);
        merge_opt!(config.scanner.robots_txt, sc.robots_txt);
        merge_opt!(config.scanner.dynamic_sampling, sc.dynamic_sampling);
        merge_opt!(config.scanner.samples, sc.samples);
        merge_opt!(config.scanner.throttle, sc.throttle);
        merge_opt!(config.scanner.skip_javascript, sc.skip_javascript);
        merge_opt!(config.scanner.warmup, sc.warmup);
        merge_opt!(config.scanner.block_assets, sc.block_assets);
        merge_opt!(config.scanner.exclude, sc.exclude);
        merge_opt!(config.scanner.include, sc.include);
        merge_parse!(config.scanner.device, sc.device, "Invalid device value in config file");
    }

    if let Some(ci) = fc.ci {
        merge_opt!(config.ci.budget, ci.budget);
        merge_opt!(config.ci.build_static, ci.build_static);
        merge_opt!(config.ci.enabled, ci.enabled);
        merge_opt!(config.ci.lhci_host, ci.lhci_host);
        merge_opt!(config.ci.lhci_build_token, ci.lhci_build_token);
        merge_opt!(config.ci.lhci_auth, ci.lhci_auth);
        merge_parse!(config.ci.reporter, ci.reporter, "Invalid reporter value in config file");
    }

    Ok(())
}

fn apply_cli_overrides(config: &mut Config, cli: CliOverrides) -> Result<()> {
    merge_opt!(config.site, cli.site);
    merge_opt!(config.output_path, cli.output_path);
    merge_opt!(config.debug, cli.debug);
    if let Some(no_cache) = cli.no_cache { config.cache = !no_cache; }
    merge_opt!(config.scanner.samples, cli.samples);
    merge_opt!(config.scanner.throttle, cli.throttle);
    merge_opt!(config.scanner.max_routes, cli.max_routes);
    merge_opt!(config.workers, cli.workers);
    merge_opt!(config.ci.budget, cli.budget);
    merge_opt!(config.ci.build_static, cli.build_static);
    merge_opt!(config.ci.enabled, cli.ci);
    merge_opt!(config.port, cli.port);
    merge_opt!(config.host, cli.host);
    merge_opt!(config.lighthouse_process_path, cli.lighthouse_process_path);
    merge_opt!(config.scanner.include, cli.include);
    merge_opt!(config.scanner.exclude, cli.exclude);
    merge_opt!(config.scanner.skip_javascript, cli.skip_javascript);
    merge_opt!(config.scanner.warmup, cli.warmup);
    merge_opt!(config.scanner.block_assets, cli.block_assets);
    merge_opt!(config.ci.lhci_host, cli.lhci_host);
    merge_opt!(config.ci.lhci_build_token, cli.lhci_build_token);
    merge_opt!(config.ci.lhci_auth, cli.lhci_auth);
    merge_opt!(config.crux_api_token, cli.crux_api_token);

    merge_parse!(config.scanner.device, cli.device, "Invalid device value in CLI");
    merge_parse!(config.ci.reporter, cli.reporter, "Invalid reporter value in CLI");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reporter_type_parsing() {
        assert_eq!("json".parse::<ReporterType>().unwrap(), ReporterType::JsonSimple);
        assert_eq!("json-expanded".parse::<ReporterType>().unwrap(), ReporterType::JsonExpanded);
        assert_eq!("markdown".parse::<ReporterType>().unwrap(), ReporterType::Markdown);
        assert_eq!("none".parse::<ReporterType>().unwrap(), ReporterType::None);
        assert!("invalid".parse::<ReporterType>().is_err());
    }

    #[test]
    fn test_device_parsing() {
        assert_eq!("mobile".parse::<Device>().unwrap(), Device::Mobile);
        assert_eq!("desktop".parse::<Device>().unwrap(), Device::Desktop);
        assert!("tablet".parse::<Device>().is_err());
    }

    #[test]
    fn test_merge_opt_macro() {
        let mut target = "original".to_string();
        let source = Some("new".to_string());
        merge_opt!(target, source);
        assert_eq!(target, "new");

        let source_none: Option<String> = None;
        merge_opt!(target, source_none);
        assert_eq!(target, "new"); // No change

        // Test into() conversion (String to Option<String>)
        let mut target_opt = Some("original".to_string());
        let source = Some("new".to_string());
        merge_opt!(target_opt, source);
        assert_eq!(target_opt, Some("new".to_string()));
    }

    #[test]
    fn test_apply_cli_overrides() {
        let mut config = Config::default();
        let overrides = CliOverrides {
            site: Some("https://overridden.com".to_string()),
            debug: Some(true),
            no_cache: Some(true),
            device: Some("desktop".to_string()),
            ..Default::default()
        };

        apply_cli_overrides(&mut config, overrides).unwrap();

        assert_eq!(config.site, "https://overridden.com");
        assert!(config.debug);
        assert!(!config.cache); // no_cache = true means cache = false
        assert_eq!(config.scanner.device, Device::Desktop);
    }

    #[test]
    fn test_apply_file_config() {
        let mut config = Config::default();
        let fc = FileConfig {
            site: Some("https://file.com".to_string()),
            port: Some(9999),
            scanner: Some(FileScannerConfig {
                max_routes: Some(10),
                samples: Some(3),
                ..Default::default()
            }),
            ..Default::default()
        };

        apply_file_config(&mut config, fc).unwrap();

        assert_eq!(config.site, "https://file.com");
        assert_eq!(config.port, 9999);
        assert_eq!(config.scanner.max_routes, Some(10));
        assert_eq!(config.scanner.samples, 3);
    }
}
