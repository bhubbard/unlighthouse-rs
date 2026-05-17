//! Browser backend abstraction.
//!
//! Three backends, selectable at runtime via `--browser`:
//!
//!   reqwest (default) — pure HTTP fetch + scraper HTML parsing.
//!                       Fast, no Chrome required for HTML inspection.
//!                       Lighthouse still runs its own Chrome for scoring.
//!
//!   headless_chrome   — sync CDP client; stable with all Chrome versions.
//!   chromiumoxide     — async/tokio-native CDP; may crash on some Chrome builds.

use anyhow::Result;
use std::sync::Arc;
use tracing::debug;

use crate::types::SeoData;

// ── reqwest+scraper HTML inspection (default) ─────────────────────────────────

async fn inspect_reqwest(
    client: &reqwest::Client,
    url: &str,
) -> Result<(SeoData, Vec<String>)> {
    let response = client
        .get(url)
        .send()
        .await?;

    // Capture HTTP health data before consuming the response body.
    let status_code = response.status().as_u16();
    let final_url = response.url().to_string();
    let redirect_to = if final_url != url { Some(final_url) } else { None };

    let mut base_url = response.url().clone();
    let html = response.text().await?;
    let html_size = html.len();

    let document = scraper::Html::parse_document(&html);

    // Extract base URL if `<base href="...">` is present.
    let base_sel = scraper::Selector::parse("base[href]").expect("hardcoded CSS selector is valid");
    if let Some(base_el) = document.select(&base_sel).next() {
        if let Some(href) = base_el.value().attr("href") {
            if let Ok(new_base) = base_url.join(href) {
                base_url = new_base;
            }
        }
    }

    // Extract canonical URL if `<link rel="canonical" href="...">` is present.
    let canonical_sel = scraper::Selector::parse("link[rel='canonical']").expect("hardcoded CSS selector is valid");
    let canonical_url = document
        .select(&canonical_sel)
        .next()
        .and_then(|el| el.value().attr("href"))
        .and_then(|href| base_url.join(href).ok())
        .map(|u| u.to_string());

    let title_sel = scraper::Selector::parse("title").expect("hardcoded CSS selector is valid");
    let title = document
        .select(&title_sel)
        .next()
        .map(|e| e.text().collect::<String>())
        .filter(|t| !t.is_empty());

    let desc_sel = scraper::Selector::parse(r#"meta[name="description"]"#).expect("hardcoded CSS selector is valid");
    let description = document
        .select(&desc_sel)
        .next()
        .and_then(|e| e.value().attr("content"))
        .map(str::to_string)
        .filter(|d| !d.is_empty());

    let anchor_sel = scraper::Selector::parse("a[href]").expect("hardcoded CSS selector is valid");
    let origin = format!("{}://{}", base_url.scheme(), base_url.host_str().unwrap_or(""));

    let mut internal_hrefs: Vec<String> = Vec::new();
    let mut external_count = 0usize;

    for el in document.select(&anchor_sel) {
        let href = match el.value().attr("href") {
            Some(h) => h,
            None => continue,
        };
        let resolved = match base_url.join(href) {
            Ok(u) => u.to_string(),
            Err(_) => continue,
        };
        if resolved.starts_with(&origin) {
            internal_hrefs.push(resolved);
        } else if resolved.starts_with("http://") || resolved.starts_with("https://") {
            external_count += 1;
        }
    }

    let internal_count = internal_hrefs.len();

    debug!(
        url,
        title     = ?title,
        internal  = internal_count,
        external  = external_count,
        html_size,
        "HTML inspection complete (reqwest)"
    );

    Ok((
        SeoData {
            title,
            description,
            internal_links: Some(internal_count),
            external_links: Some(external_count),
            html_size:      Some(html_size),
            status_code:    Some(status_code),
            redirect_to,
            canonical_url,
        },
        internal_hrefs,
    ))
}

// ── Shared Chrome inspection script ──────────────────────────────────────────

const INSPECT_SCRIPT: &str = r#"
(() => {
    const origin = window.location.origin;
    const anchors = [...document.querySelectorAll('a[href]')];

    const resolved = anchors.map(a => {
        try { return new URL(a.getAttribute('href'), window.location.href).href; }
        catch (_) { return null; }
    }).filter(Boolean);

    const internalHrefs = resolved.filter(h => h.startsWith(origin));
    const externalCount = resolved.filter(
        h => /^https?:\/\//.test(h) && !h.startsWith(origin)
    ).length;

    // HTTP status via Navigation Timing API (Chrome 109+; null on older builds).
    const navEntry = performance.getEntriesByType('navigation')[0];
    const statusCode = navEntry?.responseStatus ?? null;
    const finalUrl   = navEntry?.name ?? window.location.href;

    return JSON.stringify({
        title:         document.title || null,
        description:   document.querySelector('meta[name="description"]')
                           ?.getAttribute('content') || null,
        internalLinks: internalHrefs.length,
        externalLinks: externalCount,
        htmlSize:      document.documentElement.outerHTML.length,
        hrefs:         internalHrefs,
        statusCode,
        finalUrl,
    });
})()
"#;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct InspectResult {
    title: Option<String>,
    description: Option<String>,
    internal_links: usize,
    external_links: usize,
    html_size: usize,
    hrefs: Vec<String>,
    status_code: Option<u16>,
    final_url: Option<String>,
}

