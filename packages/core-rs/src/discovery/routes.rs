use anyhow::Result;
use std::collections::{HashMap, HashSet};
use tracing::{info, warn};

use crate::config::Config;
use crate::discovery::robots_txt::{
    collect_disallow_rules, match_path_to_rule, parse_robots_txt, RobotsRule,
};
use crate::discovery::sitemap::extract_sitemap_routes;
use crate::types::{NormalisedRoute, RouteDefinition};
use crate::util::{is_html_path, is_same_origin, resolve_url, url_origin, url_to_id, url_to_path};

/// Normalise a URL into a NormalisedRoute.
pub fn normalise_route(url: &str, site: &str) -> NormalisedRoute {
    let id = url_to_id(url);
    let path = url_to_path(url);
    let definition = RouteDefinition {
        name: path.clone(),
        path: path.clone(),
    };
    // Make the URL absolute if it isn't already
    let abs_url = if url.starts_with("http") {
        url.to_string()
    } else {
        format!("{}{}", site.trim_end_matches('/'), url)
    };
    NormalisedRoute {
        id,
        path,
        url: abs_url,
        definition,
    }
}


/// Apply include/exclude filters to a path.
pub fn passes_filters(path: &str, include: &[String], exclude: &[String]) -> bool {
    // If include list is set, path must match at least one
    if !include.is_empty() {
        let matches_include = include.iter().any(|pat| path_matches(path, pat));
        if !matches_include {
            return false;
        }
    }
    // If exclude list is set, path must not match any
    if !exclude.is_empty() {
        let matches_exclude = exclude.iter().any(|pat| path_matches(path, pat));
        if matches_exclude {
            return false;
        }
    }
    true
}

/// Returns true if `path` matches the include/exclude `pattern`.
/// Regex patterns (surrounded by slashes /re/ or starting with ^) are matched using the `regex` crate.
/// Glob wildcards (`*`, `?`) are supported via the `glob` crate.
/// Plain patterns are matched as a prefix.
fn path_matches(path: &str, pattern: &str) -> bool {
    if (pattern.starts_with('/') && pattern.ends_with('/') && pattern.len() > 2) || pattern.starts_with('^') {
        let re_str = if pattern.starts_with('/') && pattern.ends_with('/') {
            &pattern[1..pattern.len() - 1]
        } else {
            pattern
        };
        if let Ok(re) = regex::Regex::new(re_str) {
            return re.is_match(path);
        }
    }
    if pattern.contains('*') || pattern.contains('?') {
        glob_match(pattern, path)
    } else {
        path.starts_with(pattern) || path == pattern
    }
}

