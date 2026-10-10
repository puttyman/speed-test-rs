//! Run history: SQLite persistence (docs/SPEC.md §8) behind the history REST API (§6.2).
//!
//! - The database lives at `config.db_path`; parent directories are created on open. It runs
//!   in WAL mode with `foreign_keys=ON`, and the migrations in `migrations/` are embedded with
//!   `sqlx::migrate!` and applied on open.
//! - Queries are runtime-checked (`sqlx::query*` + `FromRow`) rather than the compile-time
//!   macros, so building needs no `DATABASE_URL` or offline query cache.
//! - Each run is stored twice: the full [`RunResult`] in `detail_json`, and denormalised summary
//!   columns that the list, labels and compare queries read.
//! - Timestamps are normalised to `YYYY-MM-DDTHH:MM:SS.SSSZ` (UTC) on insert, so text order is
//!   time order. Lists are newest first, paginated with an opaque cursor over
//!   `(created_at DESC, id DESC)`.
//!
//! The response types ([`RunSummary`], [`RunPage`], [`LabelSummary`], [`CompareResult`], ...)
//! live in `crate::model::history` so the UI can deserialize them.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use futures_util::{Stream, TryStreamExt, stream};
use serde::Serialize;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};
use uuid::Uuid;

use super::config::Config;
use crate::model::result::{ClientInfo, GroupResult, RunResult, Summary};
use crate::model::scores::Classification;
use crate::model::stats;

/// Runs a label stands for in the labels summary and in compare (SPEC §7.7).
pub const LABEL_WINDOW: i64 = 5;
/// Default page size of [`Storage::list_runs`].
pub const DEFAULT_PAGE_SIZE: u32 = 50;
/// Largest page size of [`Storage::list_runs`].
pub const MAX_PAGE_SIZE: u32 = 200;
/// Rows fetched per page while exporting.
const EXPORT_PAGE_SIZE: u32 = 500;
/// SQLite `strftime` format of every stored timestamp.
const TIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%fZ";

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// A storage failure. The API maps the first three to 404, 409 and 400.
#[derive(Debug)]
pub enum StorageError {
    NotFound(String),
    Conflict(String),
    Invalid(String),
    Db(sqlx::Error),
    Migrate(sqlx::migrate::MigrateError),
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(m) | Self::Conflict(m) | Self::Invalid(m) => f.write_str(m),
            Self::Db(e) => write!(f, "database error: {e}"),
            Self::Migrate(e) => write!(f, "database migration failed: {e}"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Json(e) => write!(f, "stored JSON is invalid: {e}"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<sqlx::Error> for StorageError {
    fn from(e: sqlx::Error) -> Self {
        match &e {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                Self::Conflict("a run with this id already exists".into())
            }
            _ => Self::Db(e),
        }
    }
}

impl From<sqlx::migrate::MigrateError> for StorageError {
    fn from(e: sqlx::migrate::MigrateError) -> Self {
        Self::Migrate(e)
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

pub type Result<T, E = StorageError> = std::result::Result<T, E>;

// ---------------------------------------------------------------------------------------------
// Public types (API shapes live in `crate::model::history`, shared with the UI)
// ---------------------------------------------------------------------------------------------

pub use crate::model::history::{
    CompareItem, CompareKind, CompareResult, Delta, GroupRecord, LabelSummary, RunPage, RunSummary,
    ScoreClasses, SummaryDeltas,
};

/// Filters of [`Storage::list_runs`] and the export.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunFilter {
    /// Exact label.
    pub label: Option<String>,
    /// Device ID (SPEC §7.7).
    pub device: Option<String>,
    /// Inclusive lower bound: an RFC 3339 timestamp or a `YYYY-MM-DD` date.
    pub from: Option<String>,
    /// Upper bound: an RFC 3339 timestamp (inclusive) or a `YYYY-MM-DD` date (that whole day).
    pub to: Option<String>,
    /// Skip runs that overlapped with other devices' tests (SPEC §5.11).
    pub exclude_overlapped: bool,
}

/// What to compare: 2–4 labels or run IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareTarget {
    Labels(Vec<String>),
    Ids(Vec<Uuid>),
}

/// Export file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Json,
}

impl FromStr for ExportFormat {
    type Err = StorageError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "csv" => Ok(Self::Csv),
            "json" => Ok(Self::Json),
            _ => Err(StorageError::Invalid(format!(
                "unknown export format `{s}` (expected csv or json)"
            ))),
        }
    }
}

impl ExportFormat {
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Csv => "text/csv; charset=utf-8",
            Self::Json => "application/json",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }
}

/// Columns of the CSV export, in order.
pub const CSV_COLUMNS: &[&str] = &[
    "id",
    "created_at",
    "label",
    "notes",
    "transport",
    "profile",
    "streams",
    "client_ip",
    "user_agent",
    "device_id",
    "device_name",
    "tls",
    "group_id",
    "peers_max",
    "overlap_ratio",
    "download_bps",
    "upload_bps",
    "latency_ms",
    "jitter_ms",
    "down_loaded_latency_ms",
    "down_loaded_jitter_ms",
    "up_loaded_latency_ms",
    "up_loaded_jitter_ms",
    "packet_loss_ratio",
    "score_streaming",
    "score_gaming",
    "score_rtc",
    "duration_ms",
];

// ---------------------------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------------------------

const RUN_COLUMNS: &str = "id, created_at, label, notes, transport, profile, streams, client_ip, \
     user_agent, device_id, device_name, tls, group_id, peers_max, overlap_ratio, download_bps, \
     upload_bps, latency_ms, jitter_ms, down_loaded_latency_ms, down_loaded_jitter_ms, \
     up_loaded_latency_ms, up_loaded_jitter_ms, packet_loss_ratio, score_streaming, \
     score_gaming, score_rtc, duration_ms";

#[derive(Debug, FromRow)]
struct RunRow {
    id: String,
    created_at: String,
    label: Option<String>,
    notes: Option<String>,
    transport: String,
    profile: String,
    streams: i64,
    client_ip: Option<String>,
    user_agent: Option<String>,
    device_id: Option<String>,
    device_name: Option<String>,
    tls: bool,
    group_id: Option<String>,
    peers_max: i64,
    overlap_ratio: f64,
    download_bps: Option<f64>,
    upload_bps: Option<f64>,
    latency_ms: Option<f64>,
    jitter_ms: Option<f64>,
    down_loaded_latency_ms: Option<f64>,
    down_loaded_jitter_ms: Option<f64>,
    up_loaded_latency_ms: Option<f64>,
    up_loaded_jitter_ms: Option<f64>,
    packet_loss_ratio: Option<f64>,
    score_streaming: Option<String>,
    score_gaming: Option<String>,
    score_rtc: Option<String>,
    duration_ms: i64,
}

