// worker_version.rs — Update notification via the recall response (CLA-102).
//
// Non-technical users won't read changelogs or watch GitHub releases.
// Without an in-band prompt, every future Memoria release — security
// patches, dialectic-prompt refinements, new tools — depends on the
// operator manually noticing it exists. The deployed worker becomes
// a frozen artifact of whatever version got installed.
//
// The leverage point: the Claude instance the user is talking to is
// itself the most reliable update prompt. If recall returns an
// `update_available` field, Claude will naturally surface it in the
// conversation — "looks like there's a Memoria update, want me to
// walk you through it?". The user gets prompted by the AI they're
// already speaking with, which is the only update channel that
// reliably reaches solo-dev-deployed software.
//
// Mechanism:
//   1. Compile-time `CARGO_PKG_VERSION` const baked into the bundle.
//   2. KV cache (TTL 6h) stores the most recently fetched VERSION.json.
//   3. On recall, check the cache; if missing or stale, fetch the
//      remote VERSION.json from GitHub raw and re-cache.
//   4. If the cached `latest_version` differs from the compiled one,
//      attach an `UpdateAvailable` payload to the recall response.
//
// Every error path returns `Ok(None)` — recall never fails because the
// version check failed. The user gets memories; they just don't get a
// prompt that pass.

#![cfg(target_family = "wasm")]

use serde::{Deserialize, Serialize};
use worker::{Env, Fetch, Method, Request, RequestInit, Result};

// ──── Tuning constants ──────────────────────────────────────────────────

/// Current worker version, baked in at compile time from Cargo.toml.
/// `release_branch` of `dev → master` includes a `Cargo.toml` bump
/// and a `VERSION.json` bump alongside it — the two must stay in sync
/// for the update prompt to fire correctly.
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Remote source-of-truth for the latest released version. Lives at the
/// repo root, served via GitHub raw. Updated as part of every release PR
/// (dev → master) alongside the Cargo.toml version bump.
const VERSION_URL: &str =
    "https://raw.githubusercontent.com/JuzzyDee/memoria/master/VERSION.json";

/// KV key for the cached remote version. We only ever have one entry, so
/// a fixed key is fine — namespace isolation is the binding.
const KV_KEY: &str = "remote_version";

/// KV binding name. Setup script creates the namespace; wrangler.toml.example
/// documents the binding.
const KV_BINDING: &str = "VERSION_CACHE";

/// 6 hours in seconds. Long enough to avoid hammering GitHub raw; short
/// enough that a release reaches users within a working day.
const CACHE_TTL_SECONDS: u64 = 6 * 60 * 60;

// ──── Types ─────────────────────────────────────────────────────────────

/// Shape of `VERSION.json` at the repo root. New fields are additive —
/// unknown keys are ignored by serde, missing optional keys default.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct RemoteVersion {
    latest_version: String,
    #[serde(default)]
    release_notes_url: Option<String>,
}

/// Payload attached to the recall response when an update is available.
/// `current` is the running worker's compiled version; `latest` is what
/// the remote VERSION.json reports; `url` points the user at release
/// notes if available.
///
/// Field shape designed to extend cleanly — future fields like
/// `breaking_changes`, `security_critical`, `deprecated_after` are
/// additive and don't break existing consumers.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateAvailable {
    pub current: String,
    pub latest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

// ──── Public entry point ────────────────────────────────────────────────

/// Check whether a worker update is available. Called from the recall
/// tool handler; the result is attached to the recall response so the
/// Claude instance using the tool can surface it conversationally.
///
/// Returns `Ok(None)` on every failure path — a missing KV binding,
/// unreachable GitHub, malformed VERSION.json, or matching versions
/// all collapse to "no prompt this time". Recall is never blocked by
/// the check.
pub async fn check_for_update(env: &Env) -> Result<Option<UpdateAvailable>> {
    let remote = match fetch_cached_or_remote(env).await {
        Ok(r) => r,
        Err(e) => {
            // Log and swallow — recall must not fail because the
            // version check failed.
            worker::console_error!("version check failed: {:?}", e);
            return Ok(None);
        }
    };

    if !is_remote_newer(CURRENT_VERSION, &remote.latest_version) {
        return Ok(None);
    }

    Ok(Some(UpdateAvailable {
        current: CURRENT_VERSION.to_string(),
        latest: remote.latest_version,
        url: remote.release_notes_url,
    }))
}

/// Compare two version strings, returning true only when `latest` is a
/// genuinely later release than `current`. Prevents the "any mismatch =
/// update available" footgun — a worker built from `0.2.0-dev` against
/// a release tag of `0.1.0` should not prompt the user to "update" to
/// an older version.
///
/// Parses major.minor.patch with optional `v` prefix and ignores any
/// pre-release suffix (`-dev`, `-rc1`, etc.). For malformed versions we
/// fall back to plain inequality to preserve the "something's off, tell
/// the user" behaviour rather than silently saying everything is fine.
fn is_remote_newer(current: &str, latest: &str) -> bool {
    fn parse(v: &str) -> Option<(u64, u64, u64)> {
        let core = v.trim().trim_start_matches('v').split('-').next()?;
        let mut parts = core.split('.');
        Some((
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        ))
    }

    match (parse(current), parse(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => latest != current,
    }
}

// ──── Cache + fetch plumbing ───────────────────────────────────────────

async fn fetch_cached_or_remote(env: &Env) -> Result<RemoteVersion> {
    let kv = env.kv(KV_BINDING)?;
    if let Some(cached) = kv.get(KV_KEY).text().await? {
        if let Ok(parsed) = serde_json::from_str::<RemoteVersion>(&cached) {
            return Ok(parsed);
        }
        // Corrupted cache entry — drop through to refetch.
        worker::console_error!("version cache parse failed; refetching");
    }

    let fresh = fetch_remote_version().await?;
    // Best-effort cache write — if it fails, we'll just refetch next time.
    let serialised = serde_json::to_string(&fresh).unwrap_or_default();
    if let Err(e) = kv
        .put(KV_KEY, serialised)?
        .expiration_ttl(CACHE_TTL_SECONDS)
        .execute()
        .await
    {
        worker::console_error!("version cache write failed: {:?}", e);
    }
    Ok(fresh)
}

async fn fetch_remote_version() -> Result<RemoteVersion> {
    let mut init = RequestInit::new();
    init.with_method(Method::Get);

    let req = Request::new_with_init(VERSION_URL, &init)?;
    let mut resp = Fetch::Request(req).send().await?;

    if resp.status_code() >= 400 {
        return Err(worker::Error::RustError(format!(
            "VERSION.json fetch status {}",
            resp.status_code()
        )));
    }

    let body = resp.text().await?;
    let parsed: RemoteVersion = serde_json::from_str(&body)
        .map_err(|e| worker::Error::RustError(format!("VERSION.json parse: {}", e)))?;
    Ok(parsed)
}