fn parse_cdp_result(json_str: &str, url: &str) -> Result<(SeoData, Vec<String>)> {
    let r: InspectResult = serde_json::from_str(json_str)?;
    let redirect_to = r.final_url.as_deref()
        .filter(|f| *f != url)
        .map(str::to_string);
    debug!(
        url,
        title      = ?r.title,
        internal   = r.internal_links,
        external   = r.external_links,
        html_size  = r.html_size,
        status     = ?r.status_code,
        "HTML inspection complete (CDP)"
    );
    Ok((
        SeoData {
            title:          r.title.filter(|t| !t.is_empty()),
            description:    r.description.filter(|d| !d.is_empty()),
            internal_links: Some(r.internal_links),
            external_links: Some(r.external_links),
            html_size:      Some(r.html_size),
            status_code:    r.status_code,
            redirect_to,
            canonical_url:  None,
        },
        r.hrefs,
    ))
}

// ── Web Vitals measurement (fast mode) ───────────────────────────────────────

/// JavaScript evaluated after page load to collect Core Web Vitals via the
/// PerformanceObserver API with `buffered: true`.  Returns a JSON string.
const WEB_VITALS_SCRIPT: &str = r#"
(() => new Promise(resolve => {
    const m = { fcp: null, lcp: null, cls: 0.0, ttfb: null, tbt: 0.0 };

    // TTFB from Navigation Timing
    const nav = performance.getEntriesByType('navigation')[0];
    if (nav) m.ttfb = nav.responseStart;

    // FCP from Paint Timing
    const fcpEntry = performance.getEntriesByType('paint')
        .find(e => e.name === 'first-contentful-paint');
    if (fcpEntry) m.fcp = fcpEntry.startTime;

    // Helper — start a buffered PerformanceObserver, ignore unsupported types.
    const obs = (type, fn) => {
        try { new PerformanceObserver(fn).observe({ type, buffered: true }); }
        catch (_) {}
    };

    obs('largest-contentful-paint', l => {
        const e = l.getEntries();
        if (e.length) m.lcp = e[e.length - 1].startTime;
    });
    obs('layout-shift', l => {
        for (const e of l.getEntries()) if (!e.hadRecentInput) m.cls += e.value;
    });
    obs('longtask', l => {
        for (const e of l.getEntries()) m.tbt += Math.max(0, e.duration - 50);
    });

    // Give all buffered observers ~1 s to flush their entries, then resolve.
    setTimeout(() => resolve(JSON.stringify(m)), 1000);
}))()
"#;

/// Score a single metric against good/poor thresholds (linear between them).
fn score_metric(value: f64, good: f64, poor: f64) -> f64 {
    if value <= good {
        1.0
    } else if value >= poor {
        0.0
    } else {
        1.0 - (value - good) / (poor - good)
    }
}

