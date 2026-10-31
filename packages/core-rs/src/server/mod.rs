pub mod api;
pub mod mcp;
pub mod websocket;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use tokio::sync::{broadcast, RwLock, Semaphore};
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::types::{NormalisedRoute, RouteReport};

use self::websocket::{ws_handler, WsBroadcast};

#[derive(rust_embed::RustEmbed)]
#[folder = "../client/dist/"]
struct Assets;

/// Central shared state for the Axum server.
pub struct AppState {
    pub route_reports: RwLock<HashMap<String, RouteReport>>,
    pub ws_tx: WsBroadcast,
    pub config: Arc<Config>,
    pub lighthouse_semaphore: Arc<Semaphore>,
    pub discovery_semaphore: Arc<Semaphore>,
    pub lighthouse_pool: Option<Arc<crate::queue::pool::LighthousePool>>,
    pub work_tx: tokio::sync::mpsc::Sender<NormalisedRoute>,
    /// Shared HTTP client for proxying (CrUX)
    pub http_client: reqwest::Client,
    /// SQLite connection pool — stores run history for score trending.
    pub db: sqlx::SqlitePool,
    /// Unique ID for the current scan run (hex string, md5-derived).
    pub run_id: String,
    /// Monotonic start time of the scan.
    pub start_time: std::time::Instant,
    /// Robots.txt disallow rules collected during initial discovery, enforced on crawled links.
    pub robots_rules: RwLock<Vec<crate::discovery::robots_txt::RobotsRule>>,
}

impl AppState {
    pub fn new(
        config: Arc<Config>,
        work_tx: tokio::sync::mpsc::Sender<NormalisedRoute>,
        lighthouse_pool: Option<Arc<crate::queue::pool::LighthousePool>>,
        db: sqlx::SqlitePool,
        run_id: String,
    ) -> Self {
        let (ws_tx, _) = broadcast::channel(1024);
        let workers = config.workers;
        Self {
            route_reports: RwLock::new(HashMap::new()),
            ws_tx,
            config,
            lighthouse_semaphore: Arc::new(Semaphore::new(workers)),
            discovery_semaphore: Arc::new(Semaphore::new(50)), // High concurrency for discovery
            lighthouse_pool,
            work_tx,
            http_client: reqwest::Client::new(),
            db,
            run_id,
            start_time: std::time::Instant::now(),
            robots_rules: RwLock::new(Vec::new()),
        }
    }
}

// ── index.html payload injection ──────────────────────────────────────────────

/// Default Lighthouse column definitions (mirrors packages/core/src/constants.ts).
fn default_columns_json() -> serde_json::Value {
    serde_json::json!({
        "overview": [
            { "label": "Screenshot Timeline", "key": "report.audits.screenshot-thumbnails", "cols": 6 }
        ],
        "performance": [
            { "cols": 1, "label": "FCP", "key": "report.audits.first-contentful-paint", "sortKey": "numericValue" },
            { "cols": 2, "label": "LCP", "key": "report.audits.largest-contentful-paint", "sortKey": "numericValue" },
            { "cols": 2, "label": "CLS", "key": "report.audits.cumulative-layout-shift", "sortKey": "numericValue" },
            { "cols": 1, "label": "TBT", "key": "report.audits.total-blocking-time", "sortKey": "numericValue" },
            { "cols": 1, "label": "SI",  "key": "report.audits.speed-index", "sortKey": "numericValue" }
        ],
        "accessibility": [
            { "cols": 3, "label": "Color Contrast",  "key": "report.audits.color-contrast",   "sortKey": "length:details.items" },
            { "cols": 1, "label": "Headings",         "key": "report.audits.heading-order",     "sortKey": "length:details.items" },
            { "cols": 1, "label": "Labels",           "key": "report.audits.label",             "sortKey": "length:details.items" },
            { "cols": 1, "label": "Image Alts",       "key": "report.audits.image-alt",         "sortKey": "length:details.items" },
            { "cols": 1, "label": "Link Names",       "key": "report.audits.link-name",         "sortKey": "length:details.items" }
        ],
        "best-practices": [
            { "cols": 2, "label": "Errors",            "key": "report.audits.errors-in-console",     "sortKey": "length:details.items" },
            { "cols": 2, "label": "Inspector Issues",  "key": "report.audits.inspector-issues",      "sortKey": "length:details.items" },
            { "cols": 2, "label": "Images Responsive", "key": "report.audits.image-size-responsive", "sortKey": "length:details.items" },
            { "cols": 2, "label": "Image Aspect Ratio","key": "report.audits.image-aspect-ratio",    "sortKey": "length:details.items" }
        ],
        "seo": [
            { "cols": 1, "label": "Indexable",      "key": "report.audits.is-crawlable" },
            { "cols": 1, "label": "Internal links", "key": "seo.internalLinks",  "sortable": true },
            { "cols": 1, "label": "External links", "key": "seo.externalLinks",  "sortable": true },
            { "cols": 2, "label": "Description",    "key": "seo.description" },
            { "cols": 2, "label": "Share Image",    "key": "seo.og.image" }
        ]
    })
}

