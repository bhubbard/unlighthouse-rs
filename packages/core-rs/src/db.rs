//! SQLite persistence layer — stores every completed scan run and its per-route
//! scores so the dashboard can show score trend lines across historical runs.
//!
//! # Schema
//!
//! ```text
//! runs           — one row per scan invocation (site, started_at, finished_at)
//! route_scores   — one row per (run × route); holds all numeric scores/metrics
//! ```
//!
//! The DB file is created automatically at `{output_path}/unlighthouse.db` the
//! first time the binary starts.  SQLite is used via `sqlx` with the tokio
//! async runtime; all queries use the non-macro `sqlx::query()` form so no
//! compile-time `DATABASE_URL` environment variable or `.sqlx/` directory is
//! required.

use anyhow::Result;
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

use crate::types::RouteReport;

// ── DDL ───────────────────────────────────────────────────────────────────────

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS runs (
    id          TEXT    PRIMARY KEY,
    site        TEXT    NOT NULL,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER,
    mode        TEXT    NOT NULL DEFAULT 'full',
    route_count INTEGER
);

CREATE TABLE IF NOT EXISTS route_scores (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id         TEXT    NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    path           TEXT    NOT NULL,
    url            TEXT    NOT NULL,
    score          REAL,
    performance    REAL,
    accessibility  REAL,
    best_practices REAL,
    seo_score      REAL,
    status_code    INTEGER,
    lcp            REAL,
    cls            REAL,
    fcp            REAL,
    ttfb           REAL,
    tbt            REAL,
    recorded_at    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_rs_run_id ON route_scores(run_id);
CREATE INDEX IF NOT EXISTS idx_rs_path   ON route_scores(path);
CREATE INDEX IF NOT EXISTS idx_runs_site ON runs(site);
CREATE INDEX IF NOT EXISTS idx_rs_run_path ON route_scores(run_id, path);
"#;

// ── Public row types (returned by query helpers and serialised to JSON) ───────

/// A single scan-run record.
#[derive(sqlx::FromRow, serde::Serialize, Clone, Debug)]
pub struct RunRecord {
    pub id:          String,
    pub site:        String,
    pub started_at:  i64,
    pub finished_at: Option<i64>,
    pub mode:        String,
    pub route_count: Option<i64>,
}

/// All numeric scores for one route within one run.
#[derive(sqlx::FromRow, serde::Serialize, Clone, Debug)]
pub struct RouteScoreRecord {
    pub id:             i64,
    pub run_id:         String,
    pub path:           String,
    pub url:            String,
    pub score:          Option<f64>,
    pub performance:    Option<f64>,
    pub accessibility:  Option<f64>,
    pub best_practices: Option<f64>,
    pub seo_score:      Option<f64>,
    pub status_code:    Option<i64>,
    pub lcp:            Option<f64>,
    pub cls:            Option<f64>,
    pub fcp:            Option<f64>,
    pub ttfb:           Option<f64>,
    pub tbt:            Option<f64>,
    pub recorded_at:    i64,
}

/// Trend point — one data-point per run for a given route path.
/// Used by the `/api/history/route` endpoint.
#[derive(sqlx::FromRow, serde::Serialize, Clone, Debug)]
pub struct RouteTrendPoint {
    pub run_id:         String,
    pub started_at:     i64,
    pub score:          Option<f64>,
    pub performance:    Option<f64>,
    pub accessibility:  Option<f64>,
    pub best_practices: Option<f64>,
    pub seo_score:      Option<f64>,
    pub lcp:            Option<f64>,
    pub cls:            Option<f64>,
    pub fcp:            Option<f64>,
    pub ttfb:           Option<f64>,
    pub tbt:            Option<f64>,
}

// ── Pool init ─────────────────────────────────────────────────────────────────

/// Open (or create) the SQLite database at `db_path` and apply the schema.
///
/// Uses WAL mode and foreign keys configured natively in the connection pool.
pub async fn open(db_path: &str) -> Result<SqlitePool> {
    // Ensure parent directory exists.
    if let Some(parent) = std::path::Path::new(db_path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        // `mode=rwc` — create the file if it does not yet exist.
        .connect(&format!("sqlite://{}?mode=rwc&_journal_mode=WAL&_foreign_keys=on&_busy_timeout=5000", db_path))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to open SQLite DB at {db_path}: {e}"))?;

    // Apply schema idempotently (IF NOT EXISTS guards every statement).
    sqlx::raw_sql(SCHEMA).execute(&pool).await?;

    Ok(pool)
}

/// Purge scan runs older than N days from the database.
/// SQLite foreign key cascades will automatically delete associated route scores.
pub async fn purge_old_runs(db: &SqlitePool, days: i64) -> Result<u64> {
    let threshold = chrono::Utc::now().timestamp_millis() - (days * 24 * 60 * 60 * 1000);
    let res = sqlx::query("DELETE FROM runs WHERE started_at < ?")
        .bind(threshold)
        .execute(db)
        .await?;
    Ok(res.rows_affected())
}

// ── Run lifecycle ─────────────────────────────────────────────────────────────

/// Record the start of a new scan run.
pub async fn insert_run(
    db: &SqlitePool,
    run_id: &str,
    site: &str,
    mode: &str,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query(
        "INSERT OR IGNORE INTO runs (id, site, started_at, mode) VALUES (?, ?, ?, ?)",
    )
    .bind(run_id)
    .bind(site)
    .bind(now)
    .bind(mode)
    .execute(db)
    .await?;
    Ok(())
}

/// Mark a run as complete and record the final route count.
pub async fn finish_run(db: &SqlitePool, run_id: &str, route_count: i64) -> Result<()> {
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query(
        "UPDATE runs SET finished_at = ?, route_count = ? WHERE id = ?",
    )
    .bind(now)
    .bind(route_count)
    .bind(run_id)
    .execute(db)
    .await?;
    Ok(())
}

// ── Route score persistence ───────────────────────────────────────────────────

/// Persist the scores from a completed [`RouteReport`] into the current run.
///
/// Called by the worker immediately after a route reaches `Completed` status.
pub async fn insert_route_score(
    db: &SqlitePool,
    run_id: &str,
    report: &RouteReport,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp_millis();

    // Composite score: Lighthouse in full mode, Web Vitals in fast mode.
    let score = report
        .report
        .as_ref()
        .map(|r| r.score)
        .or_else(|| report.web_vitals.as_ref().map(|wv| wv.score));

    // Per-category Lighthouse scores (full mode only).
    let performance = report
        .report
        .as_ref()
        .and_then(|r| r.categories.get("performance"))
        .and_then(|c| c.score);
    let accessibility = report
        .report
        .as_ref()
        .and_then(|r| r.categories.get("accessibility"))
        .and_then(|c| c.score);
    let best_practices = report
        .report
        .as_ref()
        .and_then(|r| r.categories.get("best-practices"))
        .and_then(|c| c.score);
    let seo_score = report
        .report
        .as_ref()
        .and_then(|r| r.categories.get("seo"))
        .and_then(|c| c.score);

    // HTTP health.
    let status_code = report
        .seo
        .as_ref()
        .and_then(|s| s.status_code)
        .map(|c| c as i64);

    // Web Vitals (fast mode).
    let lcp  = report.web_vitals.as_ref().and_then(|wv| wv.lcp);
    let cls  = report.web_vitals.as_ref().and_then(|wv| wv.cls);
    let fcp  = report.web_vitals.as_ref().and_then(|wv| wv.fcp);
    let ttfb = report.web_vitals.as_ref().and_then(|wv| wv.ttfb);
    let tbt  = report.web_vitals.as_ref().and_then(|wv| wv.tbt);

    sqlx::query(
        r#"INSERT INTO route_scores
           (run_id, path, url, score, performance, accessibility, best_practices,
            seo_score, status_code, lcp, cls, fcp, ttfb, tbt, recorded_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(run_id)
    .bind(&report.route.path)
    .bind(&report.route.url)
    .bind(score)
    .bind(performance)
    .bind(accessibility)
    .bind(best_practices)
    .bind(seo_score)
    .bind(status_code)
    .bind(lcp)
    .bind(cls)
    .bind(fcp)
    .bind(ttfb)
    .bind(tbt)
    .bind(now)
    .execute(db)
    .await?;

    Ok(())
}

// ── Query helpers (used by API handlers) ──────────────────────────────────────

/// Return the most recent N runs for a given site, newest first.
pub async fn list_runs(
    db: &SqlitePool,
    site: &str,
    limit: i64,
) -> Result<Vec<RunRecord>> {
    let rows = sqlx::query_as::<_, RunRecord>(
        "SELECT id, site, started_at, finished_at, mode, route_count
         FROM runs
         WHERE site = ?
         ORDER BY started_at DESC
         LIMIT ?",
    )
    .bind(site)
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Return all route scores for a specific run.
pub async fn get_run_scores(
    db: &SqlitePool,
    run_id: &str,
) -> Result<Vec<RouteScoreRecord>> {
    let rows = sqlx::query_as::<_, RouteScoreRecord>(
        "SELECT id, run_id, path, url, score, performance, accessibility,
                best_practices, seo_score, status_code,
                lcp, cls, fcp, ttfb, tbt, recorded_at
         FROM route_scores
         WHERE run_id = ?
         ORDER BY path ASC",
    )
    .bind(run_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Return the score trend for one route path across all finished runs,
/// oldest-first so the dashboard can plot a time-series directly.
pub async fn get_route_trend(
    db: &SqlitePool,
    site: &str,
    path: &str,
) -> Result<Vec<RouteTrendPoint>> {
    let rows = sqlx::query_as::<_, RouteTrendPoint>(
        r#"SELECT rs.run_id,
                  r.started_at,
                  rs.score,
                  rs.performance,
                  rs.accessibility,
                  rs.best_practices,
                  rs.seo_score,
                  rs.lcp,
                  rs.cls,
                  rs.fcp,
                  rs.ttfb,
                  rs.tbt
           FROM route_scores rs
           JOIN runs r ON r.id = rs.run_id
           WHERE r.site = ?
             AND rs.path = ?
             AND r.finished_at IS NOT NULL
           ORDER BY r.started_at ASC"#,
    )
    .bind(site)
    .bind(path)
    .fetch_all(db)
    .await?;
    Ok(rows)
}