/// Compute a composite performance score (0.0–1.0) from available Web Vitals.
/// Weights mirror Lighthouse's approach; missing metrics are excluded from the
/// denominator so partial measurements still produce a meaningful score.
pub fn compute_vitals_score(
    fcp: Option<f64>,
    lcp: Option<f64>,
    cls: Option<f64>,
    ttfb: Option<f64>,
    tbt: Option<f64>,
) -> f64 {
    let mut weighted = 0.0f64;
    let mut total_w = 0.0f64;

    let add = |metric: Option<f64>, good: f64, poor: f64, weight: f64,
               w: &mut f64, tw: &mut f64| {
        if let Some(v) = metric {
            *w  += score_metric(v, good, poor) * weight;
            *tw += weight;
        }
    };

    add(lcp,  2500.0, 4000.0, 0.25, &mut weighted, &mut total_w); // 25 %
    add(cls,  0.1,    0.25,   0.25, &mut weighted, &mut total_w); // 25 %
    add(tbt,  200.0,  600.0,  0.25, &mut weighted, &mut total_w); // 25 %
    add(fcp,  1800.0, 3000.0, 0.15, &mut weighted, &mut total_w); // 15 %
    add(ttfb, 800.0,  1800.0, 0.10, &mut weighted, &mut total_w); // 10 %

    if total_w == 0.0 { 0.0 } else { weighted / total_w }
}

#[derive(serde::Deserialize)]
struct RawVitals {
    fcp:  Option<f64>,
    lcp:  Option<f64>,
    cls:  Option<f64>,
    ttfb: Option<f64>,
    tbt:  Option<f64>,
}

fn parse_vitals(json_str: &str) -> Result<crate::types::WebVitalsSnapshot> {
    let r: RawVitals = serde_json::from_str(json_str)?;
    let score = compute_vitals_score(r.fcp, r.lcp, r.cls, r.ttfb, r.tbt);
    Ok(crate::types::WebVitalsSnapshot { fcp: r.fcp, lcp: r.lcp, cls: r.cls, ttfb: r.ttfb, tbt: r.tbt, score })
}

// ── Browser handle enum ───────────────────────────────────────────────────────

/// A cloneable handle to whichever inspection backend was selected at startup.
#[derive(Clone)]
pub enum BrowserHandle {
    /// Pure HTTP fetch + scraper (default — no Chrome needed for HTML inspection)
    Reqwest(Arc<reqwest::Client>, Arc<crate::config::Config>),
    /// Sync CDP client (stable)
    HeadlessChrome(Arc<headless_chrome::Browser>, Arc<crate::config::Config>),
    /// Async CDP client (may crash on some Chrome builds)
    Chromiumoxide(Arc<chromiumoxide::Browser>, Arc<crate::config::Config>),
}

impl BrowserHandle {
    pub async fn inspect_html(&self, url: &str) -> Result<(SeoData, Vec<String>)> {
        match self {
            BrowserHandle::Reqwest(c, _)          => inspect_reqwest(c, url).await,
            BrowserHandle::HeadlessChrome(b, cfg) => inspect_headless(b, cfg, url).await,
            BrowserHandle::Chromiumoxide(b, cfg)  => inspect_chromiumoxide(b, cfg, url).await,
        }
    }

    /// Measure Core Web Vitals natively via the browser's PerformanceObserver API.
    ///
    /// Navigates to `url`, waits for the load event, then evaluates
    /// [`WEB_VITALS_SCRIPT`] to collect FCP, LCP, CLS, TTFB and TBT.
    ///
    /// Returns `None` for the reqwest backend (no JS engine available).
    pub async fn measure_vitals(
        &self,
        url: &str,
    ) -> Result<Option<crate::types::WebVitalsSnapshot>> {
        match self {
            BrowserHandle::Reqwest(_, _) => Ok(None),
            BrowserHandle::HeadlessChrome(b, cfg) => {
                measure_vitals_headless(b, cfg, url).await.map(Some)
            }
            BrowserHandle::Chromiumoxide(b, cfg) => {
                measure_vitals_chromiumoxide(b, cfg, url).await.map(Some)
            }
        }
    }
}

