-- migrations/0001_initial.sql — Initial D1 schema for oneiro (CLA-84).
--
-- Derived from the existing SQLite schema in src/store.rs:138-229,
-- consolidated and adapted for D1. Differences from the local schema:
--
--   * No `embedding BLOB` column on memories. Vectors live in Vectorize
--     now, keyed by memory_id. SELECT-then-JOIN pattern: query Vectorize
--     for top-k memory_ids, then SELECT FROM memories WHERE id IN (...).
--
--   * Includes the CLA-86 phase 4 `recorded_by` column and the
--     phase 6 `api_key_audit` table — both are part of the canonical
--     schema we're migrating to.
--
--   * Keeps the `tombstones` table for now. The sync layer is retired
--     post-migration, but the code paths that write tombstones (forget)
--     are still wired up. Drop in a later migration once `forget` is
--     refactored.
--
-- Apply with: `wrangler d1 migrations apply oneiro-db`.

CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    memory_type TEXT NOT NULL,
    content TEXT NOT NULL,
    summary TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_accessed TEXT NOT NULL,
    access_count INTEGER NOT NULL DEFAULT 0,
    strength REAL NOT NULL DEFAULT 1.0,
    stability REAL NOT NULL DEFAULT 1.0,
    entity TEXT,
    tags TEXT NOT NULL DEFAULT '[]',
    image_hash TEXT,
    image_mime TEXT,
    recorded_by TEXT
);

CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type);
CREATE INDEX IF NOT EXISTS idx_memories_strength ON memories(strength);
CREATE INDEX IF NOT EXISTS idx_memories_entity ON memories(entity);
CREATE INDEX IF NOT EXISTS idx_memories_last_accessed ON memories(last_accessed);
CREATE INDEX IF NOT EXISTS idx_memories_recorded_by ON memories(recorded_by);

-- Co-activation table — Hebbian "fire together / wire together" tracking.
-- Surfaced together in recall → bond strengthens. The REM consolidator
-- uses high co-activation counts to pick candidates for synthesis.
CREATE TABLE IF NOT EXISTS co_activations (
    memory_a TEXT NOT NULL,
    memory_b TEXT NOT NULL,
    count INTEGER NOT NULL DEFAULT 1,
    last_co_activated TEXT NOT NULL,
    PRIMARY KEY (memory_a, memory_b),
    FOREIGN KEY (memory_a) REFERENCES memories(id),
    FOREIGN KEY (memory_b) REFERENCES memories(id)
);

CREATE INDEX IF NOT EXISTS idx_coact_count ON co_activations(count DESC);

-- Tombstones — kept for now (forget still writes them). Phase out in a
-- later migration once the sync-era assumptions are stripped from the
-- forget code path.
CREATE TABLE IF NOT EXISTS tombstones (
    memory_id TEXT PRIMARY KEY,
    forgotten_at TEXT NOT NULL
);

-- CLA-86 audit log — every API-key-authenticated tool invocation writes
-- one row. Indexed for fast forensic queries (recent events, per-key).
CREATE TABLE IF NOT EXISTS api_key_audit (
    timestamp INTEGER NOT NULL,
    key_id TEXT NOT NULL,
    role TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    success INTEGER NOT NULL,
    error_kind TEXT
);

CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON api_key_audit(timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_key_id ON api_key_audit(key_id);