/// Discover all routes to scan given the config.
/// Returns normalised routes, a reqwest client (for the caller to reuse for crawling),
/// and optional robots disallow rules (for filtering in the worker queue).
pub async fn resolve_reportable_routes(
    config: &Config,
) -> Result<(Vec<NormalisedRoute>, reqwest::Client, Vec<RobotsRule>)> {
    let ua = config
        .user_agent
        .as_deref()
        .unwrap_or("Mozilla/5.0 (compatible; unlighthouse-rs/0.1)");

    let mut default_headers = reqwest::header::HeaderMap::new();
    if let Some(ref extra_headers) = config.extra_headers {
        for (k, v) in extra_headers {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(v),
            ) {
                default_headers.insert(name, val);
            }
        }
    }
    if let Some(ref cookies) = config.cookies {
        if !cookies.is_empty() {
            let cookie_str = cookies
                .iter()
                .map(|c| format!("{}={}", c.name, c.value))
                .collect::<Vec<_>>()
                .join("; ");
            if let Ok(val) = reqwest::header::HeaderValue::from_str(&cookie_str) {
                default_headers.insert(reqwest::header::COOKIE, val);
            }
        }
    }
    if let Some(ref auth) = config.auth {
        use base64::Engine;
        let credentials = format!("{}:{}", auth.username, auth.password);
        let encoded = base64::engine::general_purpose::STANDARD.encode(credentials);
        if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Basic {encoded}")) {
            default_headers.insert(reqwest::header::AUTHORIZATION, val);
        }
    }

    let client = reqwest::Client::builder()
        .user_agent(ua)
        .default_headers(default_headers)
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let site_origin = url_origin(&config.site).unwrap_or_else(|| config.site.clone());
    let mut urls: HashSet<String> = HashSet::new();
    urls.insert(config.site.clone());

    let mut robots_rules: Vec<RobotsRule> = Vec::new();
    let mut extra_sitemaps: Vec<String> = Vec::new();

    // ── robots.txt ────────────────────────────────────────────────────────────
    if config.scanner.robots_txt {
        let robots_url = format!("{}/robots.txt", site_origin.trim_end_matches('/'));
        if let Ok(resp) = client.get(&robots_url).timeout(std::time::Duration::from_secs(10)).send().await {
            if resp.status().is_success() {
                if let Ok(body) = resp.text().await {
                    let parsed = parse_robots_txt(&body);
                    info!(
                        "Found /robots.txt — sitemaps: {}, groups: {}",
                        parsed.sitemaps.len(),
                        parsed.groups.len()
                    );
                    robots_rules = collect_disallow_rules(&parsed);
                    extra_sitemaps = parsed.sitemaps;
                }
            }
        }
    }

    // ── sitemap ───────────────────────────────────────────────────────────────
    let mut crawler_enabled = config.scanner.crawler;

    if config.scanner.sitemap {
        let mut sitemap_urls = extra_sitemaps.clone();
        // add default sitemap.xml if no sitemaps discovered from robots.txt
        if sitemap_urls.is_empty() {
            sitemap_urls.push(format!("{}/sitemap.xml", site_origin.trim_end_matches('/')));
        }

        match extract_sitemap_routes(&client, &site_origin, &sitemap_urls).await {
            Ok((paths, ignored)) => {
                if ignored > 0 && paths.is_empty() {
                    warn!("Sitemap exists but all URLs are from a different origin (ignored: {ignored})");
                } else if !paths.is_empty() {
                    info!("Discovered {} routes from sitemaps.", paths.len());
                    if ignored > 0 {
                        warn!("Ignoring {ignored} sitemap URLs from different origin.");
                    }
                    for u in &paths {
                        urls.insert(u.clone());
                    }
                    // Disable crawler if we have enough sitemap URLs
                    if !config.site.contains("localhost") && paths.len() >= 50 {
                        crawler_enabled = false;
                        info!("Disabling crawler: sitemap has {} URLs.", paths.len());
                    }
                } else if crawler_enabled {
                    info!("No sitemap found, falling back to crawler.");
                } else {
                    warn!("No sitemap found and crawler is disabled — no routes discovered.");
                }
            }
            Err(e) => warn!("Sitemap fetch error: {e}"),
        }
    }

    // ── Build initial route list ───────────────────────────────────────────────
    let mut routes: Vec<NormalisedRoute> = urls
        .iter()
        .filter(|u| is_same_origin(&site_origin, u))
        .filter(|u| {
            let path = url_to_path(u);
            passes_filters(&path, &config.scanner.include, &config.scanner.exclude)
        })
        .filter(|u| is_html_path(&url_to_path(u)))
        .map(|u| normalise_route(u, &site_origin))
        .collect();

    // ── robots.txt filtering ─────────────────────────────────────────────────
    if !robots_rules.is_empty() {
        routes.retain(|r| {
            match match_path_to_rule(&r.path, &robots_rules) {
                Some(rule) if !rule.allow => {
                    info!(path = %r.path, pattern = %rule.pattern, "Skipping route (robots.txt disallow)");
                    false
                }
                _ => true,
            }
        });
    }

    // ── Dynamic sampling ──────────────────────────────────────────────────────
    if let Some(sample_size) = config.scanner.dynamic_sampling {
        if sample_size > 0 {
            // Group routes by their "definition path" (in our case we use the first two segments)
            let mut groups: HashMap<String, Vec<NormalisedRoute>> = HashMap::new();
            for route in routes {
                let group_key = path_group_key(&route.path);
                groups.entry(group_key).or_default().push(route);
            }

            routes = Vec::new();
            for (_key, mut group) in groups {
                if group.len() > sample_size {
                    // Deterministic: take first N sorted by path
                    group.sort_by(|a, b| a.path.cmp(&b.path));
                    group.truncate(sample_size);
                    warn!(
                        "Dynamic sampling: keeping {}/{} routes in group",
                        sample_size,
                        group.len()
                    );
                }
                routes.extend(group);
            }
        }
    }

    // ── Max routes cap ────────────────────────────────────────────────────────
    if let Some(max) = config.scanner.max_routes {
        if routes.len() > max {
            warn!("Capping routes from {} to max_routes={max}", routes.len());
            routes.truncate(max);
        }
    }

    routes.sort_by(|a, b| a.path.cmp(&b.path));

    info!(
        "Route discovery complete: {} routes (crawler {})",
        routes.len(),
        if crawler_enabled { "enabled" } else { "disabled" }
    );

    Ok((routes, client, robots_rules))
}

/// Group key: take up to two path segments to identify a "route group".
/// e.g. /blog/post-1 and /blog/post-2 both get group key "/blog".
fn path_group_key(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match segments.len() {
        0 => "/".to_string(),
        1 => format!("/{}", segments[0]),
        _ => {
            // If the second segment looks "dynamic" (all digits or uuid-ish), use only first
            let seg = segments[1];
            if seg.chars().all(|c| c.is_ascii_digit() || c == '-') && seg.len() > 4 {
                format!("/{}", segments[0])
            } else {
                format!("/{}/{}", segments[0], segments[1])
            }
        }
    }
}

