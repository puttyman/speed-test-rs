-- Initial schema (docs/SPEC.md §8).
--
-- Timestamps are RFC 3339 UTC, normalised on insert to `YYYY-MM-DDTHH:MM:SS.SSSZ`, so
-- text order is time order.
--
-- Additions to the SPEC schema: `down_loaded_jitter_ms` / `up_loaded_jitter_ms` (so the
-- denormalised summary is complete) and an index on `runs.group_id`.

CREATE TABLE groups (
  id                     TEXT PRIMARY KEY,
  created_at             TEXT NOT NULL,
  label                  TEXT,
  device_count           INTEGER NOT NULL,
  aggregate_download_bps REAL,
  aggregate_upload_bps   REAL,
  fairness_download      REAL,
  fairness_upload        REAL,
  detail_json            TEXT NOT NULL      -- GroupResult; 'null' until the server stores it
);

CREATE TABLE runs (
  id            TEXT PRIMARY KEY,           -- UUIDv7 (time-ordered)
  created_at    TEXT NOT NULL,              -- RFC 3339 UTC
  label         TEXT,
  notes         TEXT,
  transport     TEXT NOT NULL,
  profile       TEXT NOT NULL,
  streams       INTEGER NOT NULL,
  client_ip     TEXT,
  user_agent    TEXT,
  device_id     TEXT,                       -- random per-browser ID (SPEC §7.7)
  device_name   TEXT,
  tls           INTEGER NOT NULL DEFAULT 0,
  group_id      TEXT REFERENCES groups(id),
  peers_max     INTEGER NOT NULL DEFAULT 0, -- other sessions active during the run
  overlap_ratio REAL NOT NULL DEFAULT 0,
  download_bps  REAL,
  upload_bps    REAL,
  latency_ms    REAL,
  jitter_ms     REAL,
  down_loaded_latency_ms REAL,
  down_loaded_jitter_ms  REAL,
  up_loaded_latency_ms   REAL,
  up_loaded_jitter_ms    REAL,
  packet_loss_ratio      REAL,
  score_streaming TEXT,
  score_gaming    TEXT,
  score_rtc       TEXT,
  duration_ms   INTEGER NOT NULL,
  detail_json   TEXT NOT NULL               -- full RunResult (points, phases)
);

CREATE INDEX runs_created_at ON runs(created_at DESC, id DESC);
CREATE INDEX runs_label      ON runs(label, created_at DESC);
CREATE INDEX runs_device     ON runs(device_id, created_at DESC);
CREATE INDEX runs_group      ON runs(group_id);