// ── headless_chrome backend ───────────────────────────────────────────────────

async fn inspect_headless(
    browser: &Arc<headless_chrome::Browser>,
    config: &Arc<crate::config::Config>,
    url: &str,
) -> Result<(SeoData, Vec<String>)> {
    let browser = Arc::clone(browser);
    let config = Arc::clone(config);
    let url = url.to_string();

    tokio::task::spawn_blocking(move || -> Result<(SeoData, Vec<String>)> {
        let tab = browser.new_tab()?;

        use headless_chrome::protocol::cdp::{Network, Page};

        // 1. User Agent injection
        if let Some(ref ua) = config.user_agent {
            tab.call_method(Network::SetUserAgentOverride {
                user_agent: ua.clone(),
                accept_language: None,
                platform: None,
                user_agent_metadata: None,
            })?;
        }

        // 2. Extra HTTP Headers & Basic Auth injection
        let mut headers_map = serde_json::Map::new();
        if let Some(ref extra_headers) = config.extra_headers {
            for (k, v) in extra_headers {
                headers_map.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
        }
        if let Some(ref auth) = config.auth {
            let auth_str = format!("{}:{}", auth.username, auth.password);
            let base64_auth = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, auth_str);
            headers_map.insert("Authorization".to_string(), serde_json::Value::String(format!("Basic {base64_auth}")));
        }
        if !headers_map.is_empty() {
            tab.call_method(Network::SetExtraHTTPHeaders {
                headers: Network::Headers(Some(serde_json::Value::Object(headers_map))),
            })?;
        }

        // 3. Cookies injection
        if let Some(ref cookies) = config.cookies {
            let site_url = url::Url::parse(&config.site).ok();
            let site_host = site_url.as_ref().and_then(|u| u.host_str()).unwrap_or("");
            let cookies_list: Vec<Network::CookieParam> = cookies
                .iter()
                .map(|cookie| Network::CookieParam {
                    name: cookie.name.clone(),
                    value: cookie.value.clone(),
                    domain: Some(cookie.domain.as_deref().unwrap_or(site_host).to_string()),
                    path: Some(cookie.path.as_deref().unwrap_or("/").to_string()),
                    url: None,
                    secure: None,
                    http_only: None,
                    same_site: None,
                    expires: None,
                    priority: None,
                    same_party: None,
                    source_scheme: None,
                    source_port: None,
                    partition_key: None,
                })
                .collect();
            tab.call_method(Network::SetCookies { cookies: cookies_list })?;
        }

        // 4. LocalStorage & SessionStorage injection via Page.addScriptToEvaluateOnNewDocument
        if config.local_storage.is_some() || config.session_storage.is_some() {
            let empty_map = std::collections::HashMap::new();
            let ls = config.local_storage.as_ref().unwrap_or(&empty_map);
            let ss = config.session_storage.as_ref().unwrap_or(&empty_map);
            let source_script = format!(
                r#"
                localStorage.clear();
                const ls = {};
                for (const k in ls) localStorage.setItem(k, typeof ls[k] === 'string' ? ls[k] : JSON.stringify(ls[k]));
                sessionStorage.clear();
                const ss = {};
                for (const k in ss) sessionStorage.setItem(k, typeof ss[k] === 'string' ? ss[k] : JSON.stringify(ss[k]));
                "#,
                serde_json::to_string(&ls).unwrap_or_else(|_| "{}".to_string()),
                serde_json::to_string(&ss).unwrap_or_else(|_| "{}".to_string()),
            );
            tab.call_method(Page::AddScriptToEvaluateOnNewDocument {
                source: source_script,
                world_name: None,
                include_command_line_api: None,
                run_immediately: None,
            })?;
        }

        tab.navigate_to(&url)?;
        tab.wait_until_navigated()?;

        let remote = tab.evaluate(INSPECT_SCRIPT, false)?;
        let json_str = remote
            .value
            .ok_or_else(|| anyhow::anyhow!("Inspect script returned no value for {url}"))?
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Inspect script returned non-string for {url}"))?
            .to_string();

        tab.close(true)?;
        parse_cdp_result(&json_str, &url)
    })
    .await?
}