/// Build the `window.__unlighthouse_payload` JSON from the current config.
fn build_payload(config: &Config, _addr: SocketAddr) -> serde_json::Value {
    // Prefer the bound addr (which has the real IP) for display, but use the
    // configured host for URLs so "localhost" stays human-readable.
    let host = &config.host;
    let port = config.port;
    let api_url   = format!("http://{host}:{port}/api");
    let ws_url    = format!("ws://{host}:{port}/api/ws");
    let router_prefix = if config.router_prefix.is_empty() {
        "/".to_string()
    } else {
        config.router_prefix.clone()
    };

    let device_str = match config.scanner.device {
        crate::config::Device::Desktop => "desktop",
        _ => "mobile",
    };

    serde_json::json!({
        "appName": "UnLighthouse-RS",
        "version": "0.0.1",
        "options": {
            "site": config.site,
            "websocketUrl": ws_url,
            "apiUrl": api_url,
            "routerPrefix": router_prefix,
            "lighthouseOptions": {
                "onlyCategories": ["performance", "accessibility", "best-practices", "seo"]
            },
            "scanner": {
                "dynamicSampling": config.scanner.dynamic_sampling.unwrap_or(5),
                "throttle": config.scanner.throttle,
                "device": device_str
            },
            "client": {
                "groupRoutesKey": "route.definition.name",
                "columns": default_columns_json()
            }
        }
    })
}

/// `GET /` and `GET /index.html` — serve index.html with the payload injected.
async fn index_handler(State(state): State<Arc<AppState>>) -> Response {
    let client_dir = PathBuf::from(&state.config.output_path).join("client");
    let index_path = client_dir.join("index.html");

    let html = match tokio::fs::read_to_string(&index_path).await {
        Ok(h) => h,
        Err(_) => {
            // Fallback to embedded asset
            match Assets::get("index.html") {
                Some(embedded) => String::from_utf8_lossy(&embedded.data).to_string(),
                None => {
                    let msg = format!(
                        "index.html not found at {index_path:?}.\n\
                         Build the client first:\n\
                           cd packages/client && pnpm build\n\
                         Then copy the output:\n\
                           mkdir -p {dir}/client && cp -r packages/client/dist/ {dir}/client/",
                        dir = state.config.output_path
                    );
                    return Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                        .body(Body::from(msg))
                        .unwrap();
                }
            }
        }
    };

    // Build payload — use a placeholder SocketAddr since we only need config values here.
    let addr: SocketAddr = format!("{}:{}", state.config.host, state.config.port)
        .parse()
        .unwrap_or_else(|_| "127.0.0.1:5678".parse().unwrap());
    let mut payload = build_payload(&state.config, addr);
    let reports: Vec<RouteReport> = {
        let r = state.route_reports.read().await;
        r.values().cloned().collect()
    };
    let scan_meta = api::compute_scan_meta(&state).await;
    payload["reports"] = serde_json::to_value(&reports).unwrap_or_else(|_| serde_json::json!([]));
    payload["scanMeta"] = serde_json::to_value(&scan_meta).unwrap_or(serde_json::Value::Null);

    let payload_json = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());

    let script = format!(
        "<script>\nwindow.__unlighthouse_payload = {payload_json};\nwindow.__unlighthouse_static = false;\n</script>"
    );

    // Inject before </head> (or prepend to <body> as a fallback)
    let injected = if html.contains("</head>") {
        html.replacen("</head>", &format!("{script}\n</head>"), 1)
    } else {
        html.replacen("<body>", &format!("<body>\n{script}"), 1)
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::STRICT_TRANSPORT_SECURITY, "max-age=31536000; includeSubDomains; preload")
        .header("Cross-Origin-Opener-Policy", "same-origin")
        .header("X-Frame-Options", "SAMEORIGIN")
        .header("X-Content-Type-Options", "nosniff")
        .header(header::CONTENT_SECURITY_POLICY, "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self' ws: wss: http://localhost:* http://127.0.0.1:* ws://localhost:* ws://127.0.0.1:* https://api.iconify.design; object-src 'none'; frame-ancestors 'self';")
        .body(Body::from(injected))
        .unwrap()
}

