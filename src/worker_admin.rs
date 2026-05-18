// worker_admin.rs — Admin import endpoint for the data migration.
//
// One-off but durable: this endpoint stays in place after CLA-84 phase 8
// as a disaster-recovery path. Read the local SQLite, POST each memory
// to /admin/import, repeat — works for both the initial migration and
// any future "export → restore" scenarios.
//
// Auth: a dedicated ONEIRO_ADMIN_KEY wrangler secret (not the
// per-role service key allowlist). Constant-time compare. The admin key
// is the most privileged credential on the system — it can write
// memories verbatim (bypassing entity-binding + recorded_by forcing),
// so we deliberately keep it separate from the rover-shaped keys.
//
// Body shape (one memory per request):
// {
//   "memory": <Memory struct serialized via serde — see crate::memory::Memory>,
//   "image_base64": "<optional base64 of image bytes>",
//   "image_mime": "<optional, only when image_base64 is present>"
// }
//
// What happens server-side:
//   1. Verbatim INSERT into D1 (preserves id, timestamps, all fields)
//   2. If image_base64 present: SHA-256 + R2 upload (idempotent on same bytes)
//   3. Workers AI re-embed of memory.content with bge-base-en-v1.5
//   4. Vectorize upsert keyed by memory.id

use crate::memory::Memory;
use crate::{worker_embed, worker_store, worker_vectorize};
use serde::Deserialize;
use serde_json::json;
use worker::{Env, Request, Response, Result};

#[derive(Deserialize)]
struct ImportRequest {
    memory: Memory,
    #[serde(default)]
    image_base64: Option<String>,
    #[serde(default)]
    image_mime: Option<String>,
}

/// Constant-time string equality — same shape as worker_oauth::ct_eq.
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

fn authorize(env: &Env, req: &Request) -> Result<bool> {
    let bearer = req
        .headers()
        .get("authorization")?
        .and_then(|h| h.strip_prefix("Bearer ").map(|s| s.to_string()))
        .unwrap_or_default();
    let expected = env
        .secret("ONEIRO_ADMIN_KEY")
        .map(|s| s.to_string())
        .unwrap_or_default();
    if expected.is_empty() {
        // Fail closed if the admin key isn't configured — better to
        // 401 than to accept anything.
        return Ok(false);
    }
    Ok(ct_eq(&bearer, &expected))
}

pub async fn handle_import(env: &Env, req: &mut Request) -> Result<Response> {
    if !authorize(env, req)? {
        return Response::error("Invalid or missing admin key", 401);
    }

    let body = req.text().await?;
    let import: ImportRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("bad request: {}", e), 400),
    };

    let memory = import.memory;
    let db = env.d1("DB")?;

    // 1. Verbatim INSERT.
    if let Err(e) = worker_store::insert_memory_verbatim(&db, &memory).await {
        return Response::error(format!("D1 insert: {:?}", e), 500);
    }

    // 2. Image upload (if present). The memory row already carries
    //    image_hash/image_mime that the caller supplied; we trust them
    //    to match the bytes we're about to upload. R2 dedup means
    //    redundant uploads are no-ops.
    if let (Some(b64), Some(mime)) = (import.image_base64.as_ref(), import.image_mime.as_ref()) {
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
        let bytes = match BASE64.decode(b64.as_bytes()) {
            Ok(b) => b,
            Err(e) => return Response::error(format!("bad base64: {}", e), 400),
        };
        let bucket = env.bucket("IMAGES")?;
        if let Err(e) = worker_store::store_image_to_r2(&bucket, bytes, mime).await {
            return Response::error(format!("R2 upload: {:?}", e), 500);
        }
    }

    // 3 + 4. Re-embed with bge-base, upsert to Vectorize. The original
    //        local oneiro used nomic-embed-text — those vectors aren't
    //        compatible with bge-base's space, so we have to re-embed.
    //        Worth doing once at migration time so future semantic
    //        recalls use a uniform embedding model.
    let embedding = match worker_embed::embed_document(env, &memory.content).await {
        Ok(e) => e,
        Err(e) => return Response::error(format!("embed: {:?}", e), 500),
    };
    if let Err(e) = worker_vectorize::upsert_one(env, &memory.id, &embedding).await {
        return Response::error(format!("vectorize: {:?}", e), 500);
    }

    Response::from_json(&json!({
        "imported": memory.id,
        "embedding_dims": embedding.len(),
        "has_image": memory.image_hash.is_some(),
    }))
}