// ── chromiumoxide backend ─────────────────────────────────────────────────────

async fn inspect_chromiumoxide(
    browser: &Arc<chromiumoxide::Browser>,
    config: &Arc<crate::config::Config>,
    url: &str,
) -> Result<(SeoData, Vec<String>)> {
    let page = browser.new_page(url).await?;

    // 1. User Agent
    if let Some(ref ua) = config.user_agent {
        page.set_user_agent(ua).await.ok();
    }

    // 2. Extra HTTP Headers & Basic Auth
    let mut headers = std::collections::HashMap::new();
    if let Some(ref extra_headers) = config.extra_headers {
        for (k, v) in extra_headers {
            headers.insert(k.clone(), v.clone());
        }
    }
    if let Some(ref auth) = config.auth {
        let auth_str = format!("{}:{}", auth.username, auth.password);
        let base64_auth = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, auth_str);
        headers.insert("Authorization".to_string(), format!("Basic {base64_auth}"));
    }
    if !headers.is_empty() {
        use chromiumoxide::cdp::browser_protocol::network::{Headers as CdpHeaders, SetExtraHttpHeadersParams};
        let headers_val = serde_json::to_value(&headers).unwrap_or_default();
        page.execute(SetExtraHttpHeadersParams::new(CdpHeaders::new(headers_val))).await.ok();
    }

    // 3. Cookies
    if let Some(ref cookies) = config.cookies {
        use chromiumoxide::cdp::browser_protocol::network::{CookieParam as CdpCookieParam, SetCookiesParams};
        let site_url = url::Url::parse(&config.site).ok();
        let site_host = site_url.as_ref().and_then(|u| u.host_str()).unwrap_or("");
        let cdp_cookies: Vec<CdpCookieParam> = cookies
            .iter()
            .filter_map(|cookie| {
                let domain = cookie.domain.as_deref().unwrap_or(site_host).to_string();
                let path = cookie.path.as_deref().unwrap_or("/").to_string();
                CdpCookieParam::builder()
                    .name(cookie.name.clone())
                    .value(cookie.value.clone())
                    .domain(domain)
                    .path(path)
                    .build()
                    .ok()
            })
            .collect();
        if !cdp_cookies.is_empty() {
            page.execute(SetCookiesParams::new(cdp_cookies)).await.ok();
        }
    }

    // 4. LocalStorage & SessionStorage
    if config.local_storage.is_some() || config.session_storage.is_some() {
        let empty_map = std::collections::HashMap::new();
        let ls = config.local_storage.as_ref().unwrap_or(&empty_map);
        let ss = config.session_storage.as_ref().unwrap_or(&empty_map);
        let source_script = format!(
            r#"
            localStorage.clear();
            const ls = {};
            for (const k in ls) localStorage.setItem(k, typeof ls[k] === 'string' ? ls[k] : JSON.stringify(ls[k]));
            sessionStorage.clear();
            const ss = {};
            for (const k in ss) sessionStorage.setItem(k, typeof ss[k] === 'string' ? ss[k] : JSON.stringify(ss[k]));
            "#,
            serde_json::to_string(&ls).unwrap_or_else(|_| "{}".to_string()),
            serde_json::to_string(&ss).unwrap_or_else(|_| "{}".to_string()),
        );
        page.evaluate_on_new_document(source_script).await.ok();
    }

    page.wait_for_navigation().await?;

    let json_str: String = page.evaluate(INSPECT_SCRIPT).await?.into_value()?;
    page.close().await.ok();

    parse_cdp_result(&json_str, url)
}

// ── headless_chrome vitals measurement ───────────────────────────────────────

