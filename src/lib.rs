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
mod worker_oauth;
mod worker_store;
mod worker_vectorize;

use worker::{event, Context, Env, Method, Request, Response, Result};

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let path = url.path().to_string();
    let method = req.method();

    // base_url for the OAuth metadata responses — must match how clients
    // arrive at the worker. Pulled from the Host header so it works
    // identically with custom domains and the workers.dev subdomain.
    let host = req
        .headers()
        .get("host")?
        .unwrap_or_else(|| "memoria.juzzydee.workers.dev".to_string());
    let base_url = format!("https://{}", host);

    match (method, path.as_str()) {
        // MCP — authenticated, scope-gated, audited. Accepts both
        // OAuth bearer tokens and service API keys.
        (Method::Post, "/mcp") => mcp_endpoint(&env, &mut req).await,

        // OAuth 2.1 — Authorization Code + Client Credentials grants.
        (Method::Get, "/.well-known/oauth-protected-resource") => {
            worker_oauth::protected_resource_metadata(&base_url)
        }
        (Method::Get, "/.well-known/oauth-authorization-server") => {
            worker_oauth::authorization_server_metadata(&base_url)
        }
        (Method::Get, "/authorize") => render_consent_page(&req),
        (Method::Post, "/authorize") => handle_authorize_form(&env, &mut req).await,
        (Method::Post, "/token") => handle_token_form(&env, &mut req).await,

        // Liveness.
        (Method::Get, "/healthz") => Response::ok("ok"),

        // Default — no info leakage.
        _ => Response::ok("memoria"),
    }
}

/// POST /mcp — the MCP server endpoint. Validates Bearer (OAuth OR
/// service API key), sets AUTH_CTX scope, hands off to worker_mcp::handle
/// for JSON-RPC dispatch.
async fn mcp_endpoint(env: &Env, req: &mut Request) -> Result<Response> {
    let bearer = req
        .headers()
        .get("authorization")?
        .and_then(|h| h.strip_prefix("Bearer ").map(|s| s.to_string()));

    let Some(bearer) = bearer else {
        return Response::error("Missing Authorization: Bearer <key>", 401);
    };
    let Some(auth) = worker_auth_ctx::validate_bearer(env, &bearer).await else {
        return Response::error("Invalid or unknown bearer token", 401);
    };

    let body = req.text().await?;
    worker_mcp::handle(env, &body, auth).await
}

/// GET /authorize — renders the consent HTML page. Query params:
///   client_id, redirect_uri, state, scope, code_challenge
fn render_consent_page(req: &Request) -> Result<Response> {
    let url = req.url()?;
    let q = |key: &str| -> String {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default()
    };
    worker_oauth::render_authorize_page(
        &q("client_id"),
        &q("redirect_uri"),
        &q("state"),
        &q("scope"),
        &q("code_challenge"),
    )
}

async fn handle_authorize_form(env: &Env, req: &mut Request) -> Result<Response> {
    let body = req.text().await?;
    let form = worker_oauth::parse_form(&body);
    worker_oauth::handle_authorize_post(env, form).await
}

async fn handle_token_form(env: &Env, req: &mut Request) -> Result<Response> {
    let body = req.text().await?;
    let form = worker_oauth::parse_form(&body);
    worker_oauth::handle_token_post(env, form).await
}
