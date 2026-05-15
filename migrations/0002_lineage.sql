-- migrations/0002_lineage.sql — Consolidation lineage tracking for REM (CLA-87).
--
-- Tracks which episodic memories were distilled into which semantic
-- consolidations. The post-CLA-87 architecture is *additive* — episodics
-- are NEVER destroyed by consolidation. They remain in the memories
-- table, and lineage rows here document their relationship to the
-- consolidated semantic written above them.
--
-- This table is the data that gates REM's candidate-pair filter:
--   * skip pairs where either memory already appears as a `source_id`
--     (it's already been folded into some existing semantic)
--   * skip pairs where either memory is itself a `parent_id`
--     (don't try to consolidate an already-consolidated semantic
--      with one of its own siblings without explicit intent)
--
-- Without this guard, REM would re-consolidate heavily co-activated
-- pairs on every nightly run — the exact bug visible in the pre-CLA-87
-- corpus, where one count=5 pair produced four byte-identical semantics.

CREATE TABLE IF NOT EXISTS consolidation_lineage (
    parent_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (parent_id, source_id),
    FOREIGN KEY (parent_id) REFERENCES memories(id),
    FOREIGN KEY (source_id) REFERENCES memories(id)
);

-- Index for "is this memory already a source of any consolidation?" —
-- the hot path during REM's candidate-pair filter.
CREATE INDEX IF NOT EXISTS idx_lineage_source ON consolidation_lineage(source_id);

-- Index for "what semantics consolidate from this memory?" — useful for
-- recall-side lineage exposure if/when we surface it on the agent surface.
CREATE INDEX IF NOT EXISTS idx_lineage_parent ON consolidation_lineage(parent_id);
