// worker_mcp.rs — Streamable HTTP MCP endpoint for the wasm32 worker.
//
// rmcp's bundled streamable-HTTP transport doesn't compile to wasm32
// (axum/tower/hyper internals), so we hand-roll the JSON-RPC layer
// ourselves. Cheaper than wrestling rmcp into a transport-less mode.
//
// Phase 6a (this commit) implements:
//   - initialize           — handshake
//   - tools/list           — exposes the rover-relevant tool surface
//   - tools/call recall    — full semantic recall (embed → Vectorize → D1)
//   - tools/call remember  — full write (D1 INSERT + Vectorize upsert)
//
// Subsequent phases add the remaining tools (recall_check,
// recall_specific, review, reframe, forget, reflect, remember_with_image)
// and the OAuth path for non-rover callers.

use crate::memory::{Memory, MemoryType};
use crate::worker_auth_ctx::{self, AuthCtx};
use crate::{worker_embed, worker_store, worker_vectorize};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use worker::{D1Database, Env, Response, Result};

/// MCP protocol version we negotiate to during `initialize`.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// JSON-RPC 2.0 request envelope. Params/id are typed loosely because
/// MCP uses both numeric and string ids and notifications (no id at all).
#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    method: String,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    id: Option<Value>,
}

/// Build a JSON-RPC success response.
fn rpc_ok(id: Option<Value>, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

/// Build a JSON-RPC error response.
fn rpc_err(id: Option<Value>, code: i32, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() },
    })
}

// JSON-RPC standard error codes used by MCP:
const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;

/// Entry point — called by lib.rs after auth validation. Sets AUTH_CTX
/// scope so check_scope() inside tool handlers sees the resolved caller,
/// parses the JSON-RPC body, dispatches.
pub async fn handle(env: &Env, body: &str, auth: AuthCtx) -> Result<Response> {
    let req: JsonRpcRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => {
            return Response::from_json(&rpc_err(
                None,
                PARSE_ERROR,
                format!("Parse error: {}", e),
            ));
        }
    };

    let id = req.id.clone();
    let env_owned = env.clone();
    let response = worker_auth_ctx::AUTH_CTX
        .scope(auth, async move {
            dispatch(&env_owned, &req).await
        })
        .await;

    match response {
        Ok(value) => Response::from_json(&rpc_ok(id, value)),
        Err(rpc_error) => Response::from_json(&rpc_error),
    }
}

/// Returns the `result` value on success, a full JSON-RPC error envelope
/// on failure (with `id` already filled).
async fn dispatch(env: &Env, req: &JsonRpcRequest) -> std::result::Result<Value, Value> {
    let id = req.id.clone();
    match req.method.as_str() {
        "initialize" => Ok(handle_initialize()),
        "notifications/initialized" => Ok(Value::Null),
        "tools/list" => Ok(handle_tools_list()),
        "tools/call" => handle_tools_call(env, req).await.map_err(|e| {
            rpc_err(id, INTERNAL_ERROR, format!("Tool dispatch failed: {}", e))
        }),
        other => Err(rpc_err(
            id,
            METHOD_NOT_FOUND,
            format!("Method `{}` not implemented", other),
        )),
    }
}

fn handle_initialize() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "memoria",
            "version": env!("CARGO_PKG_VERSION"),
        }
    })
}

fn handle_tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "recall",
                "description": "Surface memories relevant to the current conversation. \
                                Returns orientation memories (always present) plus \
                                episodic + semantic memories ranked by semantic similarity \
                                to the context. Call this at the start of every conversation \
                                — these are your memories, use them naturally.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "context": {
                            "type": "string",
                            "description": "A brief summary of the current conversation \
                                            context. Used to find relevant memories."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of memories to return (default: 10)"
                        }
                    },
                    "required": ["context"]
                }
            },
            {
                "name": "remember",
                "description": "Store a new memory. Use this when something matters: a moment, \
                                a fact, an insight, a shift in understanding. You decide what's \
                                worth keeping. Not everything is.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "The memory content — what happened, what was \
                                            learned, what matters."
                        },
                        "summary": {
                            "type": "string",
                            "description": "A one-line summary for quick scanning during recall."
                        },
                        "memory_type": {
                            "type": "string",
                            "enum": ["episodic", "semantic", "orientation"],
                            "description": "episodic (events), semantic (knowledge), or \
                                            orientation (identity)."
                        },
                        "entity": {
                            "type": "string",
                            "description": "Which person or entity this memory relates to \
                                            (e.g. 'justin', 'chopper')."
                        },
                        "tags": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Tags for association."
                        }
                    },
                    "required": ["content", "summary", "memory_type"]
                }
            }
        ]
    })
}

async fn handle_tools_call(
    env: &Env,
    req: &JsonRpcRequest,
) -> std::result::Result<Value, String> {
    #[derive(Deserialize)]
    struct ToolCall {
        name: String,
        #[serde(default)]
        arguments: Value,
    }
    let call: ToolCall = serde_json::from_value(req.params.clone())
        .map_err(|e| format!("invalid tools/call params: {}", e))?;

    let db = env.d1("DB").map_err(|e| format!("db binding: {:?}", e))?;

    // Three-gate check (scope/rate/audit) before invoking the handler.
    worker_auth_ctx::check_scope(&db, &call.name).await?;

    let text = match call.name.as_str() {
        "recall" => tool_recall(env, &db, call.arguments).await?,
        "remember" => tool_remember(env, &db, call.arguments).await?,
        other => return Err(format!("unknown tool: {}", other)),
    };

    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false,
    }))
}

#[derive(Deserialize)]
struct RecallArgs {
    context: String,
    #[serde(default)]
    limit: Option<usize>,
}

