// lib.rs — Memoria as a Cloudflare Worker (CLA-84).
//
// The entire library is gated on wasm32 — the native bins (src/main.rs,
// src/rem.rs) ignore the library content and use their existing modules
// directly. Once the port is complete and tested, main.rs is deleted
// and rem.rs is rewritten to talk to the deployed Worker over HTTP.
//
// Phase 1-2 of CLA-84 set up the shell; subsequent phases port store → D1,
// embed → Workers AI, auth → KV/DO, and finally the HTTP layer →
// workers-rs Router.

#![cfg(target_family = "wasm")]

// Universal types — shared with the native bins via the same source files.
mod embed;
mod memory;

// Worker-side modules (wasm32-only).
mod worker_embed;
mod worker_store;

use crate::memory::MemoryType;
use worker::{event, Context, Env, Request, Response, Result};

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let path = req.url()?.path().to_string();
    match path.as_str() {
        // Smoke-test endpoints for CLA-84 phase 2b.1 — exercise the D1
        // binding from a deployed Worker. UNAUTHENTICATED on purpose for
        // the smoke test; phase 6 replaces these with proper MCP routing
        // gated by the OAuth + service-key bearer check from CLA-86.
        "/test/create" => create_test_memory(&env).await,
        "/test/list" => list_test_memories(&env).await,
        "/test/embed" => test_embed(&env, &req).await,
        _ => Response::ok("memoria — Cloudflare migration in progress (CLA-84)"),
    }
}

async fn create_test_memory(env: &Env) -> Result<Response> {
    let db = env.d1("DB")?;
    let memory = worker_store::create_memory_with_provenance(
        &db,
        MemoryType::Episodic,
        "Smoke test: a memory written by the deployed Worker via D1.".into(),
        "CLA-84 phase 2b.1 smoke test".into(),
        None,
        vec!["cla-84".into(), "smoke-test".into()],
        Some("claude".to_string()),
    )
    .await?;
    Response::from_json(&memory)
}

async fn list_test_memories(env: &Env) -> Result<Response> {
    let db = env.d1("DB")?;
    let memories = worker_store::recall_active(&db, 0.0, 10).await?;
    Response::from_json(&memories)
}

/// Workers AI smoke test. `?q=<text>` to override the default phrase.
/// Returns the model name, the vector length, and the first 8 components
/// (full 768-dim vectors are noisy; the prefix is enough to eyeball that
/// real numbers came back).
async fn test_embed(env: &Env, req: &Request) -> Result<Response> {
    let text = req
        .url()?
        .query_pairs()
        .find(|(k, _)| k == "q")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| "Chopper barked at a kookaburra".to_string());

    let embedding = worker_embed::embed_document(env, &text).await?;

    Response::from_json(&serde_json::json!({
        "model": "@cf/baai/bge-base-en-v1.5",
        "text": text,
        "dimensions": embedding.len(),
        "preview_first_8": embedding.iter().take(8).collect::<Vec<_>>(),
    }))
}
