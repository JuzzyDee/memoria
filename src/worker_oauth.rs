// worker_oauth.rs — OAuth 2.1 server implementation for the wasm32 worker.
//
// Implements the minimum flow required by the MCP spec (and what Claude
// Code's connector wants):
//
//   GET  /.well-known/oauth-protected-resource    — RFC 9728
//   GET  /.well-known/oauth-authorization-server  — RFC 8414
//   GET  /authorize                                — consent HTML
//   POST /authorize                                — code creation + redirect
//   POST /token                                    — code exchange OR
//                                                    client_credentials grant
//
// Storage decisions (different from native auth.rs):
//   - **KV for tokens** — keys are the opaque bearer string; values are
//     `expires_at` epoch seconds. KV `expirationTtl` auto-cleans expired
//     entries. Native's HMAC-signed timestamp tokens are replaced with
//     plain random — KV lookup IS the validation, HMAC adds nothing.
//   - **KV for codes** — same shape, 5-min TTL. We delete-on-exchange so
//     a code can't be reused even within the TTL window. KV's eventual
//     consistency creates a sub-second race window for double-exchange;
//     acceptable for a single-tenant system.
//   - **Wrangler secret for client_secret** — encrypted at rest by
//     Cloudflare. Native's argon2 hash was for the on-disk auth.json
//     file; wrangler's encryption replaces that.
//
// Credential setup is documented in the deploy README — generate with
// openssl, push via `wrangler secret put`. Three secrets total:
//   MEMORIA_OAUTH_CLIENT_ID       — public identifier (e.g. memoria-abc123)
//   MEMORIA_OAUTH_CLIENT_SECRET   — pasted into Anthropic connector config
//   MEMORIA_TOKEN_SECRET          — not actually used since tokens are
//                                   opaque, but reserved for future
//                                   HMAC-signed variant.

use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use worker::{Env, Response, Result};

/// 7-day token lifetime — matches native auth.rs. Personal-server scale;
/// long-lived tokens are fine here because the connector relationship is
/// effectively pinned to the deployment.
const TOKEN_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;

/// Auth codes are short-lived — 5 minutes is plenty for the redirect
/// dance and far more than the actual flow needs (~seconds).
const CODE_TTL_SECONDS: u64 = 300;

const KV_BINDING: &str = "TOKENS";
const TOKEN_PREFIX: &str = "mem_";
const CODE_PREFIX: &str = "mem_code_";

/// Random opaque bearer token. 32 hex chars = 128 bits of entropy, more
/// than enough to make brute-force across KV's billions-of-keys space
/// statistically infeasible.
fn generate_token() -> String {
    // chrono::Utc::now() on wasm32 doesn't expose the wall-clock for
    // sub-second jitter, so we mix the current nanosecond-precision
    // (via js_sys::Date::now milliseconds) with random bytes. Random
    // alone is fine for security; the timestamp is just collision
    // insurance.
    let mut rng = rand::rng();
    let mut buf = [0u8; 16];
    rng.fill(&mut buf);
    format!("{}{}", TOKEN_PREFIX, hex::encode(buf))
}

fn generate_code() -> String {
    let mut rng = rand::rng();
    let mut buf = [0u8; 16];
    rng.fill(&mut buf);
    format!("{}{}", CODE_PREFIX, hex::encode(buf))
}

fn now_secs() -> u64 {
    // chrono::Utc::now() routes through js_sys::Date on wasm32 (wasmbind
    // feature) so this works on both targets without panicking.
    chrono::Utc::now().timestamp().max(0) as u64
}

/// A pending authorization code, stored in KV with TTL=300s.
#[derive(Serialize, Deserialize)]
struct PendingCode {
    client_id: String,
    redirect_uri: String,
    /// PKCE challenge — stored alongside the code but not currently
    /// verified on exchange (matches native auth.rs behaviour). Phase
    /// 5b.2 can add PKCE verification when we want stricter shape.
    #[serde(default)]
    code_challenge: Option<String>,
    expires_at: u64,
}

/// Validate an OAuth bearer. Returns true if the token is live in KV
/// and hasn't expired. Used by worker_auth_ctx::validate_bearer when
/// the bearer doesn't match the service-key (`mk_`) format.
pub async fn validate_token(env: &Env, token: &str) -> Result<bool> {
    let kv = env.kv(KV_BINDING)?;
    let Some(value): Option<String> = kv.get(token).text().await? else {
        return Ok(false);
    };
    // value is expires_at as a decimal string
    let expires_at: u64 = value.parse().unwrap_or(0);
    Ok(expires_at > now_secs())
}