impl RunRow {
    fn into_summary(self) -> RunSummary {
        let class = |s: Option<String>| {
            s.and_then(|s| serde_json::from_value(serde_json::Value::String(s)).ok())
        };
        RunSummary {
            id: Uuid::parse_str(&self.id).unwrap_or_default(),
            created_at: self.created_at,
            label: self.label,
            notes: self.notes,
            transport: serde_json::from_value(serde_json::Value::String(self.transport))
                .unwrap_or_default(),
            profile: self.profile,
            streams: u8::try_from(self.streams).unwrap_or(u8::MAX),
            client: ClientInfo {
                ip: self.client_ip,
                user_agent: self.user_agent,
                device_id: self.device_id,
                device_name: self.device_name,
            },
            tls: self.tls,
            group_id: self.group_id.and_then(|g| Uuid::parse_str(&g).ok()),
            peers_max: u32::try_from(self.peers_max).unwrap_or(0),
            overlap_ratio: self.overlap_ratio,
            summary: Summary {
                download_bps: self.download_bps,
                upload_bps: self.upload_bps,
                latency_ms: self.latency_ms,
                jitter_ms: self.jitter_ms,
                down_loaded_latency_ms: self.down_loaded_latency_ms,
                down_loaded_jitter_ms: self.down_loaded_jitter_ms,
                up_loaded_latency_ms: self.up_loaded_latency_ms,
                up_loaded_jitter_ms: self.up_loaded_jitter_ms,
                packet_loss_ratio: self.packet_loss_ratio,
            },
            scores: ScoreClasses {
                streaming: class(self.score_streaming),
                gaming: class(self.score_gaming),
                rtc: class(self.score_rtc),
            },
            duration_ms: u64::try_from(self.duration_ms).unwrap_or(0),
        }
    }
}

#[derive(Debug, FromRow)]
struct DetailRow {
    id: String,
    created_at: String,
    detail_json: String,
}

#[derive(Debug, FromRow)]
struct GroupRow {
    id: String,
    created_at: String,
    label: Option<String>,
    device_count: i64,
    aggregate_download_bps: Option<f64>,
    aggregate_upload_bps: Option<f64>,
    fairness_download: Option<f64>,
    fairness_upload: Option<f64>,
    detail_json: String,
}

/// Position in the `(created_at DESC, id DESC)` order; serialised as an opaque hex token.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cursor {
    created_at: String,
    id: String,
}

