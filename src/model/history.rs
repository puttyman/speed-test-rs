//! Response shapes of the history REST API (SPEC §6.2), shared by the server
//! (`server::storage`, `server::runs_api`) and the UI.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::result::{ClientInfo, GroupResult, Summary, Transport};
use crate::model::scores::Classification;

/// The AIM classes of a run, as stored in the `score_*` columns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreClasses {
    pub streaming: Option<Classification>,
    pub gaming: Option<Classification>,
    pub rtc: Option<Classification>,
}

/// A run without its raw points and phases: one row of the history list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: Uuid,
    pub created_at: String,
    pub label: Option<String>,
    pub notes: Option<String>,
    pub transport: Transport,
    pub profile: String,
    pub streams: u8,
    pub client: ClientInfo,
    pub tls: bool,
    pub group_id: Option<Uuid>,
    /// Other sessions active during the run (SPEC §5.11).
    pub peers_max: u32,
    pub overlap_ratio: f64,
    pub summary: Summary,
    pub scores: ScoreClasses,
    pub duration_ms: u64,
}

impl RunSummary {
    /// True if another session overlapped this run.
    pub fn overlapped(&self) -> bool {
        self.peers_max > 0 && self.overlap_ratio > 0.0
    }
}

/// One page of runs, newest first. Pass `next_cursor` back as `cursor` for the next page;
/// `None` means this was the last page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunPage {
    pub runs: Vec<RunSummary>,
    pub next_cursor: Option<String>,
}

/// A label with its run count, last use and the medians of its last 5 (`server::storage::LABEL_WINDOW`) runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelSummary {
    pub label: String,
    pub runs: u32,
    pub last_used: String,
    pub median_download_bps: Option<f64>,
    pub median_upload_bps: Option<f64>,
    pub median_latency_ms: Option<f64>,
}

/// Whether an item is a label (median of its last runs) or a single run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompareKind {
    Label,
    Run,
}

/// A metric's difference against the first compared item.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    /// `value - reference`, in the metric's unit.
    pub abs: f64,
    /// `(value - reference) / reference`; `None` when the reference is 0.
    pub ratio: Option<f64>,
    /// Whether this is better than the reference (higher throughput, lower latency, jitter
    /// and loss); `None` when equal.
    pub better: Option<bool>,
}

/// Per-metric deltas, with the field names of [`Summary`]. A field is `None` when either side
/// lacks the metric.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct SummaryDeltas {
    pub download_bps: Option<Delta>,
    pub upload_bps: Option<Delta>,
    pub latency_ms: Option<Delta>,
    pub jitter_ms: Option<Delta>,
    pub down_loaded_latency_ms: Option<Delta>,
    pub down_loaded_jitter_ms: Option<Delta>,
    pub up_loaded_latency_ms: Option<Delta>,
    pub up_loaded_jitter_ms: Option<Delta>,
    pub packet_loss_ratio: Option<Delta>,
}

/// One compared item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareItem {
    pub kind: CompareKind,
    /// The label, or the run ID.
    pub key: String,
    /// The label (for a run: its label, if any).
    pub label: Option<String>,
    /// Runs the summary is computed from (≤ 5 (`server::storage::LABEL_WINDOW`) for a label, 1 for a run).
    pub runs: u32,
    /// Those runs, newest first. Fetch `/api/runs/{id}` for their points.
    pub run_ids: Vec<Uuid>,
    /// The run's summary, or the per-metric medians of the label's runs.
    pub summary: Summary,
    /// Differences against the first item; `None` on the first item itself.
    pub deltas: Option<SummaryDeltas>,
}

/// The compare view's data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareResult {
    pub items: Vec<CompareItem>,
}

/// A stored group test with the runs that belong to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupRecord {
    pub id: Uuid,
    pub created_at: String,
    pub label: Option<String>,
    pub device_count: u32,
    pub aggregate_download_bps: Option<f64>,
    pub aggregate_upload_bps: Option<f64>,
    pub fairness_download: Option<f64>,
    pub fairness_upload: Option<f64>,
    /// The server-computed group result; `None` while only members' runs have been posted.
    pub detail: Option<GroupResult>,
    /// Member runs posted with this `group_id`, newest first.
    pub runs: Vec<RunSummary>,
}
