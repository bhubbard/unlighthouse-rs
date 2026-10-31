![unlighthouse - Scan your entire website with Google Lighthouse.](https://repository-images.githubusercontent.com/423079536/c88a81ee-43ec-40fc-a615-1d29bbeaaeb4)

<h1>unlighthouse-rs</h1>

[![License][license-src]][license-href]

<p align="center">
A Rust port of the <a href="https://github.com/harlan-zw/unlighthouse">Unlighthouse</a> core —<br>
scan your entire website with Google Lighthouse, fast.
</p>

---

## Credits

This project is a Rust port of **[Unlighthouse](https://github.com/harlan-zw/unlighthouse)** by [Harlan Wilton (@harlan-zw)](https://github.com/harlan-zw). The original project is the real work here — the live demo, the Vue dashboard, the Lighthouse integration design, and the smart sampling logic all come from Harlan's project. Go sponsor him: [github.com/sponsors/harlan-zw](https://github.com/sponsors/harlan-zw).

The original Unlighthouse is available as an npm package and supports Vue, Nuxt, Next.js, and many other frameworks. If you want the full-featured, battle-tested version, use the original:

```bash
npx unlighthouse --site <your-site>
```

---

## What this port does

The Rust core (`packages/core-rs`) replaces the Node.js orchestration layer with a single compiled binary while keeping two things in their original form:

- **Google Lighthouse** — still runs as a Node.js subprocess (it cannot be ported to Rust; it requires a real browser and the Lighthouse audit engine)
- **Vue dashboard** — still the original client from `packages/client`, served as static files by the Rust HTTP server

Everything else is Rust:

| Component | Implementation |
|-----------|---------------|
| CLI | `clap` |
| Config (TOML / JSON) | `serde` + `toml` |
| URL discovery (sitemap `.xml`/`.xml.gz`/`.txt`, `robots.txt`, crawler) | `reqwest` + `scraper` + `quick-xml` + `flate2` |
| HTTP server + REST API + MCP server | `axum` + JSON-RPC 2.0 (`--mcp`) |
| WebSocket broadcast | `axum` + `tokio::sync::broadcast` |
| Job queue + persistent worker pool | `tokio` + semaphore + self-healing IPC pool |
| HTML inspection & native fast-mode Web Vitals | `reqwest` + `scraper` (default), `headless_chrome`, or `chromiumoxide` (`--feature chromiumoxide-backend`) |
| Historical score trending | `sqlx` (SQLite) |
| CI reporters (JSON, CSV, Markdown, LHCI) | `serde_json` + `csv` + Markdown + LHCI upload |
| GUI & static dashboard export (`--build-static`) | `rust-embed` + inline `window.__unlighthouse_payload` injection |

---

## Recent Improvements

The Rust port has been recently optimized for performance and security:

- **Asset Compression**: Built-in Gzip/Brotli compression via `tower-http` reduces transfer sizes by ~75%.
- **Optimized Font Loading**: Fonts are now self-hosted locally with `<link rel="preload">` to eliminate layout shifts and improve LCP.
- **Hardened Security**: Implemented a robust Content Security Policy (CSP), HSTS, and Cross-Origin policies.
- **Layout Stability**: Fixed thumbnail aspect ratios and added explicit dimensions to minimize Cumulative Layout Shift (CLS).
- **Unified Dependencies**: Streamlined the Rust dependency tree for faster builds and smaller binary size.

---

### Development Workflow

The project now includes an automated development script that builds the client, syncs assets, and launches the Rust server in one command:

```bash
pnpm dev:rs --site https://example.com
```

### Manual Build (Prerequisites)

- Rust (stable) — [rustup.rs](https://rustup.rs)
- Node.js >= 20 — for the Lighthouse subprocess
- pnpm — for building the Vue client

### 1. Build the Vue client

```bash
pnpm install
cd packages/client && pnpm build
mkdir -p .unlighthouse/client
cp -r packages/client/dist/* .unlighthouse/client/
```

### 2. Build the Rust binary

```bash
cd packages/core-rs
cargo build --release
```

### 3. Run a scan

From the repo root:

```bash
./packages/core-rs/target/release/unlighthouse-rs \
  --site https://example.com \
  --lighthouse-process-path packages/core-rs/lighthouse.mjs
```

Open `http://localhost:5678` — the dashboard loads and populates in real time as routes are scanned.

---

## CLI reference

```
Usage: unlighthouse-rs [OPTIONS]

Options:
      --site <SITE>
          The site to audit [env: UNLIGHTHOUSE_SITE]
      --output-path <OUTPUT_PATH>
          Path to save reports and client files [default: .unlighthouse]
      --lighthouse-process-path <PATH>
          Path to the Lighthouse Node.js worker script
      --browser <BACKEND>
          HTML inspection backend: reqwest (default) | headless_chrome | chromiumoxide
      --workers <N>
          Number of concurrent Lighthouse workers
      --max-routes <N>
          Maximum number of routes to scan
      --device <DEVICE>
          Device to simulate: mobile | desktop
      --throttle
          Enable network/CPU throttling in Lighthouse
      --samples <N>
          Number of Lighthouse samples per URL (selects median run)
      --mode <MODE>
          Audit mode: full (default, runs Lighthouse) | fast (native CDP Web Vitals)
      --reporter <FORMAT>
          CI reporter format: json | jsonExpanded | csv | csvExpanded | markdown | lhci | none
      --budget <0-100>
          Exit with code 1 if average score is below this value
      --ci
          CI mode: no HTTP server, write report and exit
      --build-static
          Export a self-contained static client bundle into <output-path>/client/
      --mcp
          Start as a Model Context Protocol (MCP) JSON-RPC server over stdio
      --port <PORT>
          HTTP server port [default: 5678] (automatically falls back if busy)
      --host <HOST>
          HTTP server host [default: localhost]
      --include <PATHS>
          Paths to include (comma-separated: prefix, glob, or /regex/)
      --exclude <PATHS>
          Paths to exclude (comma-separated: prefix, glob, or /regex/)
      --skip-javascript
          Disable JavaScript execution during audits
      --warmup
          Navigate to the URL before running Lighthouse
      --block-assets
          Block images/fonts to speed up the audit
      --lhci-host <URL>
          Lighthouse CI (LHCI) server host
      --lhci-build-token <TOKEN>
          LHCI server build token
      --lhci-auth <USER:PASS>
          LHCI server Basic Auth credentials
      --crux-api-token <KEY>
          Google CrUX History API key [env: CRUX_API_TOKEN]
      --purge-runs-older-than-days <DAYS>
          Purge SQLite historical runs older than N days [default: 30]
  -c, --config <FILE>
          Path to config file (unlighthouse.config.toml or .json)
  -d, --debug
          Enable debug logging
      --no-cache
          Disable on-disk report caching
  -h, --help
          Print help
  -V, --version
          Print version
```

---

## Config file

Create `unlighthouse.config.toml` in your project root:

```toml
site = "https://example.com"
lighthouse_process_path = "packages/core-rs/lighthouse.mjs"
workers = 2

[scanner]
max_routes = 100
device = "mobile"
throttle = false
dynamic_sampling = 5
samples = 1

[ci]
enabled = false
reporter = "json"
budget = 80

[[budgets]]
path = "/checkout/**"
performance = 90
accessibility = 95
seo = 90
```

Or use `unlighthouse.config.json` if you prefer JSON.

---

## HTML inspection backends

The `--browser` flag controls how each page is fetched for SEO data and link discovery. Lighthouse always launches its own Chrome for scoring regardless of this setting.

| Backend | How it works | When to use |
|---------|-------------|-------------|
| `reqwest` (default) | Plain HTTP fetch + HTML parser. Fast, no Chrome required. | Most sites. Always try this first. |
| `headless_chrome` | Sync CDP client. Executes JavaScript. | JS-rendered SPAs where link discovery matters. |
| `chromiumoxide` | Async CDP client. Executes JavaScript. | Experimental. May crash on some Chrome versions. |

---

## CI mode

```bash
./packages/core-rs/target/release/unlighthouse-rs \
  --site https://example.com \
  --lighthouse-process-path packages/core-rs/lighthouse.mjs \
  --ci \
  --reporter json \
  --budget 90
```

Scans all routes, writes a report, and exits with code 1 if the average Lighthouse score is below the budget.

---

## Project structure

```
packages/
  core-rs/          ← Rust binary (this port)
    src/
      main.rs       ← CLI entry point
      config.rs     ← Config loading (TOML/JSON + CLI overrides)
      types.rs      ← Shared data types
      discovery/    ← Sitemap, robots.txt, HTML crawling
      queue/        ← Worker loop, browser backends, Lighthouse subprocess
      server/       ← Axum HTTP server, REST API, WebSocket
      reporters/    ← JSON and CSV CI reporters
    lighthouse.mjs  ← Node.js Lighthouse worker (called as subprocess)
    Cargo.toml

  client/           ← Original Vue dashboard (unchanged)
  core/             ← Original Node.js core (unchanged)
```

---

## License

Licensed under the [MIT license](LICENSE.md).

The original Unlighthouse project is also MIT licensed. Copyright © [Harlan Wilton](https://github.com/harlan-zw).

<!-- Badges -->
[license-src]: https://img.shields.io/github/license/bhubbard/unlighthouse-rs.svg?style=flat&colorA=18181B&colorB=28CF8D
[license-href]: https://github.com/bhubbard/unlighthouse-rs/blob/main/LICENSE.md
