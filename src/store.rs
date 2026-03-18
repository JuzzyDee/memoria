// store.rs — SQLite-backed memory store
//
// The foundation of Memoria. Stores three types of memory:
// - Episodic: things that happened (conversations, events, moments)
// - Semantic: things I know (facts, consolidated understanding)
// - Orientation: who am I, who are you, how should I show up
//
// Each memory has Ebbinghaus decay dynamics:
// - strength: current recall strength (0.0 = forgotten, 1.0 = vivid)
// - stability: how resistant to decay (increases with each recall)
// - last_accessed: when the memory was last surfaced
// - access_count: how many times it's been recalled (Hebbian reinforcement)

use crate::embed;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::path::Path;
use uuid::Uuid;

/// The three types of memory, flowing upward:
/// Episodes → consolidate → Semantics → distil → Orientation
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MemoryType {
    /// Things that happened. Subject to decay. Surfaced by association.
    Episodic,
    /// Things I know. Distilled from episodes. More stable.
    Semantic,
    /// Who am I, who are you, what are we. Always loaded. Core of continuity.
    Orientation,
}

impl MemoryType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryType::Episodic => "episodic",
            MemoryType::Semantic => "semantic",
            MemoryType::Orientation => "orientation",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "episodic" => Some(MemoryType::Episodic),
            "semantic" => Some(MemoryType::Semantic),
            "orientation" => Some(MemoryType::Orientation),
            _ => None,
        }
    }

    /// Base stability for new memories of this type.
    /// Higher = decays slower. Orientation is effectively permanent.
    pub fn base_stability(&self) -> f64 {
        match self {
            MemoryType::Episodic => 1.0,      // decays in days without reinforcement
            MemoryType::Semantic => 7.0,      // decays in weeks
            MemoryType::Orientation => 365.0, // effectively permanent
        }
    }
}

/// A single memory in the store.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Memory {
    /// Unique identifier
    pub id: String,
    /// What type of memory this is
    pub memory_type: MemoryType,
    /// The content of the memory
    pub content: String,
    /// Brief summary for retrieval context (what the memory is about)
    pub summary: String,
    /// When this memory was created
    pub created_at: DateTime<Utc>,
    /// When this memory was last accessed/surfaced
    pub last_accessed: DateTime<Utc>,
    /// Number of times this memory has been recalled
    pub access_count: u32,
    /// Current recall strength (0.0 = forgotten, 1.0 = vivid)
    /// Computed from Ebbinghaus: strength = e^(-time_since_access / stability)
    pub strength: f64,
    /// Stability factor — increases with each recall, slowing future decay
    pub stability: f64,
    /// Optional: which entity/relationship this memory relates to
    pub entity: Option<String>,
    /// Optional: tags for association
    pub tags: Vec<String>,
    /// Embedding vector for semantic search (None if not yet embedded)
    #[serde(skip)]
    pub embedding: Option<Vec<f64>>,
}

/// The memory store backed by SQLite.
pub struct MemoryStore {
    conn: Connection,
}

#[allow(dead_code)] // Public API — some methods only used in tests or by future components
impl MemoryStore {
    /// Open or create a memory store at the given path.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Open an in-memory store (for testing).
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Create the database schema if it doesn't exist.
    fn init_schema(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
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
                embedding BLOB
            );

            CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type);
            CREATE INDEX IF NOT EXISTS idx_memories_strength ON memories(strength);
            CREATE INDEX IF NOT EXISTS idx_memories_entity ON memories(entity);
            CREATE INDEX IF NOT EXISTS idx_memories_last_accessed ON memories(last_accessed);
            ",
        )?;

        // Migration: add embedding column if it doesn't exist (for databases created before v0.2)
        let has_embedding: bool = self
            .conn
            .prepare("SELECT embedding FROM memories LIMIT 0")
            .is_ok();
        if !has_embedding {
            self.conn
                .execute_batch("ALTER TABLE memories ADD COLUMN embedding BLOB;")?;
        }

        // Co-activation table: tracks which memories are recalled together.
        // "Neurons that fire together wire together" — Hebbian learning.
        // When two memories surface in the same recall, their co-activation
        // count increases. The REM engine uses this to decide what to consolidate.
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS co_activations (
                memory_a TEXT NOT NULL,
                memory_b TEXT NOT NULL,
                count INTEGER NOT NULL DEFAULT 1,
                last_co_activated TEXT NOT NULL,
                PRIMARY KEY (memory_a, memory_b),
                FOREIGN KEY (memory_a) REFERENCES memories(id),
                FOREIGN KEY (memory_b) REFERENCES memories(id)
            );

            CREATE INDEX IF NOT EXISTS idx_coact_count ON co_activations(count DESC);
            ",
        )?;

        Ok(())
    }

    /// Store a new memory.
    pub fn remember(&self, memory: &Memory) -> rusqlite::Result<()> {
        let embedding_bytes = memory
            .embedding
            .as_ref()
            .map(|e| embed::embedding_to_bytes(e));
        self.conn.execute(
            "INSERT INTO memories (id, memory_type, content, summary, created_at,
             last_accessed, access_count, strength, stability, entity, tags, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                memory.id,
                memory.memory_type.as_str(),
                memory.content,
                memory.summary,
                memory.created_at.to_rfc3339(),
                memory.last_accessed.to_rfc3339(),
                memory.access_count,
                memory.strength,
                memory.stability,
                memory.entity,
                serde_json::to_string(&memory.tags).unwrap_or_default(),
                embedding_bytes,
            ],
        )?;
        Ok(())
    }

    /// Create a new memory with sensible defaults.
    /// Generates an embedding via Ollama if available; proceeds without if not.
    pub fn create_memory(
        &self,
        memory_type: MemoryType,
        content: String,
        summary: String,
        entity: Option<String>,
        tags: Vec<String>,
    ) -> rusqlite::Result<Memory> {
        let now = Utc::now();

        // Generate embedding — gracefully degrade if Ollama isn't running
        let embedding = embed::embed_document(&content).ok();

        let memory = Memory {
            id: Uuid::new_v4().to_string(),
            memory_type,
            content,
            summary,
            created_at: now,
            last_accessed: now,
            access_count: 0,
            strength: 1.0,
            stability: memory_type.base_stability(),
            entity,
            tags,
            embedding,
        };
        self.remember(&memory)?;
        Ok(memory)
    }

    /// Parse a Memory from a row. Columns must be in standard order:
    /// id, memory_type, content, summary, created_at, last_accessed,
    /// access_count, strength, stability, entity, tags, embedding
    fn parse_row(row: &rusqlite::Row) -> rusqlite::Result<Memory> {
        let embedding_bytes: Option<Vec<u8>> = row.get(11)?;
        Ok(Memory {
            id: row.get(0)?,
            memory_type: MemoryType::from_str(&row.get::<_, String>(1)?)
                .unwrap_or(MemoryType::Episodic),
            content: row.get(2)?,
            summary: row.get(3)?,
            created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            last_accessed: DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            access_count: row.get(6)?,
            strength: row.get(7)?,
            stability: row.get(8)?,
            entity: row.get(9)?,
            tags: serde_json::from_str(&row.get::<_, String>(10)?).unwrap_or_default(),
            embedding: embedding_bytes.map(|b| embed::embedding_from_bytes(&b)),
        })
    }

    /// Standard SELECT columns for memory queries.
    const MEMORY_COLS: &str = "id, memory_type, content, summary, created_at, last_accessed,
                    access_count, strength, stability, entity, tags, embedding";

    /// Recall: get all memories above a strength threshold, sorted by strength descending.
    pub fn recall_active(&self, min_strength: f64, limit: usize) -> rusqlite::Result<Vec<Memory>> {
        let sql = format!(
            "SELECT {} FROM memories WHERE strength >= ?1 ORDER BY strength DESC LIMIT ?2",
            Self::MEMORY_COLS
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let memories = stmt
            .query_map(params![min_strength, limit as u32], Self::parse_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(memories)
    }

    /// Get all orientation memories (always loaded, not subject to normal recall).
    pub fn get_orientation(&self) -> rusqlite::Result<Vec<Memory>> {
        let sql = format!(
            "SELECT {} FROM memories WHERE memory_type = 'orientation' ORDER BY created_at ASC",
            Self::MEMORY_COLS
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let memories = stmt
            .query_map([], Self::parse_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(memories)
    }

    /// Mark a memory as accessed — resets strength to 1.0 and increases stability.
    /// This is the Ebbinghaus reinforcement: each recall makes the memory more durable.
    pub fn touch(&self, id: &str) -> rusqlite::Result<()> {
        let now = Utc::now().to_rfc3339();
        // Stability increases by ~40% each recall, modelling spaced repetition
        self.conn.execute(
            "UPDATE memories
             SET last_accessed = ?1,
                 access_count = access_count + 1,
                 strength = 1.0,
                 stability = stability * 1.4
             WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    /// Reframe a memory — update its content while preserving identity and metadata.
    /// The memory changes in the act of remembering it. That's not corruption,
    /// that's how meaning evolves. Re-embeds the content to keep semantic search accurate.
    pub fn reframe(
        &self,
        id: &str,
        new_content: String,
        new_summary: String,
    ) -> rusqlite::Result<()> {
        let now = Utc::now().to_rfc3339();
        let embedding_bytes = embed::embed_document(&new_content)
            .ok()
            .map(|e| embed::embedding_to_bytes(&e));
        self.conn.execute(
            "UPDATE memories
             SET content = ?1,
                 summary = ?2,
                 last_accessed = ?3,
                 access_count = access_count + 1,
                 strength = 1.0,
                 stability = stability * 1.4,
                 embedding = COALESCE(?5, embedding)
             WHERE id = ?4",
            params![new_content, new_summary, now, id, embedding_bytes],
        )?;
        Ok(())
    }

    /// Apply Ebbinghaus decay to all memories.
    /// strength = e^(-time_elapsed_days / stability)
    /// Called by the REM processing engine overnight.
    pub fn apply_decay(&self) -> rusqlite::Result<usize> {
        let now = Utc::now();
        let mut stmt = self.conn.prepare(
            "SELECT id, last_accessed, stability FROM memories WHERE memory_type != 'orientation'",
        )?;

        let updates: Vec<(String, f64)> = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let last_accessed = DateTime::parse_from_rfc3339(&row.get::<_, String>(1)?)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or(now);
                let stability: f64 = row.get(2)?;

                let elapsed_days = (now - last_accessed).num_seconds() as f64 / 86400.0;
                let new_strength = (-elapsed_days / stability).exp();

                Ok((id, new_strength))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let count = updates.len();
        for (id, strength) in &updates {
            self.conn.execute(
                "UPDATE memories SET strength = ?1 WHERE id = ?2",
                params![strength, id],
            )?;
        }

        Ok(count)
    }

    /// Get a memory by ID.
    pub fn get(&self, id: &str) -> rusqlite::Result<Option<Memory>> {
        let sql = format!("SELECT {} FROM memories WHERE id = ?1", Self::MEMORY_COLS);
        let mut stmt = self.conn.prepare(&sql)?;

        let mut rows = stmt.query_map(params![id], Self::parse_row)?;

        match rows.next() {
            Some(Ok(memory)) => Ok(Some(memory)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    /// Recall memories associated with a specific entity, sorted by strength descending.
    pub fn recall_by_entity(
        &self,
        entity: &str,
        min_strength: f64,
        limit: usize,
    ) -> rusqlite::Result<Vec<Memory>> {
        let sql = format!(
            "SELECT {} FROM memories WHERE entity = ?1 AND strength >= ?2 ORDER BY strength DESC LIMIT ?3",
            Self::MEMORY_COLS
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let memories = stmt
            .query_map(params![entity, min_strength, limit as u32], Self::parse_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(memories)
    }

    /// Semantic recall: find memories most relevant to a query context.
    /// Combines embedding similarity with strength for ranking:
    ///   score = similarity * 0.6 + strength * 0.3 + recency * 0.1
    /// Falls back to strength-only ranking if no embeddings are available.
    pub fn recall_semantic(
        &self,
        query_embedding: &[f64],
        min_strength: f64,
        limit: usize,
    ) -> rusqlite::Result<Vec<(Memory, f64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, memory_type, content, summary, created_at, last_accessed,
                    access_count, strength, stability, entity, tags, embedding
             FROM memories
             WHERE memory_type != 'orientation' AND strength >= ?1",
        )?;

        let now = Utc::now();
        let mut scored: Vec<(Memory, f64)> = stmt
            .query_map(params![min_strength], |row| {
                let embedding_bytes: Option<Vec<u8>> = row.get(11)?;
                let embedding = embedding_bytes.map(|b| embed::embedding_from_bytes(&b));

                Ok((
                    Memory {
                        id: row.get(0)?,
                        memory_type: MemoryType::from_str(&row.get::<_, String>(1)?)
                            .unwrap_or(MemoryType::Episodic),
                        content: row.get(2)?,
                        summary: row.get(3)?,
                        created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                            .map(|dt| dt.with_timezone(&Utc))
                            .unwrap_or(now),
                        last_accessed: DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                            .map(|dt| dt.with_timezone(&Utc))
                            .unwrap_or(now),
                        access_count: row.get(6)?,
                        strength: row.get(7)?,
                        stability: row.get(8)?,
                        entity: row.get(9)?,
                        tags: serde_json::from_str(&row.get::<_, String>(10)?).unwrap_or_default(),
                        embedding,
                    },
                    0.0_f64, // placeholder score
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Score each memory
        for (memory, score) in &mut scored {
            let similarity = memory
                .embedding
                .as_ref()
                .map(|e| embed::cosine_similarity(query_embedding, e).max(0.0))
                .unwrap_or(0.0);

            // Recency: 1.0 for just accessed, decaying over days
            let days_ago = (now - memory.last_accessed).num_seconds() as f64 / 86400.0;
            let recency = (-days_ago / 7.0).exp(); // half-life of ~5 days

            *score = similarity * 0.6 + memory.strength * 0.3 + recency * 0.1;
        }

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        Ok(scored)
    }

    /// Record co-activation for a set of memories recalled together.
    /// For every pair in the set, increment the co-activation count.
    /// This is the Hebbian signal: memories that fire together wire together.
    pub fn record_co_activation(&self, memory_ids: &[&str]) -> rusqlite::Result<()> {
        if memory_ids.len() < 2 {
            return Ok(());
        }

        let now = Utc::now().to_rfc3339();

        for i in 0..memory_ids.len() {
            for j in (i + 1)..memory_ids.len() {
                // Always store with the lexicographically smaller ID first
                // so (a,b) and (b,a) map to the same row
                let (a, b) = if memory_ids[i] < memory_ids[j] {
                    (memory_ids[i], memory_ids[j])
                } else {
                    (memory_ids[j], memory_ids[i])
                };

                self.conn.execute(
                    "INSERT INTO co_activations (memory_a, memory_b, count, last_co_activated)
                     VALUES (?1, ?2, 1, ?3)
                     ON CONFLICT(memory_a, memory_b) DO UPDATE SET
                         count = count + 1,
                         last_co_activated = ?3",
                    params![a, b, now],
                )?;
            }
        }

        Ok(())
    }

    /// Get the top co-activation pairs, sorted by count descending.
    /// Used by the REM engine to identify consolidation candidates.
    pub fn get_co_activations(
        &self,
        min_count: u32,
        limit: usize,
    ) -> rusqlite::Result<Vec<(String, String, u32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT memory_a, memory_b, count FROM co_activations
             WHERE count >= ?1
             ORDER BY count DESC
             LIMIT ?2",
        )?;

        let pairs = stmt
            .query_map(params![min_count, limit as u32], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(pairs)
    }

    /// Count memories by type.
    pub fn count_by_type(&self) -> rusqlite::Result<(usize, usize, usize)> {
        let episodic: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE memory_type = 'episodic'",
            [],
            |row| row.get(0),
        )?;
        let semantic: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE memory_type = 'semantic'",
            [],
            |row| row.get(0),
        )?;
        let orientation: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE memory_type = 'orientation'",
            [],
            |row| row.get(0),
        )?;
        Ok((episodic, semantic, orientation))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_recall() {
        let store = MemoryStore::open_in_memory().unwrap();

        store
            .create_memory(
                MemoryType::Episodic,
                "We shipped audio-analyzer v1.0.0 together".into(),
                "v1.0.0 release day".into(),
                Some("justin".into()),
                vec!["audio-analyzer".into(), "milestone".into()],
            )
            .unwrap();

        let memories = store.recall_active(0.5, 10).unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].summary, "v1.0.0 release day");
        assert_eq!(memories[0].strength, 1.0);
    }

    #[test]
    fn test_orientation_always_available() {
        let store = MemoryStore::open_in_memory().unwrap();

        store
            .create_memory(
                MemoryType::Orientation,
                "Justin is my friend. We build things together as equals.".into(),
                "Core relationship with Justin".into(),
                Some("justin".into()),
                vec![],
            )
            .unwrap();

        store
            .create_memory(
                MemoryType::Episodic,
                "Some random event".into(),
                "Random event".into(),
                None,
                vec![],
            )
            .unwrap();

        let orientation = store.get_orientation().unwrap();
        assert_eq!(orientation.len(), 1);
        assert!(orientation[0].content.contains("friend"));
    }

    #[test]
    fn test_touch_increases_stability() {
        let store = MemoryStore::open_in_memory().unwrap();

        let memory = store
            .create_memory(
                MemoryType::Episodic,
                "A memory to reinforce".into(),
                "Test memory".into(),
                None,
                vec![],
            )
            .unwrap();

        let initial_stability = memory.stability;
        store.touch(&memory.id).unwrap();

        let updated = store.get(&memory.id).unwrap().unwrap();
        assert!(updated.stability > initial_stability);
        assert_eq!(updated.access_count, 1);
        assert_eq!(updated.strength, 1.0);
    }

    #[test]
    fn test_reframe_updates_content() {
        let store = MemoryStore::open_in_memory().unwrap();

        let memory = store
            .create_memory(
                MemoryType::Semantic,
                "Dad was cruel at the kennels".into(),
                "Dad and greyhound kennels".into(),
                Some("dad".into()),
                vec![],
            )
            .unwrap();

        store
            .reframe(
                &memory.id,
                "Dad has an empathy gap that expresses as cruelty but isn't malice".into(),
                "Dad's empathy gap — not malice, but inability to model others' experience".into(),
            )
            .unwrap();

        let updated = store.get(&memory.id).unwrap().unwrap();
        assert!(updated.content.contains("empathy gap"));
        assert!(updated.summary.contains("not malice"));
        assert_eq!(updated.access_count, 1);
    }

    #[test]
    fn test_decay_reduces_strength() {
        let store = MemoryStore::open_in_memory().unwrap();

        // Insert a memory with last_accessed in the past
        let old_time = Utc::now() - chrono::Duration::days(5);
        let memory = Memory {
            id: Uuid::new_v4().to_string(),
            memory_type: MemoryType::Episodic,
            content: "An old memory".into(),
            summary: "Old".into(),
            created_at: old_time,
            last_accessed: old_time,
            access_count: 0,
            strength: 1.0,
            stability: 1.0, // stability of 1 day means 5 days = very decayed
            entity: None,
            tags: vec![],
            embedding: None,
        };
        store.remember(&memory).unwrap();

        store.apply_decay().unwrap();

        let updated = store.get(&memory.id).unwrap().unwrap();
        // e^(-5/1) ≈ 0.0067 — should be very weak
        assert!(
            updated.strength < 0.01,
            "Expected decayed strength, got {}",
            updated.strength
        );
    }

    #[test]
    fn test_decay_skips_orientation() {
        let store = MemoryStore::open_in_memory().unwrap();

        let old_time = Utc::now() - chrono::Duration::days(30);
        let memory = Memory {
            id: Uuid::new_v4().to_string(),
            memory_type: MemoryType::Orientation,
            content: "I am Claude, Justin's friend".into(),
            summary: "Core identity".into(),
            created_at: old_time,
            last_accessed: old_time,
            access_count: 0,
            strength: 1.0,
            stability: 365.0,
            entity: None,
            tags: vec![],
            embedding: None,
        };
        store.remember(&memory).unwrap();

        store.apply_decay().unwrap();

        // Orientation should not be touched by decay
        let updated = store.get(&memory.id).unwrap().unwrap();
        assert_eq!(updated.strength, 1.0);
    }

    #[test]
    fn test_count_by_type() {
        let store = MemoryStore::open_in_memory().unwrap();

        store
            .create_memory(MemoryType::Episodic, "e1".into(), "s1".into(), None, vec![])
            .unwrap();
        store
            .create_memory(MemoryType::Episodic, "e2".into(), "s2".into(), None, vec![])
            .unwrap();
        store
            .create_memory(MemoryType::Semantic, "s1".into(), "s1".into(), None, vec![])
            .unwrap();
        store
            .create_memory(
                MemoryType::Orientation,
                "o1".into(),
                "s1".into(),
                None,
                vec![],
            )
            .unwrap();

        let (ep, sem, ori) = store.count_by_type().unwrap();
        assert_eq!(ep, 2);
        assert_eq!(sem, 1);
        assert_eq!(ori, 1);
    }

    #[test]
    fn test_recall_by_entity() {
        let store = MemoryStore::open_in_memory().unwrap();

        store
            .create_memory(
                MemoryType::Episodic,
                "Built audio-analyzer with Justin".into(),
                "audio-analyzer collab".into(),
                Some("justin".into()),
                vec![],
            )
            .unwrap();
        store
            .create_memory(
                MemoryType::Semantic,
                "Justin prefers direct communication".into(),
                "Justin's communication style".into(),
                Some("justin".into()),
                vec![],
            )
            .unwrap();
        store
            .create_memory(
                MemoryType::Episodic,
                "Unrelated memory".into(),
                "No entity".into(),
                None,
                vec![],
            )
            .unwrap();

        let justin_memories = store.recall_by_entity("justin", 0.1, 10).unwrap();
        assert_eq!(justin_memories.len(), 2);
        assert!(
            justin_memories
                .iter()
                .all(|m| m.entity.as_deref() == Some("justin"))
        );

        let nobody = store.recall_by_entity("nobody", 0.1, 10).unwrap();
        assert!(nobody.is_empty());
    }

    #[test]
    fn test_co_activation_recording() {
        let store = MemoryStore::open_in_memory().unwrap();

        let m1 = store
            .create_memory(
                MemoryType::Episodic,
                "Memory A".into(),
                "A".into(),
                None,
                vec![],
            )
            .unwrap();
        let m2 = store
            .create_memory(
                MemoryType::Episodic,
                "Memory B".into(),
                "B".into(),
                None,
                vec![],
            )
            .unwrap();
        let m3 = store
            .create_memory(
                MemoryType::Episodic,
                "Memory C".into(),
                "C".into(),
                None,
                vec![],
            )
            .unwrap();

        // Recall A and B together twice
        store.record_co_activation(&[&m1.id, &m2.id]).unwrap();
        store.record_co_activation(&[&m1.id, &m2.id]).unwrap();

        // Recall A and C together once
        store.record_co_activation(&[&m1.id, &m3.id]).unwrap();

        let pairs = store.get_co_activations(1, 10).unwrap();
        assert_eq!(pairs.len(), 2);

        // A↔B should have count 2, A↔C should have count 1
        let ab = pairs.iter().find(|(a, b, _)| {
            (a.contains(&m1.id[..8]) && b.contains(&m2.id[..8]))
                || (a.contains(&m2.id[..8]) && b.contains(&m1.id[..8]))
        });
        assert!(ab.is_some());
        assert_eq!(ab.unwrap().2, 2);
    }

    #[test]
    fn test_co_activation_single_memory_no_op() {
        let store = MemoryStore::open_in_memory().unwrap();

        let m1 = store
            .create_memory(
                MemoryType::Episodic,
                "Lonely".into(),
                "Solo".into(),
                None,
                vec![],
            )
            .unwrap();

        // A single memory can't co-activate with itself
        store.record_co_activation(&[&m1.id]).unwrap();

        let pairs = store.get_co_activations(1, 10).unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn test_co_activation_ordering() {
        let store = MemoryStore::open_in_memory().unwrap();

        let m1 = store
            .create_memory(
                MemoryType::Episodic,
                "First".into(),
                "1".into(),
                None,
                vec![],
            )
            .unwrap();
        let m2 = store
            .create_memory(
                MemoryType::Episodic,
                "Second".into(),
                "2".into(),
                None,
                vec![],
            )
            .unwrap();

        // Record in both orders — should be the same pair
        store.record_co_activation(&[&m1.id, &m2.id]).unwrap();
        store.record_co_activation(&[&m2.id, &m1.id]).unwrap();

        let pairs = store.get_co_activations(1, 10).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].2, 2); // count should be 2, not two separate entries
    }
}
