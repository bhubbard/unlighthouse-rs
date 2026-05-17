use url::Url;

/// Generate a short, stable ID for a URL by taking the first 8 hex chars of its MD5 hash.
pub fn url_to_id(url: &str) -> String {
    let digest = md5::compute(url.as_bytes());
    format!("{:x}", digest)[..8].to_string()
}

/// Return the path component of a URL, defaulting to "/" if none.
pub fn url_to_path(url: &str) -> String {
    Url::parse(url)
        .map(|u| {
            let path = u.path().to_string();
            if path.is_empty() { "/".to_string() } else { path }
        })
        .unwrap_or_else(|_| url.to_string())
}

/// Return the origin (scheme + host + port) of a URL.
pub fn url_origin(url: &str) -> Option<String> {
    Url::parse(url).ok().map(|u| {
        let mut origin = format!("{}://{}", u.scheme(), u.host_str().unwrap_or(""));
        if let Some(port) = u.port() {
            origin.push_str(&format!(":{port}"));
        }
        origin
    })
}

/// True if `candidate` belongs to the same origin as `site`.
pub fn is_same_origin(site: &str, candidate: &str) -> bool {
    match (url_origin(site), url_origin(candidate)) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(&b),
        _ => false,
    }
}

/// Resolve a potentially-relative URL against the site base.
pub fn resolve_url(site: &str, href: &str) -> Option<String> {
    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_string());
    }
    let base = Url::parse(site).ok()?;
    base.join(href).ok().map(|u| u.to_string())
}

/// Return true if the path looks like an HTML page (not a static asset).
pub fn is_html_path(path: &str) -> bool {
    // Strip query string for the extension check
    let path_only = path.split('?').next().unwrap_or(path);
    let known_non_html = [
        ".js", ".css", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".ico",
        ".woff", ".woff2", ".ttf", ".eot", ".pdf", ".zip", ".tar", ".gz",
        ".mp4", ".mp3", ".webp", ".avif", ".json", ".xml", ".txt", ".csv",
    ];
    if let Some(dot_pos) = path_only.rfind('.') {
        let ext = &path_only[dot_pos..].to_lowercase();
        !known_non_html.contains(&ext.as_str())
    } else {
        // No extension → treat as HTML
        true
    }
}

/// Format bytes as a human-readable string.
#[allow(dead_code)]
pub fn format_bytes(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = KB * 1024;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_to_id_deterministic() {
        let id1 = url_to_id("https://example.com/");
        let id2 = url_to_id("https://example.com/");
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 8);
    }

    #[test]
    fn test_is_same_origin() {
        assert!(is_same_origin("https://example.com", "https://example.com/about"));
        assert!(!is_same_origin("https://example.com", "https://other.com/"));
    }

    #[test]
    fn test_is_html_path() {
        assert!(is_html_path("/about"));
        assert!(is_html_path("/about/"));
        assert!(is_html_path("/about?foo=bar"));
        assert!(!is_html_path("/style.css"));
        assert!(!is_html_path("/logo.png?v=123"));
        assert!(!is_html_path("/api/data.json"));
    }

    #[test]
    fn test_resolve_url() {
        let site = "https://example.com/base/";
        assert_eq!(resolve_url(site, "/abs"), Some("https://example.com/abs".to_string()));
        assert_eq!(resolve_url(site, "rel"), Some("https://example.com/base/rel".to_string()));
        assert_eq!(resolve_url(site, "https://other.com"), Some("https://other.com".to_string()));
    }

    #[test]
    fn test_url_to_path() {
        assert_eq!(url_to_path("https://example.com/foo/bar"), "/foo/bar");
        assert_eq!(url_to_path("https://example.com/"), "/");
        assert_eq!(url_to_path("https://example.com"), "/");
    }
}