/// `GET /favicon.ico` — serve the logo as a favicon.
async fn favicon_handler(State(state): State<Arc<AppState>>) -> Response {
    let logo_path = PathBuf::from(&state.config.output_path)
        .join("client")
        .join("assets")
        .join("logo.svg");

    match tokio::fs::read(&logo_path).await {
        Ok(data) => {
            debug!("Served favicon from {:?}", logo_path);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "image/svg+xml")
                .body(Body::from(data))
                .unwrap()
        },
        Err(e) => {
            debug!("Failed to read favicon at {:?}: {}, falling back to embedded assets/logo.svg", logo_path, e);
            match Assets::get("assets/logo.svg") {
                Some(content) => {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "image/svg+xml")
                        .body(Body::from(content.data))
                        .unwrap()
                }
                None => {
                    warn!("Embedded assets/logo.svg not found for favicon fallback");
                    Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::empty())
                        .unwrap()
                }
            }
        },
    }
}

async fn add_security_headers(req: axum::extract::Request, next: axum::middleware::Next) -> impl IntoResponse {
    let path = req.uri().path().to_string();
    let mut response = next.run(req).await;
    
    // Add security headers (already doing this)
    let headers = response.headers_mut();
    headers.insert(
        header::STRICT_TRANSPORT_SECURITY,
        header::HeaderValue::from_static("max-age=31536000; includeSubDomains; preload"),
    );
    headers.insert(
        header::HeaderName::from_static("cross-origin-opener-policy"),
        header::HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::HeaderName::from_static("x-frame-options"),
        header::HeaderValue::from_static("SAMEORIGIN"),
    );
    headers.insert(
        header::HeaderName::from_static("x-content-type-options"),
        header::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        header::HeaderValue::from_static("default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self' ws: wss: http://localhost:* http://127.0.0.1:* ws://localhost:* ws://127.0.0.1:* https://api.iconify.design; object-src 'none'; frame-ancestors 'self';"),
    );

    // Add Cache-Control for static assets
    // We check the path of the ORIGINAL request
    if path.starts_with("/assets/") || path.starts_with("/fonts/") {
        // Hashed assets and fonts can be cached forever
        headers.insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    } else if path.ends_with(".js") || path.ends_with(".css") || path.ends_with(".svg") || path.ends_with(".ico") || path.ends_with(".woff2") || path == "/" || path == "/index.html" {
        // Short cache for entry points and other static assets
        headers.insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("public, max-age=3600"),
        );
    }
    
    let status = response.status();
    if status.is_client_error() || status.is_server_error() {
        warn!("{} -> {}", path, status);
    } else {
        debug!("{} -> {}", path, status);
    }

    response
}

// ── Server startup ────────────────────────────────────────────────────────────

