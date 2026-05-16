-- migrations/0004_cluster_decisions.sql — REM cluster decision cache (CLA-94).
--
-- Side effect of CLA-87's additive-consolidation design: clusters judged
-- "skip" by Haiku will keep co-activating (they're real memories that
-- recall together), get reclustered on the next REM run, get sent to
-- Haiku again, and get judged "skip" again — indefinitely. Each repeat
-- pass costs a Haiku call and adds a duplicate row to rem_decisions.
--
-- This table caches each cluster's decision keyed by its members. Before
-- calling Haiku, the consolidator checks for a cached decision; on cache
-- hit it bumps decision_count and dispatches the cached action without
-- a fresh API call.
--
-- Cluster identity:
--
--   cluster_hash = SHA-256(sorted member UUIDs, comma-joined)
--
-- Same UUID set = same cluster = decision persists. Cluster grows /
-- shrinks / loses-member → different hash → fresh Haiku call. Clusters
-- are immutable as a set; their judgments persist with the set. A member
-- being reframed (content changes) doesn't invalidate the decision —
-- the connection structure hasn't changed, only the content.
--
-- Query patterns this enables:
--
--   -- Which clusters are eating the most repeat-judgment cost?
--   SELECT cluster_hash, last_action, decision_count, members_json
--   FROM cluster_decisions
--   ORDER BY decision_count DESC LIMIT 20;
--
--   -- Did any cluster have a non-skip decision invalidated by a
--   -- subsequent forget?
--   SELECT cd.cluster_hash, cd.result_memory_id, cd.last_decided_at
--   FROM cluster_decisions cd
--   LEFT JOIN memories m ON m.id = cd.result_memory_id
--   WHERE cd.result_memory_id IS NOT NULL AND m.id IS NULL;

CREATE TABLE IF NOT EXISTS cluster_decisions (
    cluster_hash       TEXT PRIMARY KEY,
    members_json       TEXT NOT NULL,
    last_action        TEXT NOT NULL,
    result_memory_id   TEXT,
    first_decided_at   TEXT NOT NULL,
    last_decided_at    TEXT NOT NULL,
    decision_count     INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX IF NOT EXISTS idx_cluster_decisions_last_action ON cluster_decisions(last_action);
CREATE INDEX IF NOT EXISTS idx_cluster_decisions_decided_at ON cluster_decisions(last_decided_at DESC);