/// Crawl a single page and extract internal hrefs.
pub async fn crawl_page(
    client: &reqwest::Client,
    url: &str,
    site: &str,
) -> Result<Vec<String>> {
    use scraper::{Html, Selector};

    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        return Ok(vec![]);
    }
    let body = resp.text().await?;
    let doc = Html::parse_document(&body);
    let selector = Selector::parse("a[href]").expect("hardcoded CSS selector is valid");

    let links: Vec<String> = doc
        .select(&selector)
        .filter_map(|el| el.value().attr("href"))
        .filter_map(|href| resolve_url(site, href))
        .filter(|u| is_same_origin(site, u))
        .filter(|u| is_html_path(&url_to_path(u)))
        .collect();

    Ok(links)
}

/// Test whether `s` matches a glob `pattern` (supports `*` and `?`).
/// Delegates to the `glob` crate for correctness and maintainability.
fn glob_match(pattern: &str, s: &str) -> bool {
    glob::Pattern::new(pattern)
        .map(|p| p.matches(s))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalise_route_relative_and_absolute() {
        let r1 = normalise_route("/about", "https://example.com/");
        assert_eq!(r1.path, "/about");
        assert_eq!(r1.url, "https://example.com/about");
        assert_eq!(r1.definition.name, "/about");

        let r2 = normalise_route("https://example.com/pricing?plan=pro", "https://example.com");
        assert_eq!(r2.path, "/pricing");
        assert_eq!(r2.url, "https://example.com/pricing?plan=pro");
    }

    #[test]
    fn test_passes_filters_prefix_glob_and_regex() {
        // Plain prefix & glob
        let include = vec!["/blog".to_string(), "/products/*".to_string()];
        let exclude = vec!["/blog/draft-*".to_string(), "/^\\/products\\/secret-\\d+$/".to_string()];

        assert!(passes_filters("/blog/post-1", &include, &exclude));
        assert!(!passes_filters("/blog/draft-1", &include, &exclude));
        assert!(passes_filters("/products/shoes", &include, &exclude));
        assert!(!passes_filters("/products/secret-42", &include, &exclude));
        assert!(!passes_filters("/about", &include, &exclude));

        // Caret regex pattern
        let re_inc = vec!["^/docs/(v1|v2)/".to_string()];
        assert!(passes_filters("/docs/v1/intro", &re_inc, &[]));
        assert!(!passes_filters("/docs/v3/intro", &re_inc, &[]));
    }

    #[test]
    fn test_path_group_key_dynamic_vs_static() {
        assert_eq!(path_group_key("/"), "/");
        assert_eq!(path_group_key("/about"), "/about");
        // Second segment is numeric/dashes > 4 chars -> grouped by first segment
        assert_eq!(path_group_key("/blog/2026-10-30"), "/blog");
        assert_eq!(path_group_key("/orders/987654"), "/orders");
        // Second segment is static word -> uses first two segments
        assert_eq!(path_group_key("/docs/getting-started/install"), "/docs/getting-started");
    }

    #[cfg(feature = "native")]
    #[tokio::test]
    async fn test_crawl_page_and_resolve_reportable_routes() {
        use axum::{extract::State, routing::get, Router};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let site = format!("http://{addr}");

        let app = Router::new()
            .route(
                "/robots.txt",
                get(|State(site): State<String>| async move {
                    format!("User-agent: *\nDisallow: /admin\nSitemap: {site}/sitemap.xml\n")
                }),
            )
            .route(
                "/sitemap.xml",
                get(|State(site): State<String>| async move {
                    format!(
                        r#"<urlset>
                            <url><loc>{site}/</loc></url>
                            <url><loc>{site}/about</loc></url>
                            <url><loc>{site}/admin/panel</loc></url>
                            <url><loc>{site}/blog/2026-01</loc></url>
                            <url><loc>{site}/blog/2026-02</loc></url>
                            <url><loc>{site}/blog/2026-03</loc></url>
                            <url><loc>{site}/assets/app.js</loc></url>
                        </urlset>"#
                    )
                }),
            )
            .route(
                "/",
                get(|| async {
                    r#"<html><body>
                        <a href="/contact">Contact</a>
                        <a href="/bundle.css">CSS</a>
                        <a href="https://external.org/out">Out</a>
                    </body></html>"#
                }),
            )
            .with_state(site.clone());

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = Config {
            site: site.clone(),
            ..Default::default()
        };
        config.scanner.dynamic_sampling = Some(2);

        let (routes, client, robots_rules) = resolve_reportable_routes(&config).await.unwrap();
        assert_eq!(robots_rules.len(), 1);

        let paths: Vec<&str> = routes.iter().map(|r| r.path.as_str()).collect();
        // /admin/panel disallowed by robots.txt; /assets/app.js excluded as non-HTML;
        // /blog/* capped at 2 by dynamic_sampling=2
        assert!(paths.contains(&"/"));
        assert!(paths.contains(&"/about"));
        assert!(!paths.contains(&"/admin/panel"));
        assert!(!paths.contains(&"/assets/app.js"));
        let blog_count = paths.iter().filter(|p| p.starts_with("/blog/")).count();
        assert_eq!(blog_count, 2);

        // Also verify crawl_page extracts only internal HTML links
        let crawled = crawl_page(&client, &format!("{site}/"), &site)
            .await
            .unwrap();
        assert_eq!(crawled, vec![format!("{site}/contact")]);
    }
}