/// Build and start the Axum HTTP server.
pub async fn start_server(
    state: Arc<AppState>,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    let output_path = PathBuf::from(&state.config.output_path);
    let client_dir = output_path.join("client");
    
    info!("Starting server with output_path: {:?}", output_path.canonicalize().unwrap_or(output_path.clone()));
    info!("Client directory: {:?}", client_dir.canonicalize().unwrap_or(client_dir.clone()));
    let router_prefix = state.config.router_prefix.clone();

    // API routes
    let api_router = Router::new()
        .route("/api/reports", get(api::get_reports))
        .route("/api/scan-meta", get(api::get_scan_meta))
        .route("/api/reports/rescan", post(api::rescan_all))
        .route("/api/reports/:id/rescan", post(api::rescan_one))
        .route("/api/crux/*site", get(api::get_crux_history))
        // ── Historical score trending ────────────────────────────────────────
        .route("/api/runs", get(api::list_runs))
        .route("/api/runs/:run_id/scores", get(api::get_run_scores))
        .route("/api/history/route", get(api::get_route_history))
        // ────────────────────────────────────────────────────────────────────
        .route("/api/ws", get(ws_handler))
        .with_state(state.clone());

    // Serve index.html with injected payload at root and /index.html
    let app_state = state.clone();
    let index_router = Router::new()
        .route("/", get(index_handler))
        .route("/index.html", get(index_handler))
        .route("/favicon.ico", get(favicon_handler))
        .with_state(app_state);

    // Serve the entire output dir as a fallback so that:
    //   /assets/...              → .unlighthouse/client/assets/... (via explicit route below)
    //   /{hostname}/{route}/lighthouse.html  → .unlighthouse/{hostname}/{route}/lighthouse.html
    //   /{hostname}/{route}/screenshot.jpeg  → .unlighthouse/{hostname}/{route}/screenshot.jpeg
    let output_path = PathBuf::from(&state.config.output_path);
    let artifact_service = ServeDir::new(&output_path);

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let asset_service = axum::routing::get(|uri: axum::http::Uri| async move {
        let path = uri.path().trim_start_matches('/');
        match Assets::get(path) {
            Some(content) => {
                let mime = mime_guess::from_path(path).first_or_octet_stream();
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, mime.as_ref())
                    .body(Body::from(content.data))
                    .unwrap()
            }
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())
                .unwrap(),
        }
    });

    let app = Router::new()
        .merge(api_router)
        .merge(index_router)
        .fallback_service(
            ServeDir::new(&client_dir)
                .fallback(artifact_service.fallback(asset_service))
        )
        .layer(cors)
        .layer(CompressionLayer::new())
        .layer(axum::middleware::from_fn(add_security_headers));

    // Optionally nest under router_prefix
    let final_app = if router_prefix.is_empty() {
        app
    } else {
        Router::new().nest(&router_prefix, app)
    };

    let addr = listener.local_addr()?;
    info!("Server listening on http://{addr}");
    axum::serve(listener, final_app).await?;
    Ok(())
}