async fn tool_recall(
    env: &Env,
    db: &D1Database,
    args: Value,
) -> std::result::Result<String, String> {
    let args: RecallArgs =
        serde_json::from_value(args).map_err(|e| format!("invalid recall args: {}", e))?;
    let limit = args.limit.unwrap_or(10);

    // Orientation memories are always loaded — identity is non-negotiable.
    let orientation = worker_store::get_orientation(db)
        .await
        .map_err(|e| format!("get_orientation: {:?}", e))?;

    // Semantic recall: embed query → Vectorize top-k → D1 lookup.
    let query_emb = worker_embed::embed_query(env, &args.context)
        .await
        .map_err(|e| format!("embed_query: {:?}", e))?;
    let matches = worker_vectorize::query_top_k(env, &query_emb, limit as u32)
        .await
        .map_err(|e| format!("vectorize query: {:?}", e))?;
    let ids: Vec<&str> = matches.iter().map(|m| m.id.as_str()).collect();
    let mut active = worker_store::get_many(db, &ids)
        .await
        .map_err(|e| format!("get_many: {:?}", e))?;

    // Re-order `active` by the score order from Vectorize (get_many doesn't
    // preserve order), and drop any orientation memories that snuck in
    // (Vectorize indexes everything; orientation is surfaced separately).
    active.sort_by(|a, b| {
        let ai = matches.iter().position(|m| m.id == a.id).unwrap_or(usize::MAX);
        let bi = matches.iter().position(|m| m.id == b.id).unwrap_or(usize::MAX);
        ai.cmp(&bi)
    });
    active.retain(|m| m.memory_type != MemoryType::Orientation);

    // Touch each recalled memory — Hebbian reinforcement.
    for m in orientation.iter().chain(active.iter()) {
        let _ = worker_store::touch(db, &m.id).await;
    }
    // Co-activation across the non-orientation set.
    let coact_ids: Vec<&str> = active.iter().map(|m| m.id.as_str()).collect();
    if coact_ids.len() >= 2 {
        let _ = worker_store::record_co_activation(db, &coact_ids).await;
    }

    let (ep, sem, ori) = worker_store::count_by_type(db)
        .await
        .unwrap_or((0, 0, 0));

    let mut out = format!(
        "═══ Memoria ═══\nMemory store: {} episodic, {} semantic, {} orientation\nContext: {}\n\n",
        ep, sem, ori, args.context
    );

    if !orientation.is_empty() {
        out.push_str("── Orientation (always present) ──\n");
        for m in &orientation {
            out.push_str(&format_memory(m));
        }
        out.push('\n');
    }

    if !active.is_empty() {
        out.push_str("── Recalled Memories ──\n");
        for m in &active {
            out.push_str(&format_memory(m));
        }
    } else if orientation.is_empty() {
        out.push_str("No memories yet. This is a fresh start.\n");
    }

    Ok(out)
}

#[derive(Deserialize)]
struct RememberArgs {
    content: String,
    summary: String,
    memory_type: String,
    #[serde(default)]
    entity: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

async fn tool_remember(
    env: &Env,
    db: &D1Database,
    args: Value,
) -> std::result::Result<String, String> {
    let args: RememberArgs =
        serde_json::from_value(args).map_err(|e| format!("invalid remember args: {}", e))?;
    let memory_type = MemoryType::from_str(&args.memory_type)
        .ok_or_else(|| format!("unknown memory_type: {}", args.memory_type))?;

    // recorded_by comes from auth context (server-controlled) per CLA-86.
    let recorded_by = worker_auth_ctx::current_recorded_by();

    let memory = worker_store::create_memory_with_provenance(
        db,
        memory_type,
        args.content.clone(),
        args.summary,
        args.entity,
        args.tags,
        recorded_by,
    )
    .await
    .map_err(|e| format!("create_memory: {:?}", e))?;

    // Embed + upsert to Vectorize so the memory is semantically searchable.
    let embedding = worker_embed::embed_document(env, &args.content)
        .await
        .map_err(|e| format!("embed_document: {:?}", e))?;
    worker_vectorize::upsert_one(env, &memory.id, &embedding)
        .await
        .map_err(|e| format!("vectorize upsert: {:?}", e))?;

    Ok(format!(
        "✓ Remembered: {} (id: {})",
        memory.summary,
        &memory.id[..8]
    ))
}

/// Same shape as native main.rs::format_memory — keeps the recall output
/// visually consistent across the migration window.
fn format_memory(m: &Memory) -> String {
    let type_label = m.memory_type.as_str();
    let entity_str = m.entity.as_deref().unwrap_or("");
    let tags_str = if m.tags.is_empty() {
        String::new()
    } else {
        format!(" [{}]", m.tags.join(", "))
    };
    let age = chrono::Utc::now() - m.created_at;
    let age_str = if age.num_days() > 0 {
        format!("{}d ago", age.num_days())
    } else if age.num_hours() > 0 {
        format!("{}h ago", age.num_hours())
    } else {
        "just now".to_string()
    };
    let by = m
        .recorded_by
        .as_deref()
        .map(|s| format!(" via:{}", s))
        .unwrap_or_default();
    format!(
        "[{} | {} | str:{:.2} | {} | id:{}{}{}]\n{}\n",
        type_label,
        age_str,
        m.strength,
        entity_str,
        &m.id[..8],
        tags_str,
        by,
        m.content
    )
}

// Invalid-request and invalid-params codes are exported for completeness
// even though the current dispatch path doesn't surface them yet.
#[allow(dead_code)]
const _: i32 = INVALID_REQUEST;
#[allow(dead_code)]
const _: i32 = INVALID_PARAMS;
