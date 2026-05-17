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

// ── Scan mode ─────────────────────────────────────────────────────────────────

/// Controls how each page is audited.
///
/// `Full` (default) — runs the Lighthouse Node.js subprocess for complete
/// performance, accessibility, best-practices and SEO scores.
///
/// `Fast` — skips Lighthouse entirely.  Instead, Core Web Vitals (FCP, LCP,
/// CLS, TTFB, TBT) are measured natively via the browser's PerformanceObserver
/// API through chromiumoxide or headless_chrome.  No Node.js process is
/// spawned, making scans significantly faster.  Use `--browser chromiumoxide`
/// or `--browser headless_chrome` together with `--mode fast`; the reqwest
/// backend will still perform HTML inspection but cannot measure vitals.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScanMode {
    #[default]
    Full,
    Fast,
}

impl std::str::FromStr for ScanMode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "full" => Ok(Self::Full),
            "fast" => Ok(Self::Fast),
            other => anyhow::bail!("Unknown mode: {other:?}. Expected: full | fast"),
        }
    }
}

// ── Score budget rules ────────────────────────────────────────────────────────

/// A per-path score budget.  The first rule whose `path` glob matches a
/// route's path is applied.  Scores are 0–100 (not 0–1).
///
/// Example in `unlighthouse.config.toml`:
/// ```toml
/// [[budgets]]
/// path = "/checkout/**"
/// performance = 90
/// accessibility = 85
///
/// [[budgets]]
/// path = "/**"
/// performance = 70
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetRule {
    /// Glob pattern matched against the route path (e.g. `/blog/**`).
    pub path: String,
    /// Minimum allowed composite score (0–100).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accessibility: Option<f64>,
    #[serde(rename = "bestPractices", skip_serializing_if = "Option::is_none")]
    pub best_practices: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seo: Option<f64>,
}

impl BudgetRule {
    /// Returns a list of `(label, threshold, actual)` for every category that
    /// failed the budget check.  `actual` is already multiplied to 0–100.
    pub fn violations(
        &self,
        categories: &std::collections::HashMap<String, crate::types::LighthouseCategoryScore>,
        composite_score: f64,
    ) -> Vec<BudgetViolation> {
        let mut out = Vec::new();

        let check = |label: &str, threshold: Option<f64>, key: &str| -> Option<BudgetViolation> {
            let t = threshold?;
            let actual = categories.get(key)?.score? * 100.0;
            if actual < t {
                Some(BudgetViolation { label: label.to_string(), threshold: t, actual })
            } else {
                None
            }
        };

        if let Some(t) = self.score {
            let actual = composite_score * 100.0;
            if actual < t {
                out.push(BudgetViolation { label: "score".to_string(), threshold: t, actual });
            }
        }
        if let Some(v) = check("performance", self.performance, "performance") { out.push(v); }
        if let Some(v) = check("accessibility", self.accessibility, "accessibility") { out.push(v); }
        if let Some(v) = check("best-practices", self.best_practices, "best-practices") { out.push(v); }
        if let Some(v) = check("seo", self.seo, "seo") { out.push(v); }

        out
    }
}

#[derive(Debug, Clone)]
pub struct BudgetViolation {
    pub label: String,
    pub threshold: f64,
    pub actual: f64,
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
    /// Google CrUX History API key. Required to fetch CrUX history data directly.
    pub crux_api_token: Option<String>,
    /// Audit mode: `Full` (default) runs Lighthouse; `Fast` measures Web Vitals via CDP.
    pub mode: ScanMode,
    /// Per-path score budget rules evaluated in CI mode.
    pub budgets: Vec<BudgetRule>,
    /// Purge database runs older than this number of days (default: 30)
    pub purge_runs_older_than_days: Option<i64>,
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
            #[cfg(feature = "native")]
            workers: (num_cpus::get() / 2).max(1),
            #[cfg(not(feature = "native"))]
            workers: 1,
            auth: None,
            cookies: None,
            local_storage: None,
            session_storage: None,
            extra_headers: None,
            user_agent: None,
            crux_api_token: None,
            mode: ScanMode::Full,
            budgets: Vec::new(),
            purge_runs_older_than_days: Some(30),
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
    mode: Option<String>,
    budgets: Option<Vec<BudgetRule>>,
    purge_runs_older_than_days: Option<i64>,
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
    pub mode: Option<String>,
    pub purge_runs_older_than_days: Option<i64>,
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
    merge_parse!(config.mode, fc.mode, "Invalid mode value in config file");
    if let Some(b) = fc.budgets { config.budgets = b; }
    merge_opt!(config.purge_runs_older_than_days, fc.purge_runs_older_than_days);

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
    merge_parse!(config.mode, cli.mode, "Invalid mode value in CLI");
    merge_opt!(config.purge_runs_older_than_days, cli.purge_runs_older_than_days);

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

