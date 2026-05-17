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
mod worker_admin;
mod worker_audit;
mod worker_auth_ctx;
mod worker_dialectic;
mod worker_dialectic_audit;
mod worker_embed;
mod worker_mcp;
mod worker_mmr;
mod worker_oauth;
mod worker_rem;
mod worker_rem_audit;
mod worker_store;
mod worker_vectorize;

use worker::{event, Context, Env, Method, Request, Response, Result, ScheduleContext, ScheduledEvent};

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

        // Admin import — verbatim memory write for the data migration
        // (CLA-84 phase 8) and future disaster-recovery flows. Auth via
        // MEMORIA_ADMIN_KEY secret, NOT the per-role service-key allowlist.
        (Method::Post, "/admin/import") => worker_admin::handle_import(&env, &mut req).await,

        // OAuth 2.1 — Authorization Code + Client Credentials grants.
        (Method::Get, "/.well-known/oauth-protected-resource") => {
            worker_oauth::protected_resource_metadata(&base_url)
        }
        (Method::Get, "/.well-known/oauth-authorization-server") => {
            worker_oauth::authorization_server_metadata(&base_url)
        }
        (Method::Get, "/authorize") => render_consent_page(&env, &req).await,
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
///
/// CLA-91 Fix 2 — redirect_uri validated against the allowlist before
/// the page renders. Unregistered URIs get a 400 instead of a primed
/// consent page that would later create a pending code for an exfil
/// destination.
async fn render_consent_page(env: &Env, req: &Request) -> Result<Response> {
    let url = req.url()?;
    let q = |key: &str| -> String {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default()
    };
    let redirect_uri = q("redirect_uri");
    if !worker_oauth::is_registered_redirect_uri(env, &redirect_uri).await {
        return Response::error("invalid_request: redirect_uri not registered", 400);
    }
    worker_oauth::render_authorize_page(
        &q("client_id"),
        &redirect_uri,
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

/// Scheduled handler — dispatched by cron triggers declared in
/// wrangler.toml. We use the cron pattern that fired (via `event.cron()`)
/// to decide which cognitive loop to invoke:
///
///   - `0 14 * * *` (14:00 UTC / 00:00 AEST) → REM consolidator
///   - `0 8 * * *`  (08:00 UTC / 18:00 AEST) → Dialectic (CLA-95)
///
/// An unknown cron pattern falls through to REM as a safe default —
/// REM is idempotent and write-conservative, so firing it on a
/// misconfigured cron costs at most a wasted Haiku call.
#[event(scheduled)]
pub async fn scheduled(event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    let cron = event.cron();
    match cron.as_str() {
        "0 8 * * *" => run_dialectic(&env).await,
        _ => run_rem(&env).await,
    }
}

async fn run_rem(env: &Env) {
    match worker_rem::run(env).await {
        Ok(summary) => {
            worker::console_log!(
                "REM run complete: decayed={} clusters={} created={} appended={} revised={} skipped={} errors={}",
                summary.decayed,
                summary.clusters_attempted,
                summary.decisions_created,
                summary.decisions_appended,
                summary.decisions_revised,
                summary.decisions_skipped,
                summary.errors.len()
            );
            for err in &summary.errors {
                worker::console_error!("REM partial error: {}", err);
            }
        }
        Err(e) => {
            worker::console_error!("REM run failed catastrophically: {:?}", e);
        }
    }
}

async fn run_dialectic(env: &Env) {
    match worker_dialectic::run(env).await {
        Ok(summary) => {
            worker::console_log!(
                "Dialectic run complete: candidates={} decisions={} errors={}",
                summary.candidates_reviewed,
                summary.decisions_count,
                summary.errors.len()
            );
            for err in &summary.errors {
                worker::console_error!("Dialectic partial error: {}", err);
            }
        }
        Err(e) => {
            worker::console_error!("Dialectic run failed catastrophically: {:?}", e);
        }
    }
}
