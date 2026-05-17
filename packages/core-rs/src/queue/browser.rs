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

    let base_url = response.url().clone();
    let html = response.text().await?;
    let html_size = html.len();

    let document = scraper::Html::parse_document(&html);

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

    return JSON.stringify({
        title:         document.title || null,
        description:   document.querySelector('meta[name="description"]')
                           ?.getAttribute('content') || null,
        internalLinks: internalHrefs.length,
        externalLinks: externalCount,
        htmlSize:      document.documentElement.outerHTML.length,
        hrefs:         internalHrefs
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
}

fn parse_cdp_result(json_str: &str, url: &str) -> Result<(SeoData, Vec<String>)> {
    let r: InspectResult = serde_json::from_str(json_str)?;
    debug!(
        url,
        title     = ?r.title,
        internal  = r.internal_links,
        external  = r.external_links,
        html_size = r.html_size,
        "HTML inspection complete (CDP)"
    );
    Ok((
        SeoData {
            title:          r.title.filter(|t| !t.is_empty()),
            description:    r.description.filter(|d| !d.is_empty()),
            internal_links: Some(r.internal_links),
            external_links: Some(r.external_links),
            html_size:      Some(r.html_size),
        },
        r.hrefs,
    ))
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
            BrowserHandle::Reqwest(c, _)        => inspect_reqwest(c, url).await,
            BrowserHandle::HeadlessChrome(b, cfg) => inspect_headless(b, cfg, url).await,
            BrowserHandle::Chromiumoxide(b, cfg)  => inspect_chromiumoxide(b, cfg, url).await,
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