/// True if a bearer looks like an OAuth token (vs a service API key).
/// Cheap prefix check before the KV roundtrip.
pub fn looks_like_oauth_token(bearer: &str) -> bool {
    bearer.starts_with(TOKEN_PREFIX) && !bearer.starts_with(CODE_PREFIX)
}

async fn read_client_creds(env: &Env) -> Result<(String, String)> {
    let client_id = env
        .secret("MEMORIA_OAUTH_CLIENT_ID")
        .map(|s| s.to_string())
        .map_err(|_| worker::Error::RustError("MEMORIA_OAUTH_CLIENT_ID not set".into()))?;
    let client_secret = env
        .secret("MEMORIA_OAUTH_CLIENT_SECRET")
        .map(|s| s.to_string())
        .map_err(|_| worker::Error::RustError("MEMORIA_OAUTH_CLIENT_SECRET not set".into()))?;
    Ok((client_id, client_secret))
}

/// Constant-time string equality — guard against timing-attack secret
/// recovery. Subtle: only constant-time for inputs of equal length;
/// short-circuit on length mismatch is fine because length isn't secret.
fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.as_bytes().iter().zip(b.as_bytes().iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ──────────────────────────────────────────────────────────────────────
// Metadata endpoints
// ──────────────────────────────────────────────────────────────────────

pub fn protected_resource_metadata(base_url: &str) -> Result<Response> {
    Response::from_json(&json!({
        "resource": base_url,
        "authorization_servers": [base_url],
        "scopes_supported": ["memoria"],
    }))
}

pub fn authorization_server_metadata(base_url: &str) -> Result<Response> {
    Response::from_json(&json!({
        "issuer": base_url,
        "authorization_endpoint": format!("{}/authorize", base_url),
        "token_endpoint": format!("{}/token", base_url),
        "token_endpoint_auth_methods_supported": [
            "client_secret_post",
            "client_secret_basic"
        ],
        "grant_types_supported": ["authorization_code", "client_credentials"],
        "scopes_supported": ["memoria"],
        "response_types_supported": ["code"],
        "code_challenge_methods_supported": ["S256"],
    }))
}

// ──────────────────────────────────────────────────────────────────────
// /authorize — consent page + form post
// ──────────────────────────────────────────────────────────────────────

pub fn render_authorize_page(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    scope: &str,
    code_challenge: &str,
) -> Result<Response> {
    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>Memoria — Authorize</title>
    <meta charset="utf-8">
    <style>
        body {{ font-family: system-ui, -apple-system, sans-serif; max-width: 440px;
                margin: 80px auto; padding: 24px; background: #1a1a1a; color: #e0e0e0; }}
        h1 {{ font-size: 1.4em; }}
        .info {{ background: #2a2a2a; padding: 16px; border-radius: 8px; margin: 20px 0; }}
        .info p {{ margin: 4px 0; color: #999; font-size: 0.9em; }}
        button {{ background: #4a9eff; color: white; border: none; padding: 12px 24px;
                  border-radius: 6px; font-size: 1em; cursor: pointer; width: 100%; }}
        button:hover {{ background: #3a8eef; }}
    </style>
</head>
<body>
    <h1>Authorize Claude to access Memoria</h1>
    <div class="info">
        <p>Client: {client_id}</p>
        <p>Scope: {scope}</p>
    </div>
    <p>This will allow Claude to read and write to your memory store.</p>
    <form method="POST" action="/authorize">
        <input type="hidden" name="client_id" value="{client_id}">
        <input type="hidden" name="redirect_uri" value="{redirect_uri}">
        <input type="hidden" name="state" value="{state}">
        <input type="hidden" name="scope" value="{scope}">
        <input type="hidden" name="code_challenge" value="{code_challenge}">
        <button type="submit">Allow</button>
    </form>
</body>
</html>"#
    );
    let mut resp = Response::from_html(html)?;
    resp.headers_mut().set("content-type", "text/html; charset=utf-8")?;
    Ok(resp)
}

/// POST /authorize — form submission. Creates a pending code, stores it
/// in KV with 5-min TTL, redirects back to the client's redirect_uri.
pub async fn handle_authorize_post(
    env: &Env,
    form: std::collections::HashMap<String, String>,
) -> Result<Response> {
    let client_id = form.get("client_id").cloned().unwrap_or_default();
    let redirect_uri = form.get("redirect_uri").cloned().unwrap_or_default();
    let state = form.get("state").cloned().unwrap_or_default();
    let code_challenge = form.get("code_challenge").cloned();

    let (registered_client_id, _secret) = read_client_creds(env).await?;
    if !ct_eq(&client_id, &registered_client_id) {
        return Response::error("invalid_client", 400);
    }

    let code = generate_code();
    let expires_at = now_secs() + CODE_TTL_SECONDS;
    let pending = PendingCode {
        client_id: client_id.clone(),
        redirect_uri: redirect_uri.clone(),
        code_challenge,
        expires_at,
    };
    let kv = env.kv(KV_BINDING)?;
    kv.put(&code, serde_json::to_string(&pending)?)?
        .expiration_ttl(CODE_TTL_SECONDS)
        .execute()
        .await?;

    let separator = if redirect_uri.contains('?') { '&' } else { '?' };
    let location = format!(
        "{}{}code={}&state={}",
        redirect_uri,
        separator,
        urlencoding_minimal(&code),
        urlencoding_minimal(&state),
    );
    let mut resp = Response::empty()?.with_status(302);
    resp.headers_mut().set("location", &location)?;
    Ok(resp)
}

// ──────────────────────────────────────────────────────────────────────
// /token — code exchange + client_credentials grant
// ──────────────────────────────────────────────────────────────────────

/// POST /token — both authorization_code and client_credentials grants.
/// Returns a JSON envelope shaped per RFC 6749 §5.1.
pub async fn handle_token_post(
    env: &Env,
    form: std::collections::HashMap<String, String>,
) -> Result<Response> {
    let grant_type = form.get("grant_type").cloned().unwrap_or_default();
    let client_id = form.get("client_id").cloned().unwrap_or_default();
    let client_secret = form.get("client_secret").cloned().unwrap_or_default();

    let (registered_client_id, registered_secret) = read_client_creds(env).await?;
    if !ct_eq(&client_id, &registered_client_id) {
        return token_error("invalid_client", "client_id does not match", 401);
    }
    if !ct_eq(&client_secret, &registered_secret) {
        return token_error("invalid_client", "client_secret does not match", 401);
    }

    match grant_type.as_str() {
        "authorization_code" => {
            let code = form.get("code").cloned().unwrap_or_default();
            let redirect_uri = form.get("redirect_uri").cloned().unwrap_or_default();
            exchange_code(env, &code, &client_id, &redirect_uri).await
        }
        "client_credentials" => issue_token(env).await,
        other => token_error("unsupported_grant_type", &format!("grant_type {} not supported", other), 400),
    }
}

async fn exchange_code(
    env: &Env,
    code: &str,
    client_id: &str,
    redirect_uri: &str,
) -> Result<Response> {
    let kv = env.kv(KV_BINDING)?;
    let Some(value): Option<String> = kv.get(code).text().await? else {
        return token_error("invalid_grant", "code not found", 400);
    };
    // One-shot: delete on read so it can't be exchanged twice. KV is
    // eventually consistent so there's a sub-second race window — for a
    // single-tenant system this is acceptable.
    kv.delete(code).await?;

    let pending: PendingCode = serde_json::from_str(&value)
        .map_err(|e| worker::Error::RustError(format!("pending code malformed: {}", e)))?;
    if pending.expires_at <= now_secs() {
        return token_error("invalid_grant", "code expired", 400);
    }
    if !ct_eq(&pending.client_id, client_id) {
        return token_error("invalid_grant", "code/client mismatch", 400);
    }
    if !ct_eq(&pending.redirect_uri, redirect_uri) {
        return token_error("invalid_grant", "redirect_uri mismatch", 400);
    }
    // (PKCE verification could go here in a follow-up.)

    issue_token(env).await
}

async fn issue_token(env: &Env) -> Result<Response> {
    let token = generate_token();
    let expires_at = now_secs() + TOKEN_TTL_SECONDS;
    let kv = env.kv(KV_BINDING)?;
    kv.put(&token, expires_at.to_string())?
        .expiration_ttl(TOKEN_TTL_SECONDS)
        .execute()
        .await?;

    Response::from_json(&json!({
        "access_token": token,
        "token_type": "Bearer",
        "expires_in": TOKEN_TTL_SECONDS,
        "scope": "memoria",
    }))
}

fn token_error(error: &str, description: &str, status: u16) -> Result<Response> {
    Ok(Response::from_json(&json!({
        "error": error,
        "error_description": description,
    }))?
    .with_status(status))
}

/// Minimal URL-encoder for the redirect Location value. Covers what the
/// auth code + state strings will actually contain (`mem_code_` prefix +
/// hex chars + state) which is essentially URL-safe already, but the
/// state from clients might contain anything.
fn urlencoding_minimal(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect()
}

/// Parse an application/x-www-form-urlencoded body into a HashMap.
pub fn parse_form(body: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for pair in body.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            map.insert(
                urldecode_minimal(k),
                urldecode_minimal(v),
            );
        }
    }
    map
}

fn urldecode_minimal(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
            } else {
                out.push(bytes[i]);
            }
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