async fn measure_vitals_headless(
    browser: &Arc<headless_chrome::Browser>,
    config: &Arc<crate::config::Config>,
    url: &str,
) -> Result<crate::types::WebVitalsSnapshot> {
    let browser = Arc::clone(browser);
    let config = Arc::clone(config);
    let url = url.to_string();

    tokio::task::spawn_blocking(move || -> Result<crate::types::WebVitalsSnapshot> {
        let tab = browser.new_tab()?;

        use headless_chrome::protocol::cdp::{Network, Page};

        // 1. User Agent
        if let Some(ref ua) = config.user_agent {
            tab.call_method(Network::SetUserAgentOverride {
                user_agent: ua.clone(),
                accept_language: None,
                platform: None,
                user_agent_metadata: None,
            })?;
        }

        // 2. Extra HTTP headers & Basic Auth
        let mut headers_map = serde_json::Map::new();
        if let Some(ref extra_headers) = config.extra_headers {
            for (k, v) in extra_headers {
                headers_map.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
        }
        if let Some(ref auth) = config.auth {
            let auth_str = format!("{}:{}", auth.username, auth.password);
            let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, auth_str);
            headers_map.insert("Authorization".to_string(), serde_json::Value::String(format!("Basic {b64}")));
        }
        if !headers_map.is_empty() {
            tab.call_method(Network::SetExtraHTTPHeaders {
                headers: Network::Headers(Some(serde_json::Value::Object(headers_map))),
            })?;
        }

        // 3. Cookies
        if let Some(ref cookies) = config.cookies {
            let site_url = url::Url::parse(&config.site).ok();
            let site_host = site_url.as_ref().and_then(|u| u.host_str()).unwrap_or("");
            let cookies_list: Vec<Network::CookieParam> = cookies
                .iter()
                .map(|cookie| Network::CookieParam {
                    name: cookie.name.clone(),
                    value: cookie.value.clone(),
                    domain: Some(cookie.domain.as_deref().unwrap_or(site_host).to_string()),
                    path: Some(cookie.path.as_deref().unwrap_or("/").to_string()),
                    url: None,
                    secure: None,
                    http_only: None,
                    same_site: None,
                    expires: None,
                    priority: None,
                    same_party: None,
                    source_scheme: None,
                    source_port: None,
                    partition_key: None,
                })
                .collect();
            tab.call_method(Network::SetCookies { cookies: cookies_list })?;
        }

        // 4. LocalStorage & SessionStorage
        if config.local_storage.is_some() || config.session_storage.is_some() {
            let empty_map = std::collections::HashMap::new();
            let ls = config.local_storage.as_ref().unwrap_or(&empty_map);
            let ss = config.session_storage.as_ref().unwrap_or(&empty_map);
            let source_script = format!(
                r#"
                localStorage.clear();
                const ls = {};
                for (const k in ls) localStorage.setItem(k, typeof ls[k] === 'string' ? ls[k] : JSON.stringify(ls[k]));
                sessionStorage.clear();
                const ss = {};
                for (const k in ss) sessionStorage.setItem(k, typeof ss[k] === 'string' ? ss[k] : JSON.stringify(ss[k]));
                "#,
                serde_json::to_string(&ls).unwrap_or_else(|_| "{}".to_string()),
                serde_json::to_string(&ss).unwrap_or_else(|_| "{}".to_string()),
            );
            tab.call_method(Page::AddScriptToEvaluateOnNewDocument {
                source: source_script,
                world_name: None,
                include_command_line_api: None,
                run_immediately: None,
            })?;
        }

        tab.navigate_to(&url)?;
        tab.wait_until_navigated()?;

        // Evaluate with await_promise=true — the script returns a Promise that
        // resolves after ~1 second once PerformanceObserver entries have flushed.
        let remote = tab.evaluate(WEB_VITALS_SCRIPT, true)?;
        let json_str = remote
            .value
            .ok_or_else(|| anyhow::anyhow!("Vitals script returned no value for {url}"))?
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Vitals script returned non-string for {url}"))?
            .to_string();

        tab.close(true)?;
        parse_vitals(&json_str)
    })
    .await?
}

// ── chromiumoxide vitals measurement ─────────────────────────────────────────

