// worker_embed.rs — Embedding generation via Workers AI for the wasm32
// worker side of oneiro. Replaces the native Ollama path (embed.rs) once
// the worker takes over MCP traffic.
//
// Model: `@cf/baai/bge-base-en-v1.5` — 768-dim, cosine-metric matching
// what the existing oneiro/Vectorize plan expects. Unlike nomic-embed-text
// (used by the native embed.rs), bge-base does NOT need the
// `search_document:` / `search_query:` prefix convention — the model
// handles both roles symmetrically.
//
// Both `embed_document` and `embed_query` produce the same shape of vector
// from bge-base; they're kept as separate functions for API symmetry with
// the native side and so that future model swaps (e.g. asymmetric models
// that DO need role prefixes) only need to change one of the two.

use crate::embed::Embedding;
use serde::{Deserialize, Serialize};
use worker::{Env, Result};

const MODEL: &str = "@cf/baai/bge-base-en-v1.5";
const AI_BINDING: &str = "AI";

#[derive(Serialize)]
struct EmbedInput<'a> {
    text: Vec<&'a str>,
}

#[derive(Deserialize)]
struct EmbedOutput {
    /// Cloudflare returns embeddings as `data: [[f32; 768]]` — outer array
    /// is one entry per input string, inner array is the vector itself.
    /// We deserialize as f64 for compatibility with the rest of oneiro's
    /// math (cosine_similarity, embedding_to_bytes both use f64).
    data: Vec<Vec<f64>>,
}

/// Generate an embedding for a stored memory's content.
pub async fn embed_document(env: &Env, text: &str) -> Result<Embedding> {
    embed_via_ai(env, text).await
}

/// Generate an embedding for a recall query.
pub async fn embed_query(env: &Env, text: &str) -> Result<Embedding> {
    embed_via_ai(env, text).await
}

async fn embed_via_ai(env: &Env, text: &str) -> Result<Embedding> {
    let ai = env.ai(AI_BINDING)?;
    let input = EmbedInput { text: vec![text] };
    let output: EmbedOutput = ai.run(MODEL, input).await?;
    Ok(output.data.into_iter().next().unwrap_or_default())
}