/// Export a self-contained static dashboard into `{output_path}/client/` with
/// `window.__unlighthouse_static = true` and `window.__unlighthouse_payload`
/// pre-populated with all route reports and scan metadata.
pub async fn write_static_client(state: &Arc<AppState>) -> anyhow::Result<PathBuf> {
    let client_dir = PathBuf::from(&state.config.output_path).join("client");
    tokio::fs::create_dir_all(&client_dir).await?;

    // Extract any embedded client assets that are not already present on disk
    for asset_path in Assets::iter() {
        let rel = asset_path.as_ref();
        let dest = client_dir.join(rel);
        if !dest.exists() {
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            if let Some(file) = Assets::get(rel) {
                tokio::fs::write(&dest, file.data.as_ref()).await?;
            }
        }
    }

    let index_path = client_dir.join("index.html");
    let base_html = match tokio::fs::read_to_string(&index_path).await {
        Ok(h) => h,
        Err(_) => match Assets::get("index.html") {
            Some(embedded) => String::from_utf8_lossy(&embedded.data).to_string(),
            None => "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>UnLighthouse-RS</title></head><body><div id=\"app\"></div></body></html>".to_string(),
        },
    };

    // Strip any previously injected __unlighthouse_payload script block before re-injecting
    let addr: SocketAddr = format!("{}:{}", state.config.host, state.config.port)
        .parse()
        .unwrap_or_else(|_| "127.0.0.1:5678".parse().unwrap());
    let mut payload = build_payload(&state.config, addr);
    let mut reports: Vec<RouteReport> = {
        let r = state.route_reports.read().await;
        r.values().cloned().collect()
    };
    reports.sort_by(|a, b| a.route.path.cmp(&b.route.path));
    let scan_meta = api::compute_scan_meta(state).await;
    payload["reports"] = serde_json::to_value(&reports)?;
    payload["scanMeta"] = serde_json::to_value(&scan_meta)?;

    let payload_json = serde_json::to_string(&payload)?;
    let script = format!(
        "<script>\nwindow.__unlighthouse_payload = {payload_json};\nwindow.__unlighthouse_static = true;\n</script>"
    );

    let injected = if base_html.contains("</head>") {
        base_html.replacen("</head>", &format!("{script}\n</head>"), 1)
    } else {
        format!("{script}\n{base_html}")
    };

    tokio::fs::write(&index_path, injected).await?;
    Ok(index_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use tower::ServiceExt;

    #[test]
    fn test_default_columns_and_build_payload() {
        let mut config = Config {
            site: "https://example.com".to_string(),
            host: "127.0.0.1".to_string(),
            port: 7000,
            ..Default::default()
        };
        config.scanner.device = crate::config::Device::Desktop;

        let addr: SocketAddr = "127.0.0.1:7000".parse().unwrap();
        let payload = build_payload(&config, addr);

        assert_eq!(payload["appName"], "UnLighthouse-RS");
        assert_eq!(payload["options"]["site"], "https://example.com");
        assert_eq!(payload["options"]["apiUrl"], "http://127.0.0.1:7000/api");
        assert_eq!(payload["options"]["websocketUrl"], "ws://127.0.0.1:7000/api/ws");
        assert_eq!(payload["options"]["routerPrefix"], "/");
        assert_eq!(payload["options"]["scanner"]["device"], "desktop");
        assert!(payload["options"]["client"]["columns"]["performance"].is_array());
    }

    #[tokio::test]
    async fn test_index_favicon_and_security_headers() {
        let dir = tempfile::tempdir().unwrap();
        let client_dir = dir.path().join("client");
        let assets_dir = client_dir.join("assets");
        std::fs::create_dir_all(&assets_dir).unwrap();
        std::fs::write(
            client_dir.join("index.html"),
            "<html><head><title>App</title></head><body>Hello</body></html>",
        )
        .unwrap();
        std::fs::write(assets_dir.join("logo.svg"), "<svg></svg>").unwrap();

        let db_path = dir.path().join("srv.db");
        let db = crate::db::open(db_path.to_str().unwrap()).await.unwrap();

        let config = Arc::new(Config {
            site: "https://example.com".to_string(),
            output_path: dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        });
        let (work_tx, _work_rx) = tokio::sync::mpsc::channel(8);
        let state = Arc::new(AppState::new(
            config,
            work_tx,
            None,
            db,
            "run-srv".to_string(),
        ));

        let app = Router::new()
            .route("/", get(index_handler))
            .route("/favicon.ico", get(favicon_handler))
            .route("/assets/bundle.js", get(|| async { "console.log(1);" }))
            .with_state(state)
            .layer(axum::middleware::from_fn(add_security_headers));

        // 1. GET / injects window.__unlighthouse_payload and sets security headers
        let resp = app
            .clone()
            .oneshot(axum::http::Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("x-frame-options").unwrap(),
            "SAMEORIGIN"
        );
        assert_eq!(
            resp.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        assert_eq!(
            resp.headers().get(header::CACHE_CONTROL).unwrap(),
            "public, max-age=3600"
        );
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("window.__unlighthouse_payload ="));
        assert!(html.contains("https://example.com"));

        // 2. GET /favicon.ico serves SVG with max-age=3600
        let fav = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/favicon.ico")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(fav.status(), StatusCode::OK);
        assert_eq!(
            fav.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/svg+xml"
        );

        // 3. GET /assets/bundle.js sets immutable Cache-Control
        let asset_resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/assets/bundle.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            asset_resp.headers().get(header::CACHE_CONTROL).unwrap(),
            "public, max-age=31536000, immutable"
        );
    }

    #[tokio::test]
    async fn test_write_static_client_and_payload_reports() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("static.db");
        let db = crate::db::open(db_path.to_str().unwrap()).await.unwrap();

        let config = Arc::new(Config {
            site: "https://static.example.com".to_string(),
            output_path: dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        });
        let (work_tx, _work_rx) = tokio::sync::mpsc::channel(8);
        let state = Arc::new(AppState::new(
            Arc::clone(&config),
            work_tx,
            None,
            db,
            "run-static".to_string(),
        ));

        let route = crate::discovery::routes::normalise_route("/about", &config.site);
        let mut rep = crate::queue::worker::create_route_report(&route, &config);
        rep.tasks.inspect_html_task = crate::types::TaskStatus::Completed;
        rep.tasks.run_lighthouse_task = crate::types::TaskStatus::Completed;
        state.route_reports.write().await.insert(route.id.clone(), rep);

        let index_path = write_static_client(&state).await.unwrap();
        assert!(index_path.exists());
        let html = tokio::fs::read_to_string(&index_path).await.unwrap();
        assert!(html.contains("window.__unlighthouse_static = true;"));
        assert!(html.contains("\"https://static.example.com/about\""));
        assert!(html.contains("\"scanMeta\""));
    }
}
