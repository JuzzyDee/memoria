-- migrations/0005_dialectic_audit.sql — Dialectic-on-CF (CLA-95).
--
-- The dialectic is the second cognitive loop on the worker, complementing
-- REM. Where REM consolidates upward (episodes → semantics), the dialectic
-- adversarially scrutinises whether existing memories are well-calibrated
-- — catching milestone inflation, validation gravity, overclaim, and the
-- system-specific failure modes the local dialectic surfaced over months.
--
-- Schema designed for all three migration stages so we don't churn:
--
--   Stage 1 (CLA-95)  — single neutral assessor sets `assessment` + `rationale`.
--                       `transcript`, `resolution`, `action`, `action_payload`
--                       remain NULL. Pure judgment-only telemetry; no action
--                       on the memory store.
--
--   Stage 2 (follow)  — Advocate vs Challenger multi-turn dialogue. The
--                       full transcript lands in `transcript` (JSON), and
--                       `resolution` records consensus / concession /
--                       deadlock.
--
--   Stage 3 (follow)  — Action dispatch lights up. `action` records
--                       reframe / forget / flag / none, and
--                       `action_payload` carries the JSON details
--                       (new content + summary for reframe, reason for
--                       forget, etc.).
--
-- Query patterns this enables:
--
--   -- What did the dialectic flag recently and why?
--   SELECT created_at, memory_id, assessment, rationale
--   FROM dialectic_decisions
--   WHERE assessment != 'well_calibrated'
--   ORDER BY created_at DESC LIMIT 20;
--
--   -- Which memories has the dialectic looked at?
--   SELECT memory_id, MAX(created_at) AS last_reviewed,
--          COUNT(*) AS times_reviewed
--   FROM dialectic_decisions
--   GROUP BY memory_id
--   ORDER BY last_reviewed DESC;
--
--   -- Stage 3 onward: what reframes did the dialectic produce?
--   SELECT created_at, memory_id, action_payload
--   FROM dialectic_decisions
--   WHERE action = 'reframe'
--   ORDER BY created_at DESC;

CREATE TABLE IF NOT EXISTS dialectic_runs (
    run_id              TEXT PRIMARY KEY,
    started_at          TEXT NOT NULL,
    finished_at         TEXT,
    candidates_reviewed INTEGER NOT NULL DEFAULT 0,
    decisions_count     INTEGER NOT NULL DEFAULT 0,
    errors_count        INTEGER NOT NULL DEFAULT 0,
    errors_summary      TEXT
);

CREATE INDEX IF NOT EXISTS idx_dialectic_runs_started ON dialectic_runs(started_at DESC);

CREATE TABLE IF NOT EXISTS dialectic_decisions (
    decision_id     TEXT PRIMARY KEY,
    run_id          TEXT NOT NULL,
    memory_id       TEXT NOT NULL,
    -- Stage 1 fields: neutral-assessor judgment
    assessment      TEXT NOT NULL,
    rationale       TEXT NOT NULL,
    -- Stage 2 fields: Advocate vs Challenger dialogue
    transcript      TEXT,
    resolution      TEXT,
    -- Stage 3 fields: action taken on the memory
    action          TEXT,
    action_payload  TEXT,
    created_at      TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES dialectic_runs(run_id)
);

CREATE INDEX IF NOT EXISTS idx_dialectic_decisions_run ON dialectic_decisions(run_id);
CREATE INDEX IF NOT EXISTS idx_dialectic_decisions_memory ON dialectic_decisions(memory_id);
CREATE INDEX IF NOT EXISTS idx_dialectic_decisions_assessment ON dialectic_decisions(assessment);
CREATE INDEX IF NOT EXISTS idx_dialectic_decisions_created ON dialectic_decisions(created_at DESC);