    #[test]
    fn test_all_config_options_file_and_cli_merging() {
        let mut config = Config::default();

        // 1. Verify Default Config values
        assert_eq!(config.site, "");
        assert_eq!(config.port, 5678);
        assert_eq!(config.scanner.device, Device::Mobile);
        assert!(config.scanner.crawler);
        assert_eq!(config.mode, ScanMode::Full);
        assert!(config.budgets.is_empty());
        assert_eq!(config.scanner.exclude.len(), 0);
        assert_eq!(config.scanner.include.len(), 0);

        // 2. Mock a complete FileConfig and apply it
        let mut local_storage = std::collections::HashMap::new();
        local_storage.insert("key".to_string(), serde_json::Value::String("val".to_string()));
        let mut extra_headers = std::collections::HashMap::new();
        extra_headers.insert("Header".to_string(), "Value".to_string());

        let budgets = vec![BudgetRule {
            path: "/checkout/**".to_string(),
            score: Some(90.0),
            performance: Some(85.0),
            accessibility: None,
            best_practices: None,
            seo: None,
        }];

        let file_cfg = FileConfig {
            site: Some("https://file-site.com".to_string()),
            output_path: Some(".unlighthouse-file".to_string()),
            debug: Some(true),
            cache: Some(false),
            router_prefix: Some("/sub".to_string()),
            api_prefix: Some("/api-sub/".to_string()),
            port: Some(8888),
            host: Some("127.0.0.1".to_string()),
            lighthouse_process_path: Some("/bin/lighthouse".to_string()),
            workers: Some(4),
            auth: Some(AuthConfig {
                username: "user".to_string(),
                password: "pass".to_string(),
            }),
            cookies: Some(vec![CookieConfig {
                name: "session".to_string(),
                value: "abc".to_string(),
                domain: Some("domain.com".to_string()),
                path: Some("/".to_string()),
            }]),
            local_storage: Some(local_storage),
            session_storage: None,
            extra_headers: Some(extra_headers),
            user_agent: Some("agent-x".to_string()),
            crux_api_token: Some("file-token".to_string()),
            mode: Some("fast".to_string()),
            budgets: Some(budgets),
            purge_runs_older_than_days: Some(15),
            scanner: Some(FileScannerConfig {
                max_routes: Some(500),
                crawler: Some(false),
                sitemap: Some(false),
                robots_txt: Some(false),
                dynamic_sampling: Some(10),
                samples: Some(2),
                throttle: Some(true),
                device: Some("desktop".to_string()),
                skip_javascript: Some(true),
                warmup: Some(true),
                block_assets: Some(true),
                exclude: Some(vec!["/admin/**".to_string()]),
                include: Some(vec!["/blog/**".to_string()]),
            }),
            ci: Some(FileCiConfig {
                budget: Some(80.0),
                build_static: Some(true),
                reporter: Some("markdown".to_string()),
                enabled: Some(true),
                lhci_host: Some("lhci.com".to_string()),
                lhci_build_token: Some("lhci-token".to_string()),
                lhci_auth: Some("lhci-auth".to_string()),
            }),
        };

        apply_file_config(&mut config, file_cfg).unwrap();

        // Verify FileConfig took effect perfectly
        assert_eq!(config.site, "https://file-site.com");
        assert_eq!(config.output_path, ".unlighthouse-file");
        assert!(config.debug);
        assert!(!config.cache);
        assert_eq!(config.router_prefix, "/sub");
        assert_eq!(config.api_prefix, "/api-sub/");
        assert_eq!(config.port, 8888);
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.lighthouse_process_path, "/bin/lighthouse");
        assert_eq!(config.workers, 4);
        assert_eq!(config.auth.as_ref().unwrap().username, "user");
        assert_eq!(config.cookies.as_ref().unwrap()[0].name, "session");
        assert_eq!(config.local_storage.as_ref().unwrap().get("key").unwrap().as_str().unwrap(), "val");
        assert_eq!(config.extra_headers.as_ref().unwrap().get("Header").unwrap(), "Value");
        assert_eq!(config.user_agent.as_ref().unwrap(), "agent-x");
        assert_eq!(config.crux_api_token.as_ref().unwrap(), "file-token");
        assert_eq!(config.mode, ScanMode::Fast);
        assert_eq!(config.purge_runs_older_than_days, Some(15));
        assert_eq!(config.budgets[0].path, "/checkout/**");
        assert_eq!(config.scanner.max_routes, Some(500));
        assert!(!config.scanner.crawler);
        assert!(!config.scanner.sitemap);
        assert!(!config.scanner.robots_txt);
        assert_eq!(config.scanner.dynamic_sampling, Some(10));
        assert_eq!(config.scanner.samples, 2);
        assert!(config.scanner.throttle);
        assert_eq!(config.scanner.device, Device::Desktop);
        assert!(config.scanner.skip_javascript);
        assert!(config.scanner.warmup);
        assert!(config.scanner.block_assets);
        assert_eq!(config.scanner.exclude[0], "/admin/**");
        assert_eq!(config.scanner.include[0], "/blog/**");
        assert_eq!(config.ci.budget, Some(80.0));
        assert!(config.ci.build_static);
        assert_eq!(config.ci.reporter, ReporterType::Markdown);
        assert!(config.ci.enabled);
        assert_eq!(config.ci.lhci_host.as_ref().unwrap(), "lhci.com");
        assert_eq!(config.ci.lhci_build_token.as_ref().unwrap(), "lhci-token");
        assert_eq!(config.ci.lhci_auth.as_ref().unwrap(), "lhci-auth");

