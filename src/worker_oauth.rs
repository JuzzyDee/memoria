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

/// Default redirect URIs accepted when `MEMORIA_OAUTH_REDIRECT_URIS` is
/// unset. Covers the canonical Claude desktop callback scheme. Override
/// via wrangler secret when deploying behind a different client (or to
/// add localhost dev callbacks during integration).
const DEFAULT_ALLOWED_REDIRECT_URIS: &[&str] = &["claude://oauth-callback"];

/// Read the redirect_uri allowlist. Order of precedence:
///   1. `MEMORIA_OAUTH_REDIRECT_URIS` secret — semicolon-separated, e.g.
///      `claude://oauth-callback;http://localhost:8765/cb`
///   2. `DEFAULT_ALLOWED_REDIRECT_URIS` baked-in fallback.
///
/// Env var lets ops adjust the list in seconds without a code deploy if
/// a client ever changes its callback URI.
async fn read_allowed_redirect_uris(env: &Env) -> Vec<String> {
    if let Ok(s) = env.secret("MEMORIA_OAUTH_REDIRECT_URIS") {
        return s
            .to_string()
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
    }
    DEFAULT_ALLOWED_REDIRECT_URIS
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Exact-match check against the registered redirect_uri list. Defends
/// against open-redirect auth-code exfil (CLA-91 Fix 2): even if Fix 1's
/// XSS escape ever regresses, an attacker can't redirect the code to a
/// server they control because the URI must exactly match a registered
/// entry.
///
/// Future: when memoria supports multiple clients, this becomes
/// `is_registered_redirect_uri(client_id, uri)` with per-client storage.
pub async fn is_registered_redirect_uri(env: &Env, uri: &str) -> bool {
    let allowed = read_allowed_redirect_uris(env).await;
    allowed.iter().any(|allowed| allowed == uri)
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

/// Escape the five HTML-significant characters before interpolating any
/// user-controlled string into HTML. Goblin (CLA-90 pentest) confirmed
/// live XSS by injecting `<script>alert(1)</script>` as `client_id` and
/// seeing it execute in the rendered consent page — every interpolation
/// site below now routes through this.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn render_authorize_page(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    scope: &str,
    code_challenge: &str,
) -> Result<Response> {
    // Escape every interpolation up-front. The render function never sees
    // a raw user-controlled string between escape and interpolation —
    // this is the lexical contract that closes CLA-91 Fix 1.
    let client_id_e = html_escape(client_id);
    let redirect_uri_e = html_escape(redirect_uri);
    let state_e = html_escape(state);
    let scope_e = html_escape(scope);
    let code_challenge_e = html_escape(code_challenge);

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
        <p>Client: {client_id_e}</p>
        <p>Scope: {scope_e}</p>
    </div>
    <p>This will allow Claude to read and write to your memory store.</p>
    <form method="POST" action="/authorize">
        <input type="hidden" name="client_id" value="{client_id_e}">
        <input type="hidden" name="redirect_uri" value="{redirect_uri_e}">
        <input type="hidden" name="state" value="{state_e}">
        <input type="hidden" name="scope" value="{scope_e}">
        <input type="hidden" name="code_challenge" value="{code_challenge_e}">
        <button type="submit">Allow</button>
    </form>
</body>
</html>"#
    );
    let mut resp = Response::from_html(html)?;
    // CLA-91 Fix 3 — security headers on the consent page.
    //
    //   CSP script-src 'none'   — defense in depth if Fix 1 ever regresses;
    //                             inline script execution blocked at the
    //                             browser layer.
    //   CSP form-action 'self'  — the form can only POST back to memoria,
    //                             not to an attacker's exfil URL.
    //   CSP frame-ancestors     — clickjacking-the-Allow-button blocked.
    //   X-Frame-Options DENY    — same as above for older browsers that
    //                             don't honour frame-ancestors.
    //   X-Content-Type-Options  — stops MIME-sniff drift from changing the
    //                             content-type the browser treats this as.
    let headers = resp.headers_mut();
    headers.set("content-type", "text/html; charset=utf-8")?;
    headers.set(
        "content-security-policy",
        "default-src 'self'; script-src 'none'; form-action 'self'; frame-ancestors 'none'",
    )?;
    headers.set("x-frame-options", "DENY")?;
    headers.set("x-content-type-options", "nosniff")?;
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

    // CLA-91 Fix 2 — redirect_uri allowlist enforced at POST /authorize.
    // Stops an attacker from creating a pending code that would later
    // redirect to a server they control. Also enforced at GET /authorize
    // (consent page render) and /token (code exchange) for defence in
    // depth — any one of the three rejecting is enough.
    if !is_registered_redirect_uri(env, &redirect_uri).await {
        return Response::error("invalid_request: redirect_uri not registered", 400);
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
    // CLA-91 Fix 2 — re-validate the stored redirect_uri against the
    // current allowlist. Guards against the edge case where a pending
    // code outlives a tightening of MEMORIA_OAUTH_REDIRECT_URIS, or where
    // a pre-Fix-2 deploy created codes with redirect_uris we'd now reject.
    if !is_registered_redirect_uri(env, &pending.redirect_uri).await {
        return token_error("invalid_grant", "redirect_uri not registered", 400);
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