impl Cursor {
    fn encode(&self) -> String {
        format!("{}|{}", self.created_at, self.id)
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn decode(token: &str) -> Result<Self> {
        let invalid = || StorageError::Invalid("invalid cursor".into());
        if !token.len().is_multiple_of(2) || !token.is_ascii() {
            return Err(invalid());
        }
        let bytes = (0..token.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&token[i..i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .map_err(|_| invalid())?;
        let text = String::from_utf8(bytes).map_err(|_| invalid())?;
        let (created_at, id) = text.split_once('|').ok_or_else(invalid)?;
        Ok(Self {
            created_at: created_at.to_owned(),
            id: id.to_owned(),
        })
    }
}

/// Serialises a unit-like enum to its serde name (`Transport::Websocket` → `"websocket"`).
fn serde_name<T: Serialize>(value: &T) -> Option<String> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => Some(s),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------------------------

/// The run history database. Cheap to clone (wraps the connection pool).
#[derive(Clone, Debug)]
pub struct Storage {
    pool: SqlitePool,
}

impl Storage {
    /// Opens the database when history is on (`Ok(None)` when it is off) and starts the
    /// retention task when `retention_days` is set. Call from inside the Tokio runtime.
    pub async fn from_config(config: &Config) -> Result<Option<Self>> {
        if !config.history {
            tracing::info!("history is off: runs are not stored");
            return Ok(None);
        }
        let storage = Self::open(&config.db_path).await?;
        tracing::info!("history database: {}", config.db_path.display());
        if let Some(days) = config.retention_days {
            storage.spawn_retention(days);
        }
        Ok(Some(storage))
    }

    /// Opens (creating if needed) the database at `path` and applies the migrations.
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| {
                StorageError::Io(std::io::Error::new(
                    e.kind(),
                    format!("cannot create {}: {e}", parent.display()),
                ))
            })?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    /// The underlying pool (for other server modules).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Closes the pool (flushes the WAL).
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Normalises an RFC 3339 timestamp or date (optionally shifted by an SQLite modifier such
    /// as `+1 day`) to the stored format. `None` if SQLite can't parse it.
    async fn normalize_time(&self, value: &str, modifier: &str) -> Result<Option<String>> {
        let normalized: Option<String> = sqlx::query_scalar("SELECT strftime(?1, ?2, ?3)")
            .bind(TIME_FORMAT)
            .bind(value.trim())
            .bind(modifier)
            .fetch_one(&self.pool)
            .await?;
        Ok(normalized)
    }

    // --- runs ---------------------------------------------------------------------------------

    /// Stores a run. `client_ip` and `user_agent` replace whatever the client sent (pass `None`
    /// when client info storage is off). A nil `id` gets a fresh UUIDv7. A `group_id` that has
    /// no group yet gets a placeholder group row, filled in by [`Self::insert_group`].
    pub async fn insert_run(
        &self,
        run: &RunResult,
        client_ip: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<Uuid> {
        let created_at = self
            .normalize_time(&run.created_at, "+0 seconds")
            .await?
            .ok_or_else(|| {
                StorageError::Invalid(format!("invalid created_at `{}`", run.created_at))
            })?;
        let mut run = run.clone();
        if run.id.is_nil() {
            run.id = Uuid::now_v7();
        }
        run.created_at = created_at;
        run.client.ip = client_ip.map(str::to_owned);
        run.client.user_agent = user_agent.map(str::to_owned);
        let detail = serde_json::to_string(&run)?;
        let id = run.id.to_string();
        let group_id = run.group_id.map(|g| g.to_string());
        let class = |s: Option<crate::model::scores::Score>| s.map(|s| s.class.as_str());

        let mut tx = self.pool.begin().await?;
        if let Some(group_id) = &group_id {
            sqlx::query(
                "INSERT INTO groups (id, created_at, device_count, detail_json) \
                 VALUES (?1, ?2, 0, 'null') ON CONFLICT(id) DO NOTHING",
            )
            .bind(group_id)
            .bind(&run.created_at)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO runs (id, created_at, label, notes, transport, profile, streams, \
             client_ip, user_agent, device_id, device_name, tls, group_id, peers_max, \
             overlap_ratio, download_bps, upload_bps, latency_ms, jitter_ms, \
             down_loaded_latency_ms, down_loaded_jitter_ms, up_loaded_latency_ms, \
             up_loaded_jitter_ms, packet_loss_ratio, score_streaming, score_gaming, score_rtc, \
             duration_ms, detail_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, \
             ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, \
             ?28, ?29)",
        )
        .bind(&id)
        .bind(&run.created_at)
        .bind(&run.label)
        .bind(&run.notes)
        .bind(serde_name(&run.transport))
        .bind(&run.profile)
        .bind(i64::from(run.streams))
        .bind(&run.client.ip)
        .bind(&run.client.user_agent)
        .bind(&run.client.device_id)
        .bind(&run.client.device_name)
        .bind(run.tls)
        .bind(&group_id)
        .bind(i64::from(run.concurrency.peers_max))
        .bind(run.concurrency.overlap_ratio)
        .bind(run.summary.download_bps)
        .bind(run.summary.upload_bps)
        .bind(run.summary.latency_ms)
        .bind(run.summary.jitter_ms)
        .bind(run.summary.down_loaded_latency_ms)
        .bind(run.summary.down_loaded_jitter_ms)
        .bind(run.summary.up_loaded_latency_ms)
        .bind(run.summary.up_loaded_jitter_ms)
        .bind(run.summary.packet_loss_ratio)
        .bind(class(run.scores.streaming))
        .bind(class(run.scores.gaming))
        .bind(class(run.scores.rtc))
        .bind(i64::try_from(run.duration_ms).unwrap_or(i64::MAX))
        .bind(&detail)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(run.id)
    }

    /// The full run, including raw points.
    pub async fn get_run(&self, id: Uuid) -> Result<RunResult> {
        let detail: Option<String> =
            sqlx::query_scalar("SELECT detail_json FROM runs WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await?;
        let detail = detail.ok_or_else(|| StorageError::NotFound("run not found".into()))?;
        Ok(serde_json::from_str(&detail)?)
    }

    /// The run's summary row.
    pub async fn get_run_summary(&self, id: Uuid) -> Result<RunSummary> {
        let sql = format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?");
        let row: Option<RunRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(RunRow::into_summary)
            .ok_or_else(|| StorageError::NotFound("run not found".into()))
    }

    /// Lists runs newest first. `limit` is clamped to `1..=MAX_PAGE_SIZE`.
    pub async fn list_runs(
        &self,
        filter: &RunFilter,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<RunPage> {
        let limit = limit.clamp(1, MAX_PAGE_SIZE);
        let cursor = cursor.map(Cursor::decode).transpose()?;
        let rows: Vec<RunRow> = self
            .fetch_page(RUN_COLUMNS, filter, cursor.as_ref(), limit + 1)
            .await?;
        let mut runs: Vec<RunSummary> = rows.into_iter().map(RunRow::into_summary).collect();
        let next_cursor = if runs.len() > limit as usize {
            runs.truncate(limit as usize);
            runs.last().map(|r| {
                Cursor {
                    created_at: r.created_at.clone(),
                    id: r.id.to_string(),
                }
                .encode()
            })
        } else {
            None
        };
        Ok(RunPage { runs, next_cursor })
    }

    /// One page of `columns` in list order.
    async fn fetch_page<T>(
        &self,
        columns: &str,
        filter: &RunFilter,
        cursor: Option<&Cursor>,
        limit: u32,
    ) -> Result<Vec<T>>
    where
        T: for<'r> FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin,
    {
        let mut qb = QueryBuilder::<Sqlite>::new(format!("SELECT {columns} FROM runs WHERE 1 = 1"));
        if let Some(label) = &filter.label {
            qb.push(" AND label = ").push_bind(label.clone());
        }
        if let Some(device) = &filter.device {
            qb.push(" AND device_id = ").push_bind(device.clone());
        }
        if let Some(from) = &filter.from {
            let from = self
                .normalize_time(from, "+0 seconds")
                .await?
                .ok_or_else(|| StorageError::Invalid(format!("invalid `from` time `{from}`")))?;
            qb.push(" AND created_at >= ").push_bind(from);
        }
        if let Some(to) = &filter.to {
            // A bare date covers that whole day.
            let date_only = to.trim().len() == 10;
            let modifier = if date_only { "+1 day" } else { "+0 seconds" };
            let bound = self
                .normalize_time(to, modifier)
                .await?
                .ok_or_else(|| StorageError::Invalid(format!("invalid `to` time `{to}`")))?;
            qb.push(if date_only {
                " AND created_at < "
            } else {
                " AND created_at <= "
            })
            .push_bind(bound);
        }
        if filter.exclude_overlapped {
            qb.push(" AND NOT (peers_max > 0 AND overlap_ratio > 0)");
        }
        if let Some(c) = cursor {
            qb.push(" AND (created_at < ")
                .push_bind(c.created_at.clone())
                .push(" OR (created_at = ")
                .push_bind(c.created_at.clone())
                .push(" AND id < ")
                .push_bind(c.id.clone())
                .push("))");
        }
        qb.push(" ORDER BY created_at DESC, id DESC LIMIT ")
            .push_bind(i64::from(limit));
        Ok(qb.build_query_as::<T>().fetch_all(&self.pool).await?)
    }

    /// Updates a run's label and/or notes. `None` leaves a field unchanged, `Some(None)` (or an
    /// empty string) clears it. Returns the updated summary.
    pub async fn update_run(
        &self,
        id: Uuid,
        label: Option<Option<String>>,
        notes: Option<Option<String>>,
    ) -> Result<RunSummary> {
        let clean = |v: Option<String>| v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        let mut tx = self.pool.begin().await?;
        let detail: Option<String> =
            sqlx::query_scalar("SELECT detail_json FROM runs WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        let detail = detail.ok_or_else(|| StorageError::NotFound("run not found".into()))?;
        let mut run: RunResult = serde_json::from_str(&detail)?;
        if let Some(label) = label {
            run.label = clean(label);
        }
        if let Some(notes) = notes {
            run.notes = clean(notes);
        }
        sqlx::query("UPDATE runs SET label = ?1, notes = ?2, detail_json = ?3 WHERE id = ?4")
            .bind(&run.label)
            .bind(&run.notes)
            .bind(serde_json::to_string(&run)?)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.get_run_summary(id).await
    }

    /// Deletes a run.
    pub async fn delete_run(&self, id: Uuid) -> Result<()> {
        let done = sqlx::query("DELETE FROM runs WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if done.rows_affected() == 0 {
            return Err(StorageError::NotFound("run not found".into()));
        }
        Ok(())
    }

    // --- labels and compare -------------------------------------------------------------------

    /// Distinct labels (optionally of one device), most recently used first, with the medians
    /// of their last [`LABEL_WINDOW`] runs.
    pub async fn labels(&self, device: Option<&str>) -> Result<Vec<LabelSummary>> {
        let device_clause = if device.is_some() {
            " AND device_id = ?1"
        } else {
            ""
        };
        let sql = format!(
            "SELECT label, COUNT(*) AS runs, MAX(created_at) AS last_used FROM runs \
             WHERE label IS NOT NULL{device_clause} GROUP BY label \
             ORDER BY last_used DESC, label ASC"
        );
        let mut q = sqlx::query_as::<_, (String, i64, String)>(sqlx::AssertSqlSafe(sql));
        if let Some(device) = device {
            q = q.bind(device.to_owned());
        }
        let groups = q.fetch_all(&self.pool).await?;

        let sql = format!(
            "SELECT label, download_bps, upload_bps, latency_ms FROM (\
               SELECT label, download_bps, upload_bps, latency_ms, ROW_NUMBER() OVER (\
                 PARTITION BY label ORDER BY created_at DESC, id DESC) AS rn \
               FROM runs WHERE label IS NOT NULL{device_clause}) \
             WHERE rn <= {LABEL_WINDOW}"
        );
        type Recent = (String, Option<f64>, Option<f64>, Option<f64>);
        let mut q = sqlx::query_as::<_, Recent>(sqlx::AssertSqlSafe(sql));
        if let Some(device) = device {
            q = q.bind(device.to_owned());
        }
        let recent = q.fetch_all(&self.pool).await?;

        Ok(groups
            .into_iter()
            .map(|(label, runs, last_used)| {
                let rows: Vec<&Recent> = recent.iter().filter(|r| r.0 == label).collect();
                let med = |f: fn(&Recent) -> Option<f64>| {
                    stats::median(&rows.iter().filter_map(|r| f(r)).collect::<Vec<_>>())
                };
                LabelSummary {
                    median_download_bps: med(|r| r.1),
                    median_upload_bps: med(|r| r.2),
                    median_latency_ms: med(|r| r.3),
                    label,
                    runs: u32::try_from(runs).unwrap_or(u32::MAX),
                    last_used,
                }
            })
            .collect())
    }

    /// Compares 2–4 labels (each the per-metric median of its last [`LABEL_WINDOW`] runs) or
    /// runs, with deltas against the first item.
    pub async fn compare(
        &self,
        target: &CompareTarget,
        exclude_overlapped: bool,
    ) -> Result<CompareResult> {
        let count = match target {
            CompareTarget::Labels(v) => v.len(),
            CompareTarget::Ids(v) => v.len(),
        };
        if !(2..=4).contains(&count) {
            return Err(StorageError::Invalid(format!(
                "compare needs 2 to 4 items, got {count}"
            )));
        }
        let mut items = Vec::with_capacity(count);
        match target {
            CompareTarget::Labels(labels) => {
                for label in labels {
                    let filter = RunFilter {
                        label: Some(label.clone()),
                        exclude_overlapped,
                        ..RunFilter::default()
                    };
                    let rows: Vec<RunRow> = self
                        .fetch_page(RUN_COLUMNS, &filter, None, LABEL_WINDOW as u32)
                        .await?;
                    if rows.is_empty() {
                        return Err(StorageError::NotFound(format!("label not found: {label}")));
                    }
                    let runs: Vec<RunSummary> =
                        rows.into_iter().map(RunRow::into_summary).collect();
                    items.push(CompareItem {
                        kind: CompareKind::Label,
                        key: label.clone(),
                        label: Some(label.clone()),
                        runs: runs.len() as u32,
                        run_ids: runs.iter().map(|r| r.id).collect(),
                        summary: median_summary(runs.iter().map(|r| &r.summary)),
                        deltas: None,
                    });
                }
            }
            CompareTarget::Ids(ids) => {
                for id in ids {
                    let run = match self.get_run_summary(*id).await {
                        Err(StorageError::NotFound(_)) => {
                            return Err(StorageError::NotFound(format!("run not found: {id}")));
                        }
                        other => other?,
                    };
                    items.push(CompareItem {
                        kind: CompareKind::Run,
                        key: id.to_string(),
                        label: run.label.clone(),
                        runs: 1,
                        run_ids: vec![run.id],
                        summary: run.summary,
                        deltas: None,
                    });
                }
            }
        }
        let reference = items[0].summary;
        for item in items.iter_mut().skip(1) {
            item.deltas = Some(summary_deltas(&reference, &item.summary));
        }
        Ok(CompareResult { items })
    }

    // --- groups -------------------------------------------------------------------------------

    /// Stores (or replaces) a group test result.
    pub async fn insert_group(&self, group: &GroupResult) -> Result<Uuid> {
        let created_at = self
            .normalize_time(&group.created_at, "+0 seconds")
            .await?
            .ok_or_else(|| {
                StorageError::Invalid(format!("invalid created_at `{}`", group.created_at))
            })?;
        // The members' common label, if they all share one.
        let mut labels = group.members.iter().map(|m| m.label.as_deref());
        let first = labels.next().flatten();
        let label = first.filter(|l| labels.all(|o| o == Some(*l)));
        let mut stored = group.clone();
        stored.created_at = created_at;
        sqlx::query(
            "INSERT INTO groups (id, created_at, label, device_count, aggregate_download_bps, \
             aggregate_upload_bps, fairness_download, fairness_upload, detail_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT(id) DO UPDATE SET \
             created_at = excluded.created_at, label = excluded.label, \
             device_count = excluded.device_count, \
             aggregate_download_bps = excluded.aggregate_download_bps, \
             aggregate_upload_bps = excluded.aggregate_upload_bps, \
             fairness_download = excluded.fairness_download, \
             fairness_upload = excluded.fairness_upload, detail_json = excluded.detail_json",
        )
        .bind(group.group_id.to_string())
        .bind(&stored.created_at)
        .bind(label)
        .bind(i64::try_from(group.members.len()).unwrap_or(i64::MAX))
        .bind(group.aggregate_download_bps)
        .bind(group.aggregate_upload_bps)
        .bind(group.fairness_download)
        .bind(group.fairness_upload)
        .bind(serde_json::to_string(&stored)?)
        .execute(&self.pool)
        .await?;
        Ok(group.group_id)
    }

    /// A group test with its member runs.
    pub async fn get_group(&self, id: Uuid) -> Result<GroupRecord> {
        let row: Option<GroupRow> = sqlx::query_as(
            "SELECT id, created_at, label, device_count, aggregate_download_bps, \
             aggregate_upload_bps, fairness_download, fairness_upload, detail_json \
             FROM groups WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        let row = row.ok_or_else(|| StorageError::NotFound("group not found".into()))?;
        let sql = format!(
            "SELECT {RUN_COLUMNS} FROM runs WHERE group_id = ? ORDER BY created_at DESC, id DESC"
        );
        let runs: Vec<RunRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(id.to_string())
            .fetch_all(&self.pool)
            .await?;
        let detail: Option<GroupResult> = serde_json::from_str(&row.detail_json)?;
        let runs: Vec<RunSummary> = runs.into_iter().map(RunRow::into_summary).collect();
        Ok(GroupRecord {
            id: Uuid::parse_str(&row.id).unwrap_or(id),
            created_at: row.created_at,
            label: row.label,
            device_count: if detail.is_some() {
                u32::try_from(row.device_count).unwrap_or(0)
            } else {
                runs.len() as u32
            },
            aggregate_download_bps: row.aggregate_download_bps,
            aggregate_upload_bps: row.aggregate_upload_bps,
            fairness_download: row.fairness_download,
            fairness_upload: row.fairness_upload,
            detail,
            runs,
        })
    }

    // --- retention ----------------------------------------------------------------------------

    /// Deletes runs older than `older_than_days` days, then groups of that age with no runs
    /// left. Returns the number of runs deleted.
    pub async fn prune(&self, older_than_days: u32) -> Result<u64> {
        let modifier = format!("-{older_than_days} days");
        let mut tx = self.pool.begin().await?;
        let runs = sqlx::query("DELETE FROM runs WHERE created_at < strftime(?1, 'now', ?2)")
            .bind(TIME_FORMAT)
            .bind(&modifier)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        sqlx::query(
            "DELETE FROM groups WHERE created_at < strftime(?1, 'now', ?2) \
             AND NOT EXISTS (SELECT 1 FROM runs WHERE runs.group_id = groups.id)",
        )
        .bind(TIME_FORMAT)
        .bind(&modifier)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(runs)
    }

    /// Prunes once now and then daily, in the background. `0` days disables pruning.
    pub fn spawn_retention(&self, days: u32) -> Option<tokio::task::JoinHandle<()>> {
        if days == 0 {
            tracing::warn!("--retention-days 0 ignored: runs are kept forever");
            return None;
        }
        tracing::info!("retention: runs older than {days} days are pruned daily");
        let storage = self.clone();
        Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                match storage.prune(days).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!("retention: pruned {n} runs older than {days} days"),
                    Err(e) => tracing::warn!("retention: prune failed: {e}"),
                }
            }
        }))
    }

    // --- export -------------------------------------------------------------------------------

    /// Streams the history (newest first) as CSV or a JSON array of full runs, one page of
    /// rows per chunk, so memory use stays flat however large the history is.
    pub fn export_stream(
        &self,
        format: ExportFormat,
        filter: RunFilter,
    ) -> impl Stream<Item = Result<String>> + Send + 'static {
        enum State {
            Start,
            Page { cursor: Option<Cursor>, first: bool },
            Done,
        }
        let storage = self.clone();
        stream::try_unfold(State::Start, move |state| {
            let storage = storage.clone();
            let filter = filter.clone();
            async move {
                let (cursor, first) = match state {
                    State::Done => return Ok(None),
                    State::Start => {
                        let head = match format {
                            ExportFormat::Csv => format!("{}\r\n", CSV_COLUMNS.join(",")),
                            ExportFormat::Json => "[".to_owned(),
                        };
                        let next = State::Page {
                            cursor: None,
                            first: true,
                        };
                        return Ok(Some((head, next)));
                    }
                    State::Page { cursor, first } => (cursor, first),
                };
                let (chunk, last) = match format {
                    ExportFormat::Csv => {
                        let rows: Vec<RunRow> = storage
                            .fetch_page(RUN_COLUMNS, &filter, cursor.as_ref(), EXPORT_PAGE_SIZE)
                            .await?;
                        let last = rows.last().map(|r| Cursor {
                            created_at: r.created_at.clone(),
                            id: r.id.clone(),
                        });
                        let mut out = String::new();
                        for row in rows {
                            csv_row(&mut out, &row.into_summary());
                        }
                        (out, last)
                    }
                    ExportFormat::Json => {
                        let rows: Vec<DetailRow> = storage
                            .fetch_page(
                                "id, created_at, detail_json",
                                &filter,
                                cursor.as_ref(),
                                EXPORT_PAGE_SIZE,
                            )
                            .await?;
                        let last = rows.last().map(|r| Cursor {
                            created_at: r.created_at.clone(),
                            id: r.id.clone(),
                        });
                        let mut out = String::new();
                        for (i, row) in rows.iter().enumerate() {
                            if !(first && i == 0) {
                                out.push(',');
                            }
                            out.push('\n');
                            out.push_str(&row.detail_json);
                        }
                        (out, last)
                    }
                };
                match last {
                    Some(cursor) => Ok(Some((
                        chunk,
                        State::Page {
                            cursor: Some(cursor),
                            first: false,
                        },
                    ))),
                    None => {
                        let tail = match format {
                            ExportFormat::Csv => String::new(),
                            ExportFormat::Json if first => "]\n".to_owned(),
                            ExportFormat::Json => "\n]\n".to_owned(),
                        };
                        Ok(Some((tail, State::Done)))
                    }
                }
            }
        })
        .try_filter(|chunk| std::future::ready(!chunk.is_empty()))
    }

    /// The whole history as CSV (header + one row per run, newest first).
    pub async fn export_csv(&self) -> Result<String> {
        self.export_stream(ExportFormat::Csv, RunFilter::default())
            .try_collect()
            .await
    }

    /// The whole history as a JSON array of full runs, newest first.
    pub async fn export_json(&self) -> Result<String> {
        self.export_stream(ExportFormat::Json, RunFilter::default())
            .try_collect()
            .await
    }
}

/// Per-metric medians of several summaries (missing values are skipped).
fn median_summary<'a>(summaries: impl Iterator<Item = &'a Summary> + Clone) -> Summary {
    let med = |f: fn(&Summary) -> Option<f64>| {
        stats::median(&summaries.clone().filter_map(f).collect::<Vec<_>>())
    };
    Summary {
        download_bps: med(|s| s.download_bps),
        upload_bps: med(|s| s.upload_bps),
        latency_ms: med(|s| s.latency_ms),
        jitter_ms: med(|s| s.jitter_ms),
        down_loaded_latency_ms: med(|s| s.down_loaded_latency_ms),
        down_loaded_jitter_ms: med(|s| s.down_loaded_jitter_ms),
        up_loaded_latency_ms: med(|s| s.up_loaded_latency_ms),
        up_loaded_jitter_ms: med(|s| s.up_loaded_jitter_ms),
        packet_loss_ratio: med(|s| s.packet_loss_ratio),
    }
}

/// Deltas of `value` against `reference`.
fn summary_deltas(reference: &Summary, value: &Summary) -> SummaryDeltas {
    let d = |r: Option<f64>, v: Option<f64>, higher_is_better: bool| {
        let (r, v) = (r?, v?);
        let abs = v - r;
        Some(Delta {
            abs,
            ratio: (r != 0.0).then(|| abs / r),
            better: (abs != 0.0).then_some((abs > 0.0) == higher_is_better),
        })
    };
    SummaryDeltas {
        download_bps: d(reference.download_bps, value.download_bps, true),
        upload_bps: d(reference.upload_bps, value.upload_bps, true),
        latency_ms: d(reference.latency_ms, value.latency_ms, false),
        jitter_ms: d(reference.jitter_ms, value.jitter_ms, false),
        down_loaded_latency_ms: d(
            reference.down_loaded_latency_ms,
            value.down_loaded_latency_ms,
            false,
        ),
        down_loaded_jitter_ms: d(
            reference.down_loaded_jitter_ms,
            value.down_loaded_jitter_ms,
            false,
        ),
        up_loaded_latency_ms: d(
            reference.up_loaded_latency_ms,
            value.up_loaded_latency_ms,
            false,
        ),
        up_loaded_jitter_ms: d(
            reference.up_loaded_jitter_ms,
            value.up_loaded_jitter_ms,
            false,
        ),
        packet_loss_ratio: d(reference.packet_loss_ratio, value.packet_loss_ratio, false),
    }
}

/// Appends one CSV record (RFC 4180, CRLF). Text cells that a spreadsheet would read as a
/// formula are prefixed with `'`.
fn csv_row(out: &mut String, r: &RunSummary) {
    fn text(v: Option<&str>) -> String {
        let Some(v) = v else {
            return String::new();
        };
        let v = if v.starts_with(['=', '+', '-', '@', '\t', '\r']) {
            format!("'{v}")
        } else {
            v.to_owned()
        };
        if v.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", v.replace('"', "\"\""))
        } else {
            v
        }
    }
    fn num(v: Option<f64>) -> String {
        v.map(|v| v.to_string()).unwrap_or_default()
    }
    let class = |c: Option<Classification>| text(c.map(Classification::as_str));
    let s = &r.summary;
    let cells = [
        r.id.to_string(),
        r.created_at.clone(),
        text(r.label.as_deref()),
        text(r.notes.as_deref()),
        text(serde_name(&r.transport).as_deref()),
        text(Some(&r.profile)),
        r.streams.to_string(),
        text(r.client.ip.as_deref()),
        text(r.client.user_agent.as_deref()),
        text(r.client.device_id.as_deref()),
        text(r.client.device_name.as_deref()),
        r.tls.to_string(),
        r.group_id.map(|g| g.to_string()).unwrap_or_default(),
        r.peers_max.to_string(),
        num(Some(r.overlap_ratio)),
        num(s.download_bps),
        num(s.upload_bps),
        num(s.latency_ms),
        num(s.jitter_ms),
        num(s.down_loaded_latency_ms),
        num(s.down_loaded_jitter_ms),
        num(s.up_loaded_latency_ms),
        num(s.up_loaded_jitter_ms),
        num(s.packet_loss_ratio),
        class(r.scores.streaming),
        class(r.scores.gaming),
        class(r.scores.rtc),
        r.duration_ms.to_string(),
    ];
    const _: () = assert!(CSV_COLUMNS.len() == 28);
    out.push_str(&cells.join(","));
    out.push_str("\r\n");
}

