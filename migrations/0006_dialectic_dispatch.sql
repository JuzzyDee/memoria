-- migrations/0006_dialectic_dispatch.sql — Dialectic Stage 3 (CLA-100).
--
-- Stage 3 is the action dispatcher. It reads `action` + `action_payload`
-- from dialectic_decisions (Stage 2 product) and actually executes:
--
--   keep    — no-op, mark row dispatched
--   reframe — rewrite memory content/summary, re-embed, upsert Vectorize,
--             preserve original in memory_reframes
--   flag    — append a row to dialectic_flags for human review
--
-- This migration adds three things:
--
--   1. Dispatch tracking columns on dialectic_decisions
--      (dispatched_at + dispatch_status + dispatch_error). Idempotency
--      via WHERE dispatched_at IS NULL — dispatch only fires once per row.
--
--   2. memory_reframes table — append-only audit trail of every reframe.
--      Preserves the original (old_content, old_summary) so a bad reframe
--      can be rolled back via SQL. Reframe is the only destructive
--      operation in the system; this is the safety net.
--
--   3. dialectic_flags table — landing surface for Synthesizer-emitted
--      flags. Memory is NOT touched on flag; this is the queue Justin
--      reads when the dialectic asks for human attention.
--
-- Query patterns this enables:
--
--   -- What did Stage 3 do recently?
--   SELECT dispatched_at, memory_id, action, dispatch_status, dispatch_error
--   FROM dialectic_decisions
--   WHERE dispatched_at IS NOT NULL
--   ORDER BY dispatched_at DESC LIMIT 20;
--
--   -- What's still pending dispatch (should be empty after each run)?
--   SELECT decision_id, memory_id, action, created_at
--   FROM dialectic_decisions
--   WHERE dispatched_at IS NULL AND action IS NOT NULL
--   ORDER BY created_at;
--
--   -- All reframes ever applied to a specific memory (rollback context)
--   SELECT reframed_at, old_summary, new_summary
--   FROM memory_reframes
--   WHERE memory_id = ?
--   ORDER BY reframed_at DESC;
--
--   -- Unreviewed flags
--   SELECT flag_id, flagged_at, memory_id, note
--   FROM dialectic_flags
--   WHERE reviewed_at IS NULL
--   ORDER BY flagged_at DESC;

-- ──────────────────────────────────────────────────────────────────────
-- 1. Dispatch tracking on dialectic_decisions + run summary
-- ──────────────────────────────────────────────────────────────────────
ALTER TABLE dialectic_decisions ADD COLUMN dispatched_at    TEXT;
ALTER TABLE dialectic_decisions ADD COLUMN dispatch_status  TEXT;
ALTER TABLE dialectic_decisions ADD COLUMN dispatch_error   TEXT;

-- Run-level dispatch counter, parallels candidates_reviewed/decisions_count.
ALTER TABLE dialectic_runs ADD COLUMN actions_dispatched INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_dialectic_decisions_dispatched
    ON dialectic_decisions(dispatched_at);
CREATE INDEX IF NOT EXISTS idx_dialectic_decisions_dispatch_pending
    ON dialectic_decisions(action) WHERE dispatched_at IS NULL;

-- ──────────────────────────────────────────────────────────────────────
-- 2. memory_reframes — append-only history of every reframe
-- ──────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS memory_reframes (
    reframe_id      TEXT PRIMARY KEY,
    memory_id       TEXT NOT NULL,
    decision_id     TEXT NOT NULL,
    old_content     TEXT NOT NULL,
    old_summary     TEXT NOT NULL,
    new_content     TEXT NOT NULL,
    new_summary     TEXT NOT NULL,
    reframed_at     TEXT NOT NULL,
    FOREIGN KEY (decision_id) REFERENCES dialectic_decisions(decision_id)
);

CREATE INDEX IF NOT EXISTS idx_memory_reframes_memory
    ON memory_reframes(memory_id);
CREATE INDEX IF NOT EXISTS idx_memory_reframes_reframed_at
    ON memory_reframes(reframed_at DESC);

-- ──────────────────────────────────────────────────────────────────────
-- 3. dialectic_flags — landing surface for human-review proposals
-- ──────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS dialectic_flags (
    flag_id         TEXT PRIMARY KEY,
    decision_id     TEXT NOT NULL,
    memory_id       TEXT NOT NULL,
    note            TEXT NOT NULL,
    flagged_at      TEXT NOT NULL,
    reviewed_at     TEXT,
    resolution      TEXT,
    FOREIGN KEY (decision_id) REFERENCES dialectic_decisions(decision_id)
);

CREATE INDEX IF NOT EXISTS idx_dialectic_flags_memory
    ON dialectic_flags(memory_id);
CREATE INDEX IF NOT EXISTS idx_dialectic_flags_flagged_at
    ON dialectic_flags(flagged_at DESC);
CREATE INDEX IF NOT EXISTS idx_dialectic_flags_unreviewed
    ON dialectic_flags(flagged_at DESC) WHERE reviewed_at IS NULL;
