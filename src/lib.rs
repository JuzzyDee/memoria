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

use worker::{event, Context, Env, Request, Response, Result};

#[event(fetch)]
async fn fetch(_req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    // Stub. Real routing lands in CLA-84 phase 6 (HTTP adapter).
    // Until then, deploying this Worker returns a placeholder so we can
    // confirm the build pipeline end-to-end.
    Response::ok("memoria — Cloudflare migration in progress (CLA-84)")
}