async fn measure_vitals_chromiumoxide(
    browser: &Arc<chromiumoxide::Browser>,
    config: &Arc<crate::config::Config>,
    url: &str,
) -> Result<crate::types::WebVitalsSnapshot> {
    let page = browser.new_page(url).await?;

    // 1. User Agent
    if let Some(ref ua) = config.user_agent {
        page.set_user_agent(ua).await.ok();
    }

    // 2. Extra HTTP headers & Basic Auth
    let mut headers = std::collections::HashMap::new();
    if let Some(ref extra_headers) = config.extra_headers {
        for (k, v) in extra_headers {
            headers.insert(k.clone(), v.clone());
        }
    }
    if let Some(ref auth) = config.auth {
        let auth_str = format!("{}:{}", auth.username, auth.password);
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, auth_str);
        headers.insert("Authorization".to_string(), format!("Basic {b64}"));
    }
    if !headers.is_empty() {
        use chromiumoxide::cdp::browser_protocol::network::{Headers as CdpHeaders, SetExtraHttpHeadersParams};
        let headers_val = serde_json::to_value(&headers).unwrap_or_default();
        page.execute(SetExtraHttpHeadersParams::new(CdpHeaders::new(headers_val))).await.ok();
    }

    // 3. Cookies
    if let Some(ref cookies) = config.cookies {
        use chromiumoxide::cdp::browser_protocol::network::{CookieParam as CdpCookieParam, SetCookiesParams};
        let site_url = url::Url::parse(&config.site).ok();
        let site_host = site_url.as_ref().and_then(|u| u.host_str()).unwrap_or("");
        let cdp_cookies: Vec<CdpCookieParam> = cookies
            .iter()
            .filter_map(|cookie| {
                let domain = cookie.domain.as_deref().unwrap_or(site_host).to_string();
                let path = cookie.path.as_deref().unwrap_or("/").to_string();
                CdpCookieParam::builder()
                    .name(cookie.name.clone())
                    .value(cookie.value.clone())
                    .domain(domain)
                    .path(path)
                    .build()
                    .ok()
            })
            .collect();
        if !cdp_cookies.is_empty() {
            page.execute(SetCookiesParams::new(cdp_cookies)).await.ok();
        }
    }

    // 4. LocalStorage & SessionStorage
    if config.local_storage.is_some() || config.session_storage.is_some() {
        let empty_map = std::collections::HashMap::new();
        let ls = config.local_storage.as_ref().unwrap_or(&empty_map);
        let ss = config.session_storage.as_ref().unwrap_or(&empty_map);
        let source_script = format!(
            r#"
            localStorage.clear();
            const ls = {};
            for (const k in ls) localStorage.setItem(k, typeof ls[k] === 'string' ? ls[k] : JSON.stringify(ls[k]));
            sessionStorage.clear();
            const ss = {};
            for (const k in ss) sessionStorage.setItem(k, typeof ss[k] === 'string' ? ss[k] : JSON.stringify(ss[k]));
            "#,
            serde_json::to_string(&ls).unwrap_or_else(|_| "{}".to_string()),
            serde_json::to_string(&ss).unwrap_or_else(|_| "{}".to_string()),
        );
        page.evaluate_on_new_document(source_script).await.ok();
    }

    page.wait_for_navigation().await?;

    // chromiumoxide's evaluate() natively awaits Promises, so the 1-second
    // timeout in WEB_VITALS_SCRIPT runs inside the browser before we resume.
    let json_str: String = page.evaluate(WEB_VITALS_SCRIPT).await?.into_value()?;
    page.close().await.ok();

    parse_vitals(&json_str)
}

// ── Backend launch helpers ────────────────────────────────────────────────────