        // 3. Mock a complete CliOverrides and apply it
        let cli_overrides = CliOverrides {
            site: Some("https://cli-site.com".to_string()),
            output_path: Some(".unlighthouse-cli".to_string()),
            debug: Some(false),
            no_cache: Some(false), // means cache will be true (original cache is false)
            device: Some("mobile".to_string()),
            samples: Some(5),
            throttle: Some(false),
            max_routes: Some(100),
            reporter: Some("lhci".to_string()),
            build_static: Some(false),
            budget: Some(95.0),
            workers: Some(8),
            ci: Some(false),
            port: Some(7777),
            host: Some("0.0.0.0".to_string()),
            lighthouse_process_path: Some("/bin/cli-lh".to_string()),
            include: Some(vec!["/cli-include".to_string()]),
            exclude: Some(vec!["/cli-exclude".to_string()]),
            skip_javascript: Some(bool_to_option(false)),
            warmup: Some(false),
            block_assets: Some(false),
            lhci_host: Some("cli-lhci.com".to_string()),
            lhci_build_token: Some("cli-lhci-token".to_string()),
            lhci_auth: Some("cli-lhci-auth".to_string()),
            crux_api_token: Some("cli-token".to_string()),
            mode: Some("full".to_string()),
            purge_runs_older_than_days: Some(60),
        };

        apply_cli_overrides(&mut config, cli_overrides).unwrap();

        // Verify CLI took priority perfectly
        assert_eq!(config.site, "https://cli-site.com");
        assert_eq!(config.output_path, ".unlighthouse-cli");
        assert!(!config.debug);
        assert!(config.cache); // no_cache = false means cache = true
        assert_eq!(config.scanner.device, Device::Mobile);
        assert_eq!(config.scanner.samples, 5);
        assert!(!config.scanner.throttle);
        assert_eq!(config.scanner.max_routes, Some(100));
        assert_eq!(config.ci.reporter, ReporterType::Lhci);
        assert!(!config.ci.build_static);
        assert_eq!(config.ci.budget, Some(95.0));
        assert_eq!(config.workers, 8);
        assert!(!config.ci.enabled);
        assert_eq!(config.port, 7777);
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.lighthouse_process_path, "/bin/cli-lh");
        assert_eq!(config.scanner.include[0], "/cli-include");
        assert_eq!(config.scanner.exclude[0], "/cli-exclude");
        assert!(!config.scanner.skip_javascript);
        assert!(!config.scanner.warmup);
        assert!(!config.scanner.block_assets);
        assert_eq!(config.ci.lhci_host.as_ref().unwrap(), "cli-lhci.com");
        assert_eq!(config.ci.lhci_build_token.as_ref().unwrap(), "cli-lhci-token");
        assert_eq!(config.ci.lhci_auth.as_ref().unwrap(), "cli-lhci-auth");
        assert_eq!(config.crux_api_token.as_ref().unwrap(), "cli-token");
        assert_eq!(config.mode, ScanMode::Full);
        assert_eq!(config.purge_runs_older_than_days, Some(60));
    }

    fn bool_to_option(b: bool) -> bool {
        b
    }
}
