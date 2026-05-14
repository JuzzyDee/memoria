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
mod api_key;
mod audit;
mod embed;
mod key_rate;
mod memory;

// Worker-side modules (wasm32-only).
mod worker_audit;
mod worker_auth_ctx;
mod worker_embed;
mod worker_mcp;
mod worker_store;
mod worker_vectorize;

use crate::memory::MemoryType;
use worker::{event, Context, Env, Request, Response, Result};

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let path = req.url()?.path().to_string();
    let method = req.method();
    match (method.clone(), path.as_str()) {
        // Real MCP endpoint — authenticated, scope-gated, audited.
        (worker::Method::Post, "/mcp") => mcp_endpoint(&env, &mut req).await,
        // Smoke-test endpoints from earlier phases. UNAUTHENTICATED on
        // purpose for diagnostics; remove once /mcp covers the surface.
        (_, "/test/create") => create_test_memory(&env, &req).await,
        (_, "/test/list") => list_test_memories(&env).await,
        (_, "/test/embed") => test_embed(&env, &req).await,
        (_, "/test/recall_semantic") => test_recall_semantic(&env, &req).await,
        (_, "/test/auth") => test_auth(&env, &req).await,
        _ => Response::ok("memoria — Cloudflare migration in progress (CLA-84)"),
    }
}

/// POST /mcp — the real MCP server endpoint. Validates Bearer, sets
/// AUTH_CTX scope, hands off to worker_mcp::handle for JSON-RPC dispatch.
async fn mcp_endpoint(env: &Env, req: &mut Request) -> Result<Response> {
    // Auth — require a valid bearer.
    let bearer = req
        .headers()
        .get("authorization")?
        .and_then(|h| h.strip_prefix("Bearer ").map(|s| s.to_string()));

    let Some(bearer) = bearer else {
        return Response::error("Missing Authorization: Bearer <key>", 401);
    };
    let Some(auth) = worker_auth_ctx::validate_bearer(env, &bearer) else {
        return Response::error("Invalid or unknown bearer token", 401);
    };

    let body = req.text().await?;
    worker_mcp::handle(env, &body, auth).await
}

/// Resolve a Bearer header to the worker's auth context. Pass the raw
/// service key as `Authorization: Bearer mk_rover_xxx`. Returns the
/// derived AuthCtx — role + key_id — or 401 if the bearer doesn't match
/// any configured MEMORIA_API_KEYS entry. OAuth-style bearers (Phase 5b)
/// will resolve here too once that lands.
async fn test_auth(env: &Env, req: &Request) -> Result<Response> {
    let bearer = req
        .headers()
        .get("authorization")?
        .and_then(|h| h.strip_prefix("Bearer ").map(|s| s.to_string()));

    let Some(bearer) = bearer else {
        return Response::error("Missing Authorization: Bearer <key>", 401);
    };

    match worker_auth_ctx::validate_bearer(env, &bearer) {
        Some(worker_auth_ctx::AuthCtx::ApiKey { role, key_id }) => Response::from_json(
            &serde_json::json!({
                "auth": "api_key",
                "role": role.as_str(),
                "key_id": key_id,
                "recorded_by": role.as_str(),
            }),
        ),
        Some(worker_auth_ctx::AuthCtx::OAuth) => Response::from_json(&serde_json::json!({
            "auth": "oauth",
        })),
        None => Response::error("Invalid or unknown bearer token", 401),
    }
}

/// Full write loop: D1 insert + Workers AI embed + Vectorize upsert.
/// `?content=<text>&summary=<text>` to override the defaults. After this
/// endpoint returns, the memory exists in D1 AND its vector exists in
/// Vectorize keyed by the same id — semantic recall via /test/recall_semantic
/// will find it.
async fn create_test_memory(env: &Env, req: &Request) -> Result<Response> {
    let url = req.url()?;
    let content = url
        .query_pairs()
        .find(|(k, _)| k == "content")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| "Smoke test: a memory written by the Worker via D1 + Vectorize.".into());
    let summary = url
        .query_pairs()
        .find(|(k, _)| k == "summary")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| "CLA-84 phase 4 smoke test".into());

    let db = env.d1("DB")?;
    let memory = worker_store::create_memory_with_provenance(
        &db,
        MemoryType::Episodic,
        content.clone(),
        summary,
        None,
        vec!["cla-84".into(), "smoke-test".into()],
        Some("claude".to_string()),
    )
    .await?;

    // Generate embedding and upsert to Vectorize keyed by the memory's id.
    let embedding = worker_embed::embed_document(env, &content).await?;
    worker_vectorize::upsert_one(env, &memory.id, &embedding).await?;

    Response::from_json(&serde_json::json!({
        "memory": memory,
        "vectorized": true,
        "embedding_dims": embedding.len(),
    }))
}

async fn list_test_memories(env: &Env) -> Result<Response> {
    let db = env.d1("DB")?;
    let memories = worker_store::recall_active(&db, 0.0, 10).await?;
    Response::from_json(&memories)
}

/// Full semantic-recall loop: embed query → Vectorize.query → fetch matching
/// memories from D1. `?q=<text>&top_k=<n>` (defaults: q="Chopper", top_k=5).
async fn test_recall_semantic(env: &Env, req: &Request) -> Result<Response> {
    let url = req.url()?;
    let query = url
        .query_pairs()
        .find(|(k, _)| k == "q")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| "Chopper".to_string());
    let top_k: u32 = url
        .query_pairs()
        .find(|(k, _)| k == "top_k")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(5);

    let query_embedding = worker_embed::embed_query(env, &query).await?;
    let matches = worker_vectorize::query_top_k(env, &query_embedding, top_k).await?;

    // Resolve match ids → full Memory records.
    let db = env.d1("DB")?;
    let mut results = Vec::with_capacity(matches.len());
    for m in &matches {
        if let Some(memory) = worker_store::get(&db, &m.id).await? {
            results.push(serde_json::json!({
                "score": m.score,
                "memory": memory,
            }));
        }
    }

    Response::from_json(&serde_json::json!({
        "query": query,
        "top_k": top_k,
        "match_count": matches.len(),
        "resolved_count": results.len(),
        "results": results,
    }))
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