/// Create a reqwest-based handle (default — no Chrome needed for HTML inspection).
pub fn launch_reqwest(config: &Arc<crate::config::Config>) -> Result<BrowserHandle> {
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10));

    if let Some(ref ua) = config.user_agent {
        builder = builder.user_agent(ua);
    } else {
        builder = builder.user_agent("Mozilla/5.0 (compatible; unlighthouse-rs/0.1)");
    }

    let mut header_map = reqwest::header::HeaderMap::new();

    if let Some(ref headers) = config.extra_headers {
        for (k, v) in headers {
            if let (Ok(name), Ok(val)) = (reqwest::header::HeaderName::from_bytes(k.as_bytes()), reqwest::header::HeaderValue::from_str(v)) {
                header_map.insert(name, val);
            }
        }
    }

    if let Some(ref auth) = config.auth {
        let auth_str = format!("{}:{}", auth.username, auth.password);
        let base64_auth = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, auth_str);
        if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Basic {base64_auth}")) {
            header_map.insert(reqwest::header::AUTHORIZATION, val);
        }
    }

    if !header_map.is_empty() {
        builder = builder.default_headers(header_map);
    }

    if let Some(ref cookies) = config.cookies {
        let jar = reqwest::cookie::Jar::default();
        let site_url = reqwest::Url::parse(&config.site).ok();
        for cookie in cookies {
            let mut cookie_str = format!("{}={}", cookie.name, cookie.value);
            if let Some(ref domain) = cookie.domain {
                cookie_str.push_str(&format!("; Domain={}", domain));
            } else if let Some(ref site) = site_url {
                if let Some(host) = site.host_str() {
                    cookie_str.push_str(&format!("; Domain={}", host));
                }
            }
            if let Some(ref path) = cookie.path {
                cookie_str.push_str(&format!("; Path={}", path));
            }
            if let Some(ref site) = site_url {
                jar.add_cookie_str(&cookie_str, site);
            }
        }
        builder = builder.cookie_provider(Arc::new(jar));
    }

    let client = builder.build()?;
    Ok(BrowserHandle::Reqwest(Arc::new(client), Arc::clone(config)))
}

/// Launch a `headless_chrome` browser and return a `BrowserHandle`.
pub fn launch_headless_chrome(config: &Arc<crate::config::Config>) -> Result<BrowserHandle> {
    let browser = headless_chrome::Browser::new(headless_chrome::LaunchOptions {
        headless: true,
        sandbox: false, // safe default; required on Docker/root
        args: vec![
            std::ffi::OsStr::new("--no-first-run"),
            std::ffi::OsStr::new("--no-default-browser-check"),
            std::ffi::OsStr::new("--disable-extensions"),
            std::ffi::OsStr::new("--disable-sync"),
            std::ffi::OsStr::new("--disable-default-apps"),
        ],
        ..Default::default()
    })
    .map_err(|e| anyhow::anyhow!("headless_chrome launch failed: {e}"))?;

    Ok(BrowserHandle::HeadlessChrome(Arc::new(browser), Arc::clone(config)))
}

/// Launch a `chromiumoxide` browser and return a `BrowserHandle`.
/// **Note:** chromiumoxide may crash on `chrome-untrusted://` CDP events
/// emitted by some Chrome versions during startup. Use headless_chrome
/// if you hit `WS Connection error: Serde(...)` on startup.
pub async fn launch_chromiumoxide(config: &Arc<crate::config::Config>) -> Result<BrowserHandle> {
    use futures::StreamExt;

    let (browser, mut handler) =
        chromiumoxide::Browser::launch(
            chromiumoxide::browser::BrowserConfig::builder()
                .args([
                    "--no-first-run",
                    "--no-default-browser-check",
                    "--disable-extensions",
                    "--disable-sync",
                    "--disable-default-apps",
                ])
                .build()
                .map_err(|e| anyhow::anyhow!(e))?,
        )
        .await
        .map_err(|e| anyhow::anyhow!("chromiumoxide launch failed: {e}"))?;

    // Drive the CDP event loop; ignore non-fatal parse errors
    tokio::spawn(async move {
        loop {
            match handler.next().await {
                Some(Ok(_))  => {}
                Some(Err(e)) => tracing::debug!("chromiumoxide event (non-fatal): {e}"),
                None         => break,
            }
        }
    });

    Ok(BrowserHandle::Chromiumoxide(Arc::new(browser), Arc::clone(config)))
}
