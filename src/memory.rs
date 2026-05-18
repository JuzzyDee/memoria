// memory.rs — Universal type definitions for oneiro.
//
// Lives outside store.rs so the wasm32 worker side (worker_store.rs) and
// the native rusqlite side (store.rs) can share the same `Memory` and
// `MemoryType` types. No DB-specific dependencies live here — only
// chrono + serde, both wasm32-compatible.

use chrono::{DateTime, Utc};

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
    ///
    /// Episodic stability was raised from 1.0 to 7.0 in CLA-87 to counter
    /// the decay-asymmetry problem: with stability=1.0 a new memory not
    /// retrieved by semantic match in its first week dies before it had
    /// a chance to be load-bearing in a future context. At stability=7.0
    /// the same memory decays to ~0.37 at 7 days and ~0.014 at 30 days,
    /// widening the gauntlet from a week to a month — long enough for
    /// the conversation that would have naturally surfaced it to happen.
    pub fn base_stability(&self) -> f64 {
        match self {
            MemoryType::Episodic => 7.0,      // decays in ~30 days without reinforcement
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
    /// Embedding vector for semantic search (None if not yet embedded).
    /// On native this is loaded from the SQLite blob column; on wasm32
    /// (post-CLA-84) the vector lives in Vectorize and this field is
    /// typically None on Memory instances flowing through the worker.
    /// Skipped in serde because it's huge and reconstructable.
    #[serde(skip)]
    pub embedding: Option<Vec<f64>>,
    /// Optional: SHA-256 hex hash of the associated image file.
    /// On native the bytes live at {images_dir}/{hash}.{ext} (content-
    /// addressed storage); on the worker they live in R2 under the same
    /// key shape. Serialized so the migration path (CLA-84 phase 8) can
    /// round-trip image metadata.
    #[serde(default)]
    pub image_hash: Option<String>,
    /// Optional: MIME type of the image (e.g. "image/jpeg"). Determines
    /// file extension. Same serialization rationale as `image_hash`.
    #[serde(default)]
    pub image_mime: Option<String>,
    /// Optional: provenance — who or what recorded this memory.
    /// Server-controlled (set from auth context); any client-supplied value
    /// is ignored to prevent forgery.
    ///   "claude" — written by a Claude instance via OAuth
    ///   "rover"  — written by the rover via its service API key
    ///   None     — pre-CLA-86 legacy memory, or a local stdio write
    pub recorded_by: Option<String>,
}
