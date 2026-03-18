// embed.rs — Embedding generation via Ollama
//
// Uses nomic-embed-text running locally via Ollama to generate 768-dimension
// embeddings for semantic similarity search. The "search_document:" and
// "search_query:" prefixes are part of nomic's training — they improve
// retrieval quality by signalling intent.
//
// This module handles the HTTP call to Ollama and cosine similarity math.
// The store handles persistence. The MCP server handles orchestration.

use serde::{Deserialize, Serialize};

const OLLAMA_URL: &str = "http://localhost:11434/api/embeddings";
const MODEL: &str = "nomic-embed-text";

/// An embedding vector.
pub type Embedding = Vec<f64>;

#[derive(Serialize)]
struct OllamaRequest<'a> {
    model: &'a str,
    prompt: &'a str,
}

#[derive(Deserialize)]
struct OllamaResponse {
    embedding: Vec<f64>,
}

/// Generate an embedding for memory content (stored document).
/// Prefixes with "search_document:" per nomic's training protocol.
pub fn embed_document(text: &str) -> Result<Embedding, String> {
    let prompt = format!("search_document: {}", text);
    call_ollama(&prompt)
}

/// Generate an embedding for a recall query (search context).
/// Prefixes with "search_query:" per nomic's training protocol.
pub fn embed_query(text: &str) -> Result<Embedding, String> {
    let prompt = format!("search_query: {}", text);
    call_ollama(&prompt)
}

/// Call Ollama's embedding API synchronously.
fn call_ollama(prompt: &str) -> Result<Embedding, String> {
    let request = OllamaRequest {
        model: MODEL,
        prompt,
    };

    // Use ureq for synchronous HTTP — we don't need async for a local API call
    let response: OllamaResponse = ureq::post(OLLAMA_URL)
        .send_json(&request)
        .map_err(|e| format!("Ollama request failed: {}. Is Ollama running?", e))?
        .body_mut()
        .read_json()
        .map_err(|e| format!("Failed to parse Ollama response: {}", e))?;

    Ok(response.embedding)
}

/// Cosine similarity between two embedding vectors.
/// Returns a value between -1.0 (opposite) and 1.0 (identical).
pub fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0_f64;
    let mut norm_a = 0.0_f64;
    let mut norm_b = 0.0_f64;

    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < 1e-10 { 0.0 } else { dot / denom }
}

/// Serialize an embedding to a compact binary blob for SQLite storage.
/// Uses f32 to halve storage (768 * 4 = 3KB per memory instead of 6KB).
pub fn embedding_to_bytes(embedding: &[f64]) -> Vec<u8> {
    embedding
        .iter()
        .flat_map(|&v| (v as f32).to_le_bytes())
        .collect()
}

/// Deserialize an embedding from the binary blob format.
pub fn embedding_from_bytes(bytes: &[u8]) -> Embedding {
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            let arr: [u8; 4] = chunk.try_into().unwrap();
            f32::from_le_bytes(arr) as f64
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![-1.0, -2.0, -3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_embedding_roundtrip() {
        let original = vec![0.5, -1.2, 3.14159, 0.0, -0.001];
        let bytes = embedding_to_bytes(&original);
        let recovered = embedding_from_bytes(&bytes);

        // f32 precision — not exact but close
        assert_eq!(original.len(), recovered.len());
        for (a, b) in original.iter().zip(recovered.iter()) {
            assert!((a - b).abs() < 0.001, "Expected {}, got {}", a, b);
        }
    }

    #[test]
    fn test_cosine_similarity_empty() {
        let sim = cosine_similarity(&[], &[]);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn test_cosine_similarity_mismatched() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }
}