/// Fixtures shared by the storage and API tests.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::model::profile::PhaseStatus;
    use crate::model::protocol::Phase;
    use crate::model::result::Transport;
    use crate::model::result::{
        BandwidthPoint, Concurrency, PacketLossCounts, PhaseRecord, Points,
    };

    /// A temp-file database, removed with the returned directory.
    pub async fn temp_storage() -> (Storage, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(&dir.path().join("nested/dir/speedtest.db"))
            .await
            .unwrap();
        (storage, dir)
    }

    /// A realistic run: `down_mbps` download, half that upload, raw points and phases.
    pub fn sample_run(
        created_at: &str,
        label: Option<&str>,
        device: &str,
        down_mbps: f64,
    ) -> RunResult {
        let summary = Summary {
            download_bps: Some(down_mbps * 1e6),
            upload_bps: Some(down_mbps * 0.5e6),
            latency_ms: Some(2.0 + down_mbps / 1000.0),
            jitter_ms: Some(0.4),
            down_loaded_latency_ms: Some(12.5),
            down_loaded_jitter_ms: Some(1.5),
            up_loaded_latency_ms: Some(9.0),
            up_loaded_jitter_ms: Some(1.1),
            packet_loss_ratio: Some(0.0),
        };
        RunResult {
            id: Uuid::now_v7(),
            created_at: created_at.to_owned(),
            label: label.map(str::to_owned),
            notes: None,
            transport: Transport::Websocket,
            profile: "standard".into(),
            streams: 4,
            client: ClientInfo {
                ip: Some("10.9.9.9".into()),
                user_agent: Some("spoofed".into()),
                device_id: Some(device.to_owned()),
                device_name: Some(format!("{device} name")),
            },
            tls: false,
            group_id: None,
            concurrency: Concurrency::default(),
            scores: summary.scores(),
            summary,
            points: Points {
                latency: vec![2.1, 2.0, 2.4, 1.9],
                download: vec![
                    BandwidthPoint::new(1_000_000, 9.0, None),
                    BandwidthPoint::new(10_000_000, 85.0, None),
                ],
                upload: vec![BandwidthPoint::new(1_000_000, 18.0, Some(17.5))],
                down_loaded_latency: vec![11.0, 12.5, 14.0],
                up_loaded_latency: vec![8.0, 9.0],
                packet_loss: Some(PacketLossCounts::new(1000, 1000)),
            },
            phases: vec![
                PhaseRecord {
                    kind: Phase::Latency,
                    bytes: None,
                    count: 20,
                    status: PhaseStatus::Done,
                },
                PhaseRecord {
                    kind: Phase::Download,
                    bytes: Some(10_000_000),
                    count: 8,
                    status: PhaseStatus::Done,
                },
                PhaseRecord {
                    kind: Phase::PacketLoss,
                    bytes: None,
                    count: 0,
                    status: PhaseStatus::Skipped,
                },
            ],
            duration_ms: 18_250,
        }
    }

    /// `2026-10-<day>T<hour>:<minute>:00.000Z`.
    pub fn ts(day: u32, hour: u32, minute: u32) -> String {
        format!("2026-10-{day:02}T{hour:02}:{minute:02}:00.000Z")
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::model::protocol::DeviceKind;
    use crate::model::result::GroupMemberInput;

    #[tokio::test]
    async fn migrations_apply_and_pragmas_are_set() {
        let (storage, dir) = temp_storage().await;
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('runs', 'groups') \
             ORDER BY name",
        )
        .fetch_all(storage.pool())
        .await
        .unwrap();
        assert_eq!(tables, ["groups", "runs"]);
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(storage.pool())
            .await
            .unwrap();
        assert_eq!(mode, "wal");
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(storage.pool())
            .await
            .unwrap();
        assert_eq!(fk, 1);
        storage
            .insert_run(&sample_run(&ts(1, 0, 0), None, "d1", 1.0), None, None)
            .await
            .unwrap();
        storage.close().await;
        // Reopening applies nothing new and keeps the data.
        let again = Storage::open(&dir.path().join("nested/dir/speedtest.db"))
            .await
            .unwrap();
        let page = again
            .list_runs(&RunFilter::default(), 10, None)
            .await
            .unwrap();
        assert_eq!(page.runs.len(), 1);
    }

    #[tokio::test]
    async fn insert_get_roundtrip() {
        let (storage, _dir) = temp_storage().await;
        let mut run = sample_run(
            "2026-10-10T14:30:00.25+02:00",
            Some("Office"),
            "dev-1",
            940.0,
        );
        run.notes = Some("by the window".into());
        run.tls = true;
        let id = storage
            .insert_run(&run, Some("192.168.1.23"), Some("Mozilla/5.0"))
            .await
            .unwrap();
        assert_eq!(id, run.id);

        let got = storage.get_run(id).await.unwrap();
        let mut expected = run.clone();
        expected.created_at = "2026-10-10T12:30:00.250Z".into();
        expected.client.ip = Some("192.168.1.23".into());
        expected.client.user_agent = Some("Mozilla/5.0".into());
        assert_eq!(got, expected);

        let summary = storage.get_run_summary(id).await.unwrap();
        assert_eq!(summary.summary, run.summary);
        assert_eq!(summary.client, expected.client);
        assert!(summary.tls);
        assert_eq!(
            summary.scores.streaming,
            run.scores.streaming.map(|s| s.class)
        );
        assert_eq!(summary.duration_ms, 18_250);

        // Duplicate id, bad timestamp, nil id.
        assert!(matches!(
            storage.insert_run(&run, None, None).await,
            Err(StorageError::Conflict(_))
        ));
        let bad = sample_run("yesterday", None, "dev-1", 1.0);
        assert!(matches!(
            storage.insert_run(&bad, None, None).await,
            Err(StorageError::Invalid(_))
        ));
        let mut nil = sample_run(&ts(1, 0, 0), None, "dev-1", 1.0);
        nil.id = Uuid::nil();
        let id = storage.insert_run(&nil, None, None).await.unwrap();
        assert!(!id.is_nil());
        assert_eq!(storage.get_run(id).await.unwrap().client.ip, None);
        assert!(matches!(
            storage.get_run(Uuid::now_v7()).await,
            Err(StorageError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn list_filters_and_cursor_pagination() {
        let (storage, _dir) = temp_storage().await;
        // 25 runs; pairs share a timestamp so the id tie-break matters.
        for i in 0..25u32 {
            let label = if i % 2 == 0 { "A" } else { "B" };
            let device = if i < 10 { "d1" } else { "d2" };
            let run = sample_run(
                &ts(1 + i / 2, 12, 0),
                Some(label),
                device,
                100.0 + f64::from(i),
            );
            storage.insert_run(&run, None, None).await.unwrap();
        }

        let all = RunFilter::default();
        let mut seen = Vec::new();
        let mut cursor = None;
        let mut pages = 0;
        loop {
            let page = storage
                .list_runs(&all, 10, cursor.as_deref())
                .await
                .unwrap();
            pages += 1;
            seen.extend(page.runs.iter().map(|r| (r.created_at.clone(), r.id)));
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        assert_eq!(pages, 3);
        assert_eq!(seen.len(), 25);
        let mut sorted = seen.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(seen, sorted, "newest first, id DESC on ties");
        let mut unique: Vec<_> = seen.iter().map(|s| s.1).collect();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 25);

        let count = |filter: RunFilter| {
            let storage = storage.clone();
            async move {
                storage
                    .list_runs(&filter, 200, None)
                    .await
                    .unwrap()
                    .runs
                    .len()
            }
        };
        let label = |l: &str| RunFilter {
            label: Some(l.into()),
            ..RunFilter::default()
        };
        assert_eq!(count(label("A")).await, 13);
        assert_eq!(count(label("B")).await, 12);
        assert_eq!(count(label("C")).await, 0);
        assert_eq!(
            count(RunFilter {
                device: Some("d1".into()),
                ..RunFilter::default()
            })
            .await,
            10
        );
        // Days 1..=13; 2 runs per day except day 13.
        assert_eq!(
            count(RunFilter {
                from: Some("2026-10-12".into()),
                to: Some("2026-10-13".into()),
                ..RunFilter::default()
            })
            .await,
            3
        );
        assert_eq!(
            count(RunFilter {
                to: Some(ts(2, 12, 0)),
                ..RunFilter::default()
            })
            .await,
            4
        );
        // Label A (even i) on d2 (i >= 10) from day 6 (i >= 10): i = 10, 12, .., 24.
        assert_eq!(
            count(RunFilter {
                label: Some("A".into()),
                device: Some("d2".into()),
                from: Some("2026-10-06T00:00:00Z".into()),
                ..RunFilter::default()
            })
            .await,
            8
        );

        assert!(matches!(
            storage.list_runs(&all, 10, Some("zz")).await,
            Err(StorageError::Invalid(_))
        ));
        let bad_from = RunFilter {
            from: Some("not a date".into()),
            ..RunFilter::default()
        };
        assert!(matches!(
            storage.list_runs(&bad_from, 10, None).await,
            Err(StorageError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn labels_summarise_last_five_runs() {
        let (storage, _dir) = temp_storage().await;
        // Office: 7 runs of 100..700 Mbps, oldest first → last 5 are 300..700.
        for i in 1..=7u32 {
            let run = sample_run(&ts(i, 9, 0), Some("Office"), "d1", f64::from(i) * 100.0);
            storage.insert_run(&run, None, None).await.unwrap();
        }
        // Bedroom: used last, from another device.
        for (i, mbps) in [(8u32, 50.0), (9, 70.0)] {
            let run = sample_run(&ts(i, 9, 0), Some("Bedroom"), "d2", mbps);
            storage.insert_run(&run, None, None).await.unwrap();
        }
        // Unlabelled runs don't show up.
        storage
            .insert_run(&sample_run(&ts(10, 9, 0), None, "d1", 1.0), None, None)
            .await
            .unwrap();

        let labels = storage.labels(None).await.unwrap();
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0].label, "Bedroom");
        assert_eq!(labels[0].runs, 2);
        assert_eq!(labels[0].last_used, ts(9, 9, 0));
        assert_eq!(labels[0].median_download_bps, Some(60e6));
        let office = &labels[1];
        assert_eq!(office.runs, 7);
        assert_eq!(office.median_download_bps, Some(500e6));
        assert_eq!(office.median_upload_bps, Some(250e6));
        assert!((office.median_latency_ms.unwrap() - 2.5).abs() < 1e-9);

        let d1 = storage.labels(Some("d1")).await.unwrap();
        assert_eq!(d1.len(), 1);
        assert_eq!(d1[0].label, "Office");
        assert!(storage.labels(Some("nobody")).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn compare_labels_and_runs() {
        let (storage, _dir) = temp_storage().await;
        for i in 1..=6u32 {
            // Office medians over the last 5 (200..600) → 400 Mbps.
            let run = sample_run(&ts(i, 9, 0), Some("Office"), "d1", f64::from(i) * 100.0);
            storage.insert_run(&run, None, None).await.unwrap();
        }
        let mut bedroom_ids = Vec::new();
        for mbps in [480.0, 500.0, 520.0] {
            let run = sample_run(&ts(20, 9, 0), Some("Bedroom"), "d1", mbps);
            bedroom_ids.push(storage.insert_run(&run, None, None).await.unwrap());
        }

        let target = CompareTarget::Labels(vec!["Office".into(), "Bedroom".into()]);
        let res = storage.compare(&target, false).await.unwrap();
        assert_eq!(res.items.len(), 2);
        let (office, bedroom) = (&res.items[0], &res.items[1]);
        assert_eq!(office.kind, CompareKind::Label);
        assert_eq!(office.runs, 5);
        assert_eq!(office.run_ids.len(), 5);
        assert_eq!(office.summary.download_bps, Some(400e6));
        assert!(office.deltas.is_none());
        assert_eq!(bedroom.summary.download_bps, Some(500e6));
        let d = bedroom.deltas.unwrap();
        let down = d.download_bps.unwrap();
        assert_eq!(down.abs, 100e6);
        assert_eq!(down.ratio, Some(0.25));
        assert_eq!(down.better, Some(true));
        // Latency 2 + Mbps/1000: 2.4 → 2.5 ms is worse.
        let lat = d.latency_ms.unwrap();
        assert!((lat.abs - 0.1).abs() < 1e-9);
        assert_eq!(lat.better, Some(false));
        // Equal jitter: neither better nor worse; zero reference: no ratio.
        assert_eq!(d.jitter_ms.unwrap().better, None);
        assert_eq!(d.packet_loss_ratio.unwrap().ratio, None);

        let ids = CompareTarget::Ids(vec![bedroom_ids[2], bedroom_ids[0]]);
        let res = storage.compare(&ids, false).await.unwrap();
        assert_eq!(res.items[0].kind, CompareKind::Run);
        assert_eq!(res.items[0].label.as_deref(), Some("Bedroom"));
        let down = res.items[1].deltas.unwrap().download_bps.unwrap();
        assert_eq!(down.abs, -40e6);
        assert_eq!(down.better, Some(false));

        for bad in [
            CompareTarget::Labels(vec!["Office".into()]),
            CompareTarget::Labels(vec!["a".into(); 5]),
        ] {
            assert!(matches!(
                storage.compare(&bad, false).await,
                Err(StorageError::Invalid(_))
            ));
        }
        let unknown = CompareTarget::Labels(vec!["Office".into(), "Garage".into()]);
        assert!(matches!(
            storage.compare(&unknown, false).await,
            Err(StorageError::NotFound(m)) if m.contains("Garage")
        ));
    }

    #[tokio::test]
    async fn compare_can_skip_overlapped_runs() {
        let (storage, _dir) = temp_storage().await;
        for (i, overlapped) in [(1u32, false), (2, true)] {
            let mut run = sample_run(&ts(i, 9, 0), Some("A"), "d1", f64::from(i) * 100.0);
            if overlapped {
                run.concurrency.peers_max = 2;
                run.concurrency.overlap_ms = 5000.0;
                run.concurrency.overlap_ratio = 0.3;
            }
            storage.insert_run(&run, None, None).await.unwrap();
        }
        storage
            .insert_run(&sample_run(&ts(3, 9, 0), Some("B"), "d1", 1.0), None, None)
            .await
            .unwrap();
        let target = CompareTarget::Labels(vec!["A".into(), "B".into()]);
        let all = storage.compare(&target, false).await.unwrap();
        assert_eq!(all.items[0].summary.download_bps, Some(150e6));
        let solo = storage.compare(&target, true).await.unwrap();
        assert_eq!(solo.items[0].summary.download_bps, Some(100e6));
        let page = storage
            .list_runs(&RunFilter::default(), 10, None)
            .await
            .unwrap();
        assert_eq!(page.runs.iter().filter(|r| r.overlapped()).count(), 1);
    }

    #[tokio::test]
    async fn update_and_delete() {
        let (storage, _dir) = temp_storage().await;
        let run = sample_run(&ts(1, 9, 0), Some("Office"), "d1", 100.0);
        let id = storage.insert_run(&run, None, None).await.unwrap();

        let s = storage
            .update_run(id, Some(Some("  Office AP – 5 GHz ".into())), None)
            .await
            .unwrap();
        assert_eq!(s.label.as_deref(), Some("Office AP – 5 GHz"));
        let s = storage
            .update_run(id, None, Some(Some("moved the node".into())))
            .await
            .unwrap();
        assert_eq!(s.label.as_deref(), Some("Office AP – 5 GHz"));
        assert_eq!(s.notes.as_deref(), Some("moved the node"));
        let full = storage.get_run(id).await.unwrap();
        assert_eq!(full.label.as_deref(), Some("Office AP – 5 GHz"));
        assert_eq!(full.notes.as_deref(), Some("moved the node"));

        let s = storage
            .update_run(id, Some(None), Some(Some(String::new())))
            .await
            .unwrap();
        assert_eq!((s.label, s.notes), (None, None));
        assert!(storage.labels(None).await.unwrap().is_empty());

        storage.delete_run(id).await.unwrap();
        assert!(matches!(
            storage.get_run(id).await,
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(
            storage.delete_run(id).await,
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(
            storage.update_run(id, Some(None), None).await,
            Err(StorageError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn prune_removes_old_runs_and_empty_groups() {
        let (storage, _dir) = temp_storage().await;
        let now = storage
            .normalize_time("now", "+0 seconds")
            .await
            .unwrap()
            .unwrap();
        let old_day = storage
            .normalize_time("now", "-40 days")
            .await
            .unwrap()
            .unwrap();
        let recent = storage
            .insert_run(&sample_run(&now, None, "d1", 1.0), None, None)
            .await
            .unwrap();
        let old = storage
            .insert_run(&sample_run(&old_day, None, "d1", 1.0), None, None)
            .await
            .unwrap();
        let mut grouped = sample_run("2020-01-01T00:00:00Z", None, "d1", 1.0);
        let group_id = Uuid::now_v7();
        grouped.group_id = Some(group_id);
        storage.insert_run(&grouped, None, None).await.unwrap();

        assert_eq!(storage.prune(30).await.unwrap(), 2);
        assert!(storage.get_run(recent).await.is_ok());
        assert!(matches!(
            storage.get_run(old).await,
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(
            storage.get_group(group_id).await,
            Err(StorageError::NotFound(_))
        ));
        assert_eq!(storage.prune(30).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn groups_link_runs_and_results() {
        let (storage, _dir) = temp_storage().await;
        let group_id = Uuid::now_v7();
        let mut run = sample_run(&ts(5, 9, 0), Some("Office"), "d1", 300.0);
        run.group_id = Some(group_id);
        // The run may arrive before the server stores the group result.
        let run_id = storage.insert_run(&run, None, None).await.unwrap();
        let g = storage.get_group(group_id).await.unwrap();
        assert!(g.detail.is_none());
        assert_eq!(g.device_count, 1);
        assert_eq!(g.runs[0].id, run_id);

        let member = |name: &str, down: f64| GroupMemberInput {
            session: Uuid::now_v7(),
            device_name: name.into(),
            device_kind: DeviceKind::Phone,
            label: Some("Office".into()),
            download_bps: Some(down),
            upload_bps: Some(down / 2.0),
            down_loaded_latency_ms: Some(20.0),
            up_loaded_latency_ms: Some(15.0),
            incomplete: false,
            series: Vec::new(),
        };
        let result = GroupResult::compute(
            group_id,
            ts(5, 9, 0),
            10_000,
            vec![member("a", 300e6), member("b", 100e6)],
            None,
        );
        storage.insert_group(&result).await.unwrap();
        let g = storage.get_group(group_id).await.unwrap();
        assert_eq!(g.detail.as_ref(), Some(&result));
        assert_eq!(g.device_count, 2);
        assert_eq!(g.label.as_deref(), Some("Office"));
        assert_eq!(g.aggregate_download_bps, Some(400e6));
        assert_eq!(g.runs.len(), 1);
        assert!(matches!(
            storage.get_group(Uuid::now_v7()).await,
            Err(StorageError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn export_csv_and_json() {
        let (storage, _dir) = temp_storage().await;
        let empty = storage.export_json().await.unwrap();
        assert!(
            serde_json::from_str::<Vec<RunResult>>(&empty)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            storage.export_csv().await.unwrap(),
            format!("{}\r\n", CSV_COLUMNS.join(","))
        );

        let mut a = sample_run(&ts(1, 9, 0), Some("Desk, wired"), "d1", 940.0);
        a.notes = Some("=HYPERLINK(\"x\")".into());
        storage
            .insert_run(&a, Some("192.168.1.5"), None)
            .await
            .unwrap();
        let b = sample_run(&ts(2, 9, 0), None, "d2", 300.0);
        storage.insert_run(&b, None, None).await.unwrap();

        let csv = storage.export_csv().await.unwrap();
        let lines: Vec<&str> = csv.split("\r\n").filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("id,created_at,label,notes,transport,profile,streams,"));
        assert!(lines[1].starts_with(&format!("{},{},,,websocket,standard,4,", b.id, ts(2, 9, 0))));
        assert!(lines[2].contains(",\"Desk, wired\",\"'=HYPERLINK(\"\"x\"\")\",websocket,"));
        assert!(lines[2].contains(",192.168.1.5,,d1,d1 name,false,,0,0,940000000,470000000,"));
        assert!(lines[2].ends_with(",18250"));
        for line in &lines[1..] {
            // Every row has as many cells as the header (no commas in these values but the
            // quoted label).
            let unquoted = line.replace("\"Desk, wired\"", "x");
            assert_eq!(unquoted.split(',').count(), CSV_COLUMNS.len(), "{line}");
        }

        let json = storage.export_json().await.unwrap();
        let runs: Vec<RunResult> = serde_json::from_str(&json).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, b.id);
        assert_eq!(runs[1].label.as_deref(), Some("Desk, wired"));
        assert_eq!(runs[1].points, a.points);
    }

    #[tokio::test]
    async fn export_pages_through_large_histories() {
        let (storage, _dir) = temp_storage().await;
        let n = EXPORT_PAGE_SIZE as usize + 3;
        for i in 0..n {
            let at = format!("2026-09-01T00:00:{:02}.{:03}Z", i / 1000, i % 1000);
            storage
                .insert_run(&sample_run(&at, None, "d", 1.0), None, None)
                .await
                .unwrap();
        }
        let csv = storage.export_csv().await.unwrap();
        assert_eq!(csv.split("\r\n").filter(|l| !l.is_empty()).count(), n + 1);
        let json = storage.export_json().await.unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<RunResult>>(&json).unwrap().len(),
            n
        );
    }

    #[test]
    fn cursor_roundtrip() {
        let c = Cursor {
            created_at: ts(1, 2, 3),
            id: Uuid::now_v7().to_string(),
        };
        assert_eq!(Cursor::decode(&c.encode()).unwrap(), c);
        for bad in ["abc", "zz", "6869"] {
            assert!(Cursor::decode(bad).is_err(), "{bad}");
        }
    }
}
