// worker_store.rs — D1-backed memory store for the Cloudflare Worker.
//
// Mirrors a subset of store.rs's public API but uses Cloudflare D1
// instead of rusqlite. Lives only on wasm32; the native bins keep using
// store.rs unchanged during the migration.
//
// Embeddings DO NOT live in this table — Vectorize owns the vector
// index, keyed by memory_id. Recall is a two-step pattern:
//
//   1. Vectorize.query(embedding) → top-k memory_ids
//   2. SELECT … FROM memories WHERE id IN (?, ?, …)
//
// Phase 2b.1 (this commit) implements the minimum viable set:
//
//   create_memory_with_provenance — INSERT a memory row
//   get                            — SELECT one by id
//   recall_active                  — SELECT top-k by strength
//
// Embedding generation (Workers AI) and Vectorize integration land in
// CLA-84 phases 3 + 4. Subsequent commits add `find_neighbours`,
// `recall_semantic`, `touch`, `forget`, `consolidate`, `reframe`,
// `record_co_activation`, `apply_decay`, and the image-handling paths.

use crate::memory::{Memory, MemoryType};
use chrono::{DateTime, Utc};
use uuid::Uuid;
use worker::{D1Database, Result};

/// Row representation matching the D1 `memories` table schema. Serde
/// deserializes D1 rows into this; `into_memory()` converts to the
/// in-process `Memory` type the rest of the codebase uses. Kept
/// separate from `Memory` because:
///
///   * D1 stores timestamps as TEXT (RFC3339), `Memory.created_at`
///     is `DateTime<Utc>` — round trip via string.
///   * D1 stores tags as a JSON-encoded string, `Memory.tags` is
///     `Vec<String>` — round trip via serde_json.
///   * `Memory.embedding` doesn't exist in D1 (Vectorize owns vectors).
#[derive(Debug, serde::Deserialize)]
struct MemoryRow {
    id: String,
    memory_type: String,
    content: String,
    summary: String,
    created_at: String,
    last_accessed: String,
    access_count: u32,
    strength: f64,
    stability: f64,
    entity: Option<String>,
    tags: String,
    image_hash: Option<String>,
    image_mime: Option<String>,
    recorded_by: Option<String>,
}

impl MemoryRow {
    fn into_memory(self) -> Memory {
        let created_at = DateTime::parse_from_rfc3339(&self.created_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let last_accessed = DateTime::parse_from_rfc3339(&self.last_accessed)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let memory_type = MemoryType::from_str(&self.memory_type).unwrap_or(MemoryType::Episodic);
        let tags: Vec<String> = serde_json::from_str(&self.tags).unwrap_or_default();

        Memory {
            id: self.id,
            memory_type,
            content: self.content,
            summary: self.summary,
            created_at,
            last_accessed,
            access_count: self.access_count,
            strength: self.strength,
            stability: self.stability,
            entity: self.entity,
            tags,
            embedding: None, // Vectorize owns the vector — never populated from D1
            image_hash: self.image_hash,
            image_mime: self.image_mime,
            recorded_by: self.recorded_by,
        }
    }
}

/// INSERT a fresh memory row. Caller supplies provenance (`recorded_by`)
/// from the auth context — never trust a client-supplied value.
///
/// Returns the constructed `Memory` so callers can attach the embedding
/// to Vectorize using the same id (phase 4 wiring).
pub async fn create_memory_with_provenance(
    db: &D1Database,
    memory_type: MemoryType,
    content: String,
    summary: String,
    entity: Option<String>,
    tags: Vec<String>,
    recorded_by: Option<String>,
) -> Result<Memory> {
    let now = Utc::now();
    let id = Uuid::new_v4().to_string();
    let tags_json = serde_json::to_string(&tags).unwrap_or_else(|_| "[]".to_string());
    let stability = memory_type.base_stability();

    db.prepare(
        "INSERT INTO memories
            (id, memory_type, content, summary, created_at, last_accessed,
             access_count, strength, stability, entity, tags, image_hash,
             image_mime, recorded_by)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&[
        id.clone().into(),
        memory_type.as_str().into(),
        content.clone().into(),
        summary.clone().into(),
        now.to_rfc3339().into(),
        now.to_rfc3339().into(),
        0i32.into(),
        1.0_f64.into(),
        stability.into(),
        match &entity {
            Some(e) => e.clone().into(),
            None => worker::wasm_bindgen::JsValue::NULL,
        },
        tags_json.into(),
        worker::wasm_bindgen::JsValue::NULL, // image_hash — phase 2b.N adds image writes
        worker::wasm_bindgen::JsValue::NULL, // image_mime
        match &recorded_by {
            Some(r) => r.clone().into(),
            None => worker::wasm_bindgen::JsValue::NULL,
        },
    ])?
    .run()
    .await?;

    Ok(Memory {
        id,
        memory_type,
        content,
        summary,
        created_at: now,
        last_accessed: now,
        access_count: 0,
        strength: 1.0,
        stability,
        entity,
        tags,
        embedding: None,
        image_hash: None,
        image_mime: None,
        recorded_by,
    })
}

/// Fetch one memory by exact id. Returns `Ok(None)` when not found.
pub async fn get(db: &D1Database, id: &str) -> Result<Option<Memory>> {
    let row = db
        .prepare(
            "SELECT id, memory_type, content, summary, created_at, last_accessed,
                    access_count, strength, stability, entity, tags, image_hash,
                    image_mime, recorded_by
             FROM memories
             WHERE id = ?",
        )
        .bind(&[id.into()])?
        .first::<MemoryRow>(None)
        .await?;
    Ok(row.map(MemoryRow::into_memory))
}

/// Top-k active memories by strength. Filters out orientation memories
/// (those load via a separate `get_orientation` path — added in a later
/// phase) so they don't crowd the active set.
pub async fn recall_active(
    db: &D1Database,
    min_strength: f64,
    limit: usize,
) -> Result<Vec<Memory>> {
    let rows: Vec<MemoryRow> = db
        .prepare(
            "SELECT id, memory_type, content, summary, created_at, last_accessed,
                    access_count, strength, stability, entity, tags, image_hash,
                    image_mime, recorded_by
             FROM memories
             WHERE strength >= ?
               AND memory_type != 'orientation'
             ORDER BY strength DESC
             LIMIT ?",
        )
        .bind(&[min_strength.into(), (limit as u32).into()])?
        .all()
        .await?
        .results()?;
    Ok(rows.into_iter().map(MemoryRow::into_memory).collect())
}
