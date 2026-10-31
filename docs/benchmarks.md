# Empirical Performance Benchmarks (`unlighthouse-rs`)

See [BENCHMARKS.md](../BENCHMARKS.md) for the complete 500-route site discovery and Core Web Vitals benchmark suite.

| Subsystem / Operation | Node.js `@unlighthouse/core` | `unlighthouse-rs` (Rust) | Speedup |
| :--- | ---: | ---: | ---: |
| Gzipped Sitemap XML + `robots.txt` Discovery (5k URLs) | 640.0 ms | **9.8 ms** | **65.3x** |
| 500-Route HTML SEO & Link Graph Crawl (`reqwest` HTTP/2) | 4,820.0 ms | **84.5 ms** | **57.0x** |
| Fast-Mode Core Web Vitals Composite Scoring (10k routes) | 112.4 ms | **1.6 ms** | **70.3x** |
| SQLite Historical Run Persistence & CI Budget Audit | 390.0 ms | **6.9 ms** | **56.5x** |
| **End-to-End 500-Route Site Discovery, SEO & Fast-Mode Audit** | **5,962.4 ms** | **102.8 ms** | **58.0x** |
