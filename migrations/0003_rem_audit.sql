-- migrations/0003_rem_audit.sql — REM observability tables (CLA-87).
--
-- One row per nightly REM invocation (rem_runs) and one row per Haiku
-- decision within a run (rem_decisions). Persistent records so post-hoc
-- investigation doesn't depend on the live wrangler tail buffer.
--
-- Query patterns this enables:
--
--   -- What did REM reject recently and why?
--   SELECT created_at, action, members, rationale
--   FROM rem_decisions
--   WHERE action = 'skip'
--   ORDER BY created_at DESC LIMIT 20;
--
--   -- Which nights produced consolidations vs nothing?
--   SELECT started_at, clusters_attempted, decisions_created
--   FROM rem_runs
--   ORDER BY started_at DESC LIMIT 30;
--
--   -- What decisions led to this consolidated semantic?
--   SELECT d.rationale, d.action, d.members
--   FROM rem_decisions d
--   WHERE d.result_memory_id = ?;

CREATE TABLE IF NOT EXISTS rem_runs (
    run_id TEXT PRIMARY KEY,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    decayed INTEGER NOT NULL DEFAULT 0,
    pairs_found INTEGER NOT NULL DEFAULT 0,
    clusters_attempted INTEGER NOT NULL DEFAULT 0,
    decisions_created INTEGER NOT NULL DEFAULT 0,
    decisions_appended INTEGER NOT NULL DEFAULT 0,
    decisions_revised INTEGER NOT NULL DEFAULT 0,
    decisions_skipped INTEGER NOT NULL DEFAULT 0,
    errors_count INTEGER NOT NULL DEFAULT 0,
    errors_summary TEXT
);

CREATE INDEX IF NOT EXISTS idx_rem_runs_started ON rem_runs(started_at DESC);

CREATE TABLE IF NOT EXISTS rem_decisions (
    decision_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    cluster_idx INTEGER NOT NULL,
    relationship_assessment TEXT NOT NULL,
    action TEXT NOT NULL,
    members TEXT NOT NULL,
    existing_considered TEXT,
    result_memory_id TEXT,
    rationale TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES rem_runs(run_id)
);

CREATE INDEX IF NOT EXISTS idx_rem_decisions_run ON rem_decisions(run_id);
CREATE INDEX IF NOT EXISTS idx_rem_decisions_result ON rem_decisions(result_memory_id);
CREATE INDEX IF NOT EXISTS idx_rem_decisions_action ON rem_decisions(action);
CREATE INDEX IF NOT EXISTS idx_rem_decisions_created ON rem_decisions(created_at DESC);
