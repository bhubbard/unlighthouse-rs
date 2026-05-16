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
        .header("User-Agent", "Mozilla/5.0 (compatible; unlighthouse-rs/0.1)")
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
    Reqwest(Arc<reqwest::Client>),
    /// Sync CDP client (stable)
    HeadlessChrome(Arc<headless_chrome::Browser>),
    /// Async CDP client (may crash on some Chrome builds)
    Chromiumoxide(Arc<chromiumoxide::Browser>),
}

impl BrowserHandle {
    pub async fn inspect_html(&self, url: &str) -> Result<(SeoData, Vec<String>)> {
        match self {
            BrowserHandle::Reqwest(c)        => inspect_reqwest(c, url).await,
            BrowserHandle::HeadlessChrome(b) => inspect_headless(b, url).await,
            BrowserHandle::Chromiumoxide(b)  => inspect_chromiumoxide(b, url).await,
        }
    }
}

// ── headless_chrome backend ───────────────────────────────────────────────────

async fn inspect_headless(
    browser: &Arc<headless_chrome::Browser>,
    url: &str,
) -> Result<(SeoData, Vec<String>)> {
    let browser = Arc::clone(browser);
    let url = url.to_string();

    tokio::task::spawn_blocking(move || -> Result<(SeoData, Vec<String>)> {
        let tab = browser.new_tab()?;
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
    url: &str,
) -> Result<(SeoData, Vec<String>)> {
    let page = browser.new_page(url).await?;
    page.wait_for_navigation().await?;

    let json_str: String = page.evaluate(INSPECT_SCRIPT).await?.into_value()?;
    page.close().await.ok();

    parse_cdp_result(&json_str, url)
}

// ── Backend launch helpers ────────────────────────────────────────────────────

/// Create a reqwest-based handle (default — no Chrome needed for HTML inspection).
pub fn launch_reqwest() -> Result<BrowserHandle> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?;
    Ok(BrowserHandle::Reqwest(Arc::new(client)))
}

/// Launch a `headless_chrome` browser and return a `BrowserHandle`.
pub fn launch_headless_chrome() -> Result<BrowserHandle> {
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

    Ok(BrowserHandle::HeadlessChrome(Arc::new(browser)))
}

/// Launch a `chromiumoxide` browser and return a `BrowserHandle`.
/// **Note:** chromiumoxide may crash on `chrome-untrusted://` CDP events
/// emitted by some Chrome versions during startup. Use headless_chrome
/// if you hit `WS Connection error: Serde(...)` on startup.
pub async fn launch_chromiumoxide() -> Result<BrowserHandle> {
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

    Ok(BrowserHandle::Chromiumoxide(Arc::new(browser)))
}
