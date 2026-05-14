// lib.rs — Memoria as a Cloudflare Worker (CLA-84).
//
// Public surface, by design, is just POST /mcp (authenticated MCP via
// JSON-RPC). Everything else falls through to a placeholder string —
// no anonymous read/write paths exist. The /test/* endpoints used
// during the migration's earlier phases have been removed; their
// behaviour is now reachable only through authenticated tools/call
// invocations.

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

use worker::{event, Context, Env, Request, Response, Result};

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let path = req.url()?.path().to_string();
    let method = req.method();
    match (method, path.as_str()) {
        // Real MCP endpoint — authenticated, scope-gated, audited.
        (worker::Method::Post, "/mcp") => mcp_endpoint(&env, &mut req).await,
        // Liveness — no info leakage, no DB access, just confirms the Worker
        // is alive. Safe for monitoring agents to poll.
        (worker::Method::Get, "/healthz") => Response::ok("ok"),
        // Anything else gets the migration placeholder. No 404 with stack
        // traces, no body-shaped responses that expose protocol details.
        _ => Response::ok("memoria"),
    }
}

/// POST /mcp — the MCP server endpoint. Validates Bearer, sets AUTH_CTX
/// scope, hands off to worker_mcp::handle for JSON-RPC dispatch.
async fn mcp_endpoint(env: &Env, req: &mut Request) -> Result<Response> {
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
