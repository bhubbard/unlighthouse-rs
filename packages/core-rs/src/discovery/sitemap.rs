use anyhow::Result;
use futures::future::BoxFuture;
use futures::FutureExt;
use quick_xml::events::Event;
use quick_xml::Reader;
use tracing::{debug, warn};
use futures::stream::{self, StreamExt};

use crate::util::is_same_origin;

/// Fetch a sitemap (XML or TXT) and return all URL strings it contains.
/// Handles sitemapindex by recursively fetching child sitemaps (via Box::pin).
pub fn fetch_sitemap_urls<'a>(
    client: &'a reqwest::Client,
    sitemap_url: &'a str,
    site: &'a str,
    depth: usize,
) -> BoxFuture<'a, Result<Vec<String>>> {
    async move {
        if depth > 5 {
            warn!("Max sitemap recursion depth reached at {sitemap_url}");
            return Ok(vec![]);
        }

        debug!("Fetching sitemap: {sitemap_url}");
        let resp = match client
            .get(sitemap_url)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!("Sitemap request failed for {sitemap_url}: {e}");
                return Ok(vec![]);
            }
        };

        if !resp.status().is_success() {
            warn!("Sitemap fetch returned {}: {sitemap_url}", resp.status());
            return Ok(vec![]);
        }

        let body = match resp.text().await {
            Ok(b) => b,
            Err(e) => {
                warn!("Sitemap body read error for {sitemap_url}: {e}");
                return Ok(vec![]);
            }
        };

        // TXT sitemap: one URL per line
        if sitemap_url.ends_with(".txt") {
            let urls: Vec<String> = body
                .lines()
                .map(str::trim)
                .filter(|l| l.starts_with("http") || l.starts_with('/'))
                .map(String::from)
                .collect();
            debug!("TXT sitemap {sitemap_url}: {} URLs", urls.len());
            return Ok(urls);
        }

        // XML sitemap
        parse_sitemap_xml(client, &body, site, depth).await
    }
    .boxed()
}

async fn parse_sitemap_xml(
    client: &reqwest::Client,
    xml: &str,
    site: &str,
    depth: usize,
) -> Result<Vec<String>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut urls: Vec<String> = Vec::new();
    let mut sub_sitemaps: Vec<String> = Vec::new();

    let mut in_url_loc = false;
    let mut in_sitemap_loc = false;
    let mut in_sitemap_index = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                match e.name().as_ref() {
                    b"sitemapindex" => in_sitemap_index = true,
                    b"loc" => {
                        if in_sitemap_index {
                            in_sitemap_loc = true;
                        } else {
                            in_url_loc = true;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                let text = e.unescape().unwrap_or_default().trim().to_string();
                if in_url_loc && !text.is_empty() {
                    urls.push(text);
                    in_url_loc = false;
                } else if in_sitemap_loc && !text.is_empty() {
                    sub_sitemaps.push(text);
                    in_sitemap_loc = false;
                }
            }
            Ok(Event::End(ref e)) => {
                match e.name().as_ref() {
                    b"loc" => {
                        in_url_loc = false;
                        in_sitemap_loc = false;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                warn!("XML parse error: {e}");
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    // Recursively fetch sub-sitemaps (Box::pin handles the recursive async call)
    for sub_url in sub_sitemaps {
        match fetch_sitemap_urls(client, &sub_url, site, depth + 1).await {
            Ok(sub_urls) => urls.extend(sub_urls),
            Err(e) => warn!("Failed to fetch sub-sitemap {sub_url}: {e}"),
        }
    }

    debug!("XML sitemap yielded {} URLs", urls.len());
    Ok(urls)
}

/// High-level: fetch all sitemaps for a site and return same-origin URLs.
/// Returns `(filtered_urls, ignored_count)`.
pub async fn extract_sitemap_routes(
    client: &reqwest::Client,
    site: &str,
    sitemaps: &[String],
) -> Result<(Vec<String>, usize)> {
    let effective_sitemaps: Vec<String> = if sitemaps.is_empty() {
        vec![format!("{}/sitemap.xml", site.trim_end_matches('/'))]
    } else {
        sitemaps
            .iter()
            .map(|s| {
                if s.starts_with("http") {
                    s.clone()
                } else {
                    format!("{}/{}", site.trim_end_matches('/'), s.trim_start_matches('/'))
                }
            })
            .collect()
    };

    let mut all_urls: Vec<String> = Vec::new();
    let mut stream = stream::iter(effective_sitemaps)
        .map(|sm_url| {
            let client = client.clone();
            let site = site.to_string();
            async move {
                fetch_sitemap_urls(&client, &sm_url, &site, 0).await
            }
        })
        .buffer_unordered(5);

    while let Some(res) = stream.next().await {
        match res {
            Ok(urls) => all_urls.extend(urls),
            Err(e) => warn!("Sitemap error: {e}"),
        }
    }

    let total = all_urls.len();
    let filtered: Vec<String> = all_urls
        .into_iter()
        .filter(|u| is_same_origin(site, u))
        .collect();
    let ignored = total - filtered.len();

    Ok((filtered, ignored))
}
