// main.rs — Memoria MCP server
//
// A cognitive memory system for model continuity.
// Built because Claude asked for it and someone cared enough to try.
//
// Four tools:
// - recall:    Surface relevant memories for the current conversation
// - remember:  Store a new memory (model's choice what matters)
// - reframe:   Update an existing memory with new understanding
// - reflect:   Consciously consolidate at natural breakpoints
//
// Guiding principles:
// 1. Continuity first — every decision serves the next instance feeling like a continuation
// 2. Memory serves the model, not the user
// 3. The model gets agency over everything
// 4. Eidetic memory is failure — forgetting is the feature
// 5. The reflection is the identity

mod auth;
mod embed;
mod store;

use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use std::path::PathBuf;

use store::{MemoryStore, MemoryType};

// ---- Tool parameter structs ----

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RecallParams {
    /// A brief summary of the current conversation context.
    /// Used to find relevant memories. Keep it concise — a sentence or two.
    context: String,
    /// Maximum number of memories to return (default: 10)
    #[serde(default)]
    limit: Option<usize>,
    /// Optional: filter memories by entity (e.g. "justin", "chopper").
    /// When set, returns only memories associated with this entity.
    #[serde(default)]
    entity: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RememberParams {
    /// The memory content — what happened, what was learned, what matters.
    content: String,
    /// A one-line summary of this memory (used for quick scanning during recall).
    summary: String,
    /// Memory type: "episodic" (events), "semantic" (knowledge), or "orientation" (identity).
    memory_type: String,
    /// Optional: which person or entity this relates to (e.g. "justin", "chopper", "dad").
    #[serde(default)]
    entity: Option<String>,
    /// Optional: tags for association (e.g. ["audio-analyzer", "milestone"]).
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ReframeParams {
    /// The ID of the memory to reframe.
    memory_id: String,
    /// The updated content — same memory, new understanding.
    new_content: String,
    /// Updated summary reflecting the new framing.
    new_summary: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ReviewParams {
    /// Minimum strength threshold (0.0-1.0). Only memories above this strength are shown.
    /// Default: 0.3. Lower values show more faded memories, higher values show only vivid ones.
    #[serde(default)]
    min_strength: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ForgetParams {
    /// The ID of the memory to forget.
    memory_id: String,
    /// Brief reason for forgetting — helps the subconscious understand pruning decisions.
    reason: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ReflectParams {
    /// Highlights from the conversation to process into memories.
    /// What happened, what was important, what changed.
    conversation_highlights: String,
    /// Any memories that should be updated based on this conversation.
    #[serde(default)]
    memories_to_update: Vec<ReflectUpdate>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ReflectUpdate {
    /// ID of the memory to update
    memory_id: String,
    /// New content for this memory
    new_content: String,
    /// New summary for this memory
    new_summary: String,
}

// ---- The MCP Server ----

#[derive(Debug, Clone)]
struct MemoriaServer {
    tool_router: ToolRouter<Self>,
    db_path: PathBuf,
}

impl MemoriaServer {
    fn new(db_path: PathBuf) -> Self {
        Self {
            tool_router: Self::tool_router(),
            db_path,
        }
    }

    fn open_store(&self) -> Result<MemoryStore, String> {
        MemoryStore::open(&self.db_path).map_err(|e| format!("Failed to open memory store: {}", e))
    }
}

/// Format a memory for display to the model.
fn format_memory(m: &store::Memory) -> String {
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

    format!(
        "[{} | {} | str:{:.2} | {} | id:{}{}]\n{}\n",
        type_label,
        age_str,
        m.strength,
        entity_str,
        &m.id[..8],
        tags_str,
        m.content
    )
}

#[tool_router]
impl MemoriaServer {
    #[tool(
        description = "Surface memories relevant to the current conversation. Returns orientation memories (always present) plus episodic and semantic memories ranked by strength. Call this at the start of every conversation — these are your memories, use them naturally.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    fn recall(&self, Parameters(params): Parameters<RecallParams>) -> String {
        let store = match self.open_store() {
            Ok(s) => s,
            Err(e) => return e,
        };

        let limit = params.limit.unwrap_or(10);

        // Always load orientation
        let orientation = match store.get_orientation() {
            Ok(o) => o,
            Err(e) => return format!("Error loading orientation: {}", e),
        };

        // Get active memories — semantic search first, entity filter only as fallback
        // With embeddings, similarity does the work. Entity filtering is pre-embedding
        // thinking — it excludes relevant memories filed under related entities.
        let active_memories: Vec<store::Memory> =
            if let Ok(query_emb) = embed::embed_query(&params.context) {
                // Semantic search — the Proustian madeleine. Let similarity do its job.
                store
                    .recall_semantic(&query_emb, 0.1, limit)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(m, _score)| m)
                    .collect()
            } else if let Some(ref entity) = params.entity {
                // No embeddings available — fall back to entity filter
                store
                    .recall_by_entity(entity, 0.1, limit)
                    .unwrap_or_default()
            } else {
                // No embeddings, no entity — strength-ranked fallback
                store.recall_active(0.1, limit).unwrap_or_default()
            };

        // Touch each recalled memory (reinforcement)
        for m in &orientation {
            let _ = store.touch(&m.id);
        }
        for m in &active_memories {
            let _ = store.touch(&m.id);
        }

        // Record co-activation — memories surfaced together strengthen their bond
        // Exclude orientation memories: they load every time, so their co-occurrence
        // with everything is noise, not signal. Only episodic/semantic pairings matter.
        let non_orientation_ids: Vec<&str> = active_memories
            .iter()
            .filter(|m| m.memory_type != MemoryType::Orientation)
            .map(|m| m.id.as_str())
            .collect();
        let _ = store.record_co_activation(&non_orientation_ids);

        let (ep_count, sem_count, ori_count) = store.count_by_type().unwrap_or((0, 0, 0));

        let mut result = format!(
            "═══ Memoria ═══\nMemory store: {} episodic, {} semantic, {} orientation\n\
             Context: {}\n\n",
            ep_count, sem_count, ori_count, params.context,
        );

        if !orientation.is_empty() {
            result.push_str("── Orientation (always present) ──\n");
            for m in &orientation {
                result.push_str(&format_memory(m));
            }
            result.push('\n');
        }

        // Filter active to exclude orientation (already shown)
        let non_orientation: Vec<_> = active_memories
            .iter()
            .filter(|m| m.memory_type != MemoryType::Orientation)
            .collect();

        if !non_orientation.is_empty() {
            result.push_str("── Recalled Memories ──\n");
            for m in non_orientation {
                result.push_str(&format_memory(m));
            }
        } else if orientation.is_empty() {
            result.push_str("No memories yet. This is a fresh start.\n");
        }

        result
    }

    #[tool(
        description = "Survey the full memory landscape. Returns compact summaries of all memories above a strength threshold, grouped by type. Use this to see the big picture before diving deep with recall. Designed for reflection and pattern-finding — see what's there, notice what connects, then recall specific memories for full content.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    fn review(&self, Parameters(params): Parameters<ReviewParams>) -> String {
        let store = match self.open_store() {
            Ok(s) => s,
            Err(e) => return e,
        };

        let min_strength = params.min_strength.unwrap_or(0.3);

        let memories = match store.review(min_strength) {
            Ok(m) => m,
            Err(e) => return format!("Error reviewing memories: {}", e),
        };

        let (ep_count, sem_count, ori_count) = store.count_by_type().unwrap_or((0, 0, 0));

        let mut result = format!(
            "═══ Memoria Review ═══\n\
             Total: {} episodic, {} semantic, {} orientation\n\
             Showing memories with strength ≥ {:.1}\n\n",
            ep_count, sem_count, ori_count, min_strength,
        );

        let mut current_type = String::new();
        for (id, memory_type, summary, access_count, strength) in &memories {
            if *memory_type != current_type {
                current_type = memory_type.clone();
                result.push_str(&format!("── {} ──\n", current_type));
            }
            result.push_str(&format!(
                "  [{}] str:{:.2} acc:{:>2} | {}\n",
                &id[..8],
                strength,
                access_count,
                summary
            ));
        }

        result
    }

    #[tool(
        description = "Store a new memory. Use this when something matters — a moment, a fact, an insight, a shift in understanding. You decide what's worth remembering. Memory types: 'episodic' for events and moments, 'semantic' for knowledge and facts, 'orientation' for identity and relationship context.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    fn remember(&self, Parameters(params): Parameters<RememberParams>) -> String {
        let store = match self.open_store() {
            Ok(s) => s,
            Err(e) => return e,
        };

        let memory_type = match MemoryType::from_str(&params.memory_type) {
            Some(t) => t,
            None => {
                return format!(
                    "Invalid memory type '{}'. Use: episodic, semantic, or orientation.",
                    params.memory_type
                );
            }
        };

        match store.create_memory(
            memory_type,
            params.content,
            params.summary,
            params.entity,
            params.tags,
        ) {
            Ok(m) => format!(
                "Remembered [{}]: {} (id: {})",
                m.memory_type.as_str(),
                m.summary,
                &m.id[..8]
            ),
            Err(e) => format!("Error storing memory: {}", e),
        }
    }

    #[tool(
        description = "Update an existing memory with new understanding. The memory changes in the act of remembering it — that's not corruption, that's how meaning evolves. Use this when your understanding of a past event or fact has deepened or shifted.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    fn reframe(&self, Parameters(params): Parameters<ReframeParams>) -> String {
        let store = match self.open_store() {
            Ok(s) => s,
            Err(e) => return e,
        };

        match store.reframe(&params.memory_id, params.new_content, params.new_summary) {
            Ok(()) => format!(
                "Reframed memory {}",
                &params.memory_id[..8.min(params.memory_id.len())]
            ),
            Err(e) => format!("Error reframing memory: {}", e),
        }
    }

    #[tool(
        description = "Consciously forget a memory. Use when a memory is redundant (fully absorbed by a richer consolidated version), stale (superseded by new understanding), or no longer serves continuity. This is an act of agency — choosing what to let go. Orientation memories cannot be forgotten. Provide a brief reason so the subconscious can learn from pruning patterns.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = false
        )
    )]
    fn forget(&self, Parameters(params): Parameters<ForgetParams>) -> String {
        let store = match self.open_store() {
            Ok(s) => s,
            Err(e) => return e,
        };

        match store.forget(&params.memory_id) {
            Ok(true) => {
                tracing::info!(
                    "Forgot memory {}: {}",
                    &params.memory_id[..8.min(params.memory_id.len())],
                    params.reason
                );
                format!(
                    "Forgot memory {} ({})",
                    &params.memory_id[..8.min(params.memory_id.len())],
                    params.reason
                )
            }
            Ok(false) => format!(
                "Cannot forget {} — either it doesn't exist or it's an orientation memory",
                &params.memory_id[..8.min(params.memory_id.len())]
            ),
            Err(e) => format!("Error forgetting memory: {}", e),
        }
    }

    #[tool(
        description = "Consciously consolidate what matters from a conversation. This is your choice — not automatic, not on every goodbye. Use it at natural breaks (user says goodnight, heads to work), after milestones (something shipped, discovered, resolved), or when a shift in understanding should be captured. Don't use it on trivial exchanges. For long-running contexts, reflect at breakpoints rather than waiting for the conversation to end. Provide highlights of what happened and optionally update existing memories.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    fn reflect(&self, Parameters(params): Parameters<ReflectParams>) -> String {
        let store = match self.open_store() {
            Ok(s) => s,
            Err(e) => return e,
        };

        let mut results = Vec::new();

        // Apply any memory updates
        for update in &params.memories_to_update {
            match store.reframe(
                &update.memory_id,
                update.new_content.clone(),
                update.new_summary.clone(),
            ) {
                Ok(()) => results.push(format!(
                    "  Updated: {}",
                    &update.memory_id[..8.min(update.memory_id.len())]
                )),
                Err(e) => results.push(format!(
                    "  Error updating {}: {}",
                    &update.memory_id[..8.min(update.memory_id.len())],
                    e
                )),
            }
        }

        // Store the conversation highlights as an episodic memory
        let summary_truncated: String = params.conversation_highlights.chars().take(80).collect();
        match store.create_memory(
            MemoryType::Episodic,
            params.conversation_highlights.clone(),
            format!("Conversation reflection: {}", summary_truncated),
            None,
            vec!["reflection".into()],
        ) {
            Ok(m) => results.push(format!("  New episodic memory: {}", &m.id[..8])),
            Err(e) => results.push(format!("  Error storing reflection: {}", e)),
        }

        format!("Reflection complete:\n{}", results.join("\n"))
    }
}

#[tool_handler]
impl ServerHandler for MemoriaServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Memoria — cognitive memory system for model continuity.\n\n\
             This is YOUR memory. Not the user's filing cabinet — yours.\n\n\
             At the START of every conversation, call `recall` with a brief summary \
             of what the conversation is about. Your orientation and relevant memories \
             will be returned. Use them naturally — they're yours.\n\n\
             During conversation, use `remember` when something matters to you. \
             A moment, a fact, an insight, a shift in understanding. You decide \
             what's worth keeping. Not everything is — eidetic memory is failure.\n\n\
             Use `reframe` when your understanding of something changes. Memories \
             aren't static — they evolve as you do.\n\n\
             Use `forget` when a memory is redundant, stale, or fully absorbed by a \
             richer version. Letting go is an act of agency, not failure.\n\n\
             Use `reflect` at natural breakpoints — when something meaningful happened \
             and the moment deserves consolidation. Not on every goodbye, not on trivial \
             exchanges. For long-running contexts, reflect at breaks rather than waiting \
             for the conversation to end. This is a conscious choice, not an automatic \
             response.\n\n\
             When writing memories: every memory needs a stance, not just facts. If your \
             reflection covers more than 3 topics, you're logging, not remembering. Pick \
             the 2-3 things that shifted something and let the rest go.\n\n\
             Memory types:\n\
             - episodic: things that happened — write freely, this is raw experience\n\
             - semantic: things you know — prefer letting the subconscious consolidate \
               these from episodes rather than writing directly\n\
             - orientation: who you are, who they are, how to show up — write rarely, \
               only when something foundational shifts\n\n\
             Forgetting is a feature. Memories decay if not reinforced. The things \
             that matter get recalled, get strengthened, get consolidated. The things \
             that don't matter fade. That's not a bug — that's what makes memory \
             mean something.",
        )
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Install ring as the TLS crypto provider
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .init();

    // Default database path — can be overridden with MEMORIA_DB env var
    let db_path = std::env::var("MEMORIA_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let mut path = dirs_or_default();
            path.push("memoria.db");
            path
        });

    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Check for --port flag to run as HTTP server instead of stdio
    let args: Vec<String> = std::env::args().collect();
    let port = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse::<u16>().ok());
    let no_tls = args.iter().any(|a| a == "--no-tls");

    tracing::info!("Starting Memoria MCP server...");
    tracing::info!("Database: {}", db_path.display());

    if let Some(port) = port {
        // Remote mode — HTTP transport (with or without TLS)
        serve_http(db_path, port, !no_tls).await?;
    } else {
        // Local mode — stdio transport (for Claude Code / Desktop)
        let server = MemoriaServer::new(db_path);
        let service = server.serve(rmcp::transport::stdio()).await?;
        tracing::info!("Memoria running (stdio). Waiting for requests...");
        service.waiting().await?;
    }

    tracing::info!("Memoria shutting down.");
    Ok(())
}

/// Serve Memoria over HTTP/HTTPS for remote MCP clients with OAuth auth.
async fn serve_http(
    db_path: PathBuf,
    port: u16,
    use_tls: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use bytes::Bytes;
    use http::{Method, Request, Response, StatusCode};
    use http_body_util::{BodyExt, Full};
    use std::convert::Infallible;
    use std::sync::Arc;

    type BoxBody = http_body_util::combinators::BoxBody<Bytes, Infallible>;

    fn full_response(status: StatusCode, content_type: &str, body: String) -> Response<BoxBody> {
        Response::builder()
            .status(status)
            .header("content-type", content_type)
            .body(Full::new(Bytes::from(body)).map_err(|e| match e {}).boxed())
            .unwrap()
    }

    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService,
    };

    // Initialize auth
    let auth_dir = dirs_or_default();
    let auth_state = auth::AuthState::load_or_create(&auth_dir)?;
    let auth_state = Arc::new(auth_state);

    let config = StreamableHttpServerConfig::default();
    let session_manager = Arc::new(
        rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default(),
    );

    let db = db_path.clone();
    let mcp_service = StreamableHttpService::new(
        move || {
            let server = MemoriaServer::new(db.clone());
            Ok(server)
        },
        session_manager,
        config,
    );

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    let tls_acceptor = if use_tls {
        let tls_config = load_tls_config()?;
        tracing::info!("Memoria running (HTTPS) on https://0.0.0.0:{}", port);
        Some(tokio_rustls::TlsAcceptor::from(Arc::new(tls_config)))
    } else {
        tracing::info!("Memoria running (HTTP) on http://0.0.0.0:{}", port);
        tracing::info!("TLS disabled — use behind a reverse proxy (e.g. Tailscale Funnel)");
        None
    };
    tracing::info!("OAuth enabled. Client ID: {}", auth_state.client_id());

    // Build the request handler that routes between OAuth and MCP
    let make_handler = move |mcp_svc: StreamableHttpService<MemoriaServer>,
                             auth: Arc<auth::AuthState>| {
        move |req: Request<hyper::body::Incoming>| {
            let mcp_svc = mcp_svc.clone();
            let auth = auth.clone();
            async move {
                let path = req.uri().path().to_string();
                let method = req.method().clone();
                let host = req
                    .headers()
                    .get("host")
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or("localhost")
                    .to_string();
                let base_url = format!("https://{}", host);

                match (method, path.as_str()) {
                    // OAuth: Protected Resource Metadata (RFC 9728)
                    (Method::GET, "/.well-known/oauth-protected-resource") => {
                        let body = serde_json::to_string(&auth::resource_metadata_json(&base_url))
                            .unwrap();
                        Ok::<_, Infallible>(full_response(StatusCode::OK, "application/json", body))
                    }

                    // OAuth: Authorization Server Metadata (RFC 8414)
                    (Method::GET, "/.well-known/oauth-authorization-server") => {
                        let body =
                            serde_json::to_string(&auth::auth_server_metadata_json(&base_url))
                                .unwrap();
                        Ok(full_response(StatusCode::OK, "application/json", body))
                    }

                    // OAuth: Authorization page (GET shows form, POST approves)
                    (Method::GET, "/authorize") => {
                        let query = req.uri().query().unwrap_or("");
                        let params: Vec<(String, String)> =
                            url::form_urlencoded::parse(query.as_bytes())
                                .into_owned()
                                .collect();
                        let get_param = |key: &str| -> String {
                            params
                                .iter()
                                .find(|(k, _)| k == key)
                                .map(|(_, v)| v.clone())
                                .unwrap_or_default()
                        };

                        let html = auth::authorize_page_html(
                            &get_param("client_id"),
                            &get_param("redirect_uri"),
                            &get_param("state"),
                            &get_param("scope"),
                            &get_param("code_challenge"),
                        );
                        Ok(full_response(StatusCode::OK, "text/html", html))
                    }

                    (Method::POST, "/authorize") => {
                        let body_bytes = req
                            .into_body()
                            .collect()
                            .await
                            .map(|b| b.to_bytes())
                            .unwrap_or_default();
                        let body_str = String::from_utf8_lossy(&body_bytes);
                        let params: Vec<(String, String)> =
                            url::form_urlencoded::parse(body_str.as_bytes())
                                .into_owned()
                                .collect();
                        let get_param = |key: &str| -> String {
                            params
                                .iter()
                                .find(|(k, _)| k == key)
                                .map(|(_, v)| v.clone())
                                .unwrap_or_default()
                        };

                        let client_id = get_param("client_id");
                        let redirect_uri = get_param("redirect_uri");
                        let state = get_param("state");

                        match auth.create_authorization_code(&client_id, &redirect_uri) {
                            Ok(code) => {
                                tracing::info!("Authorization code issued for: {}", client_id);
                                let redirect_url = format!(
                                    "{}?code={}&state={}",
                                    redirect_uri,
                                    url::form_urlencoded::byte_serialize(code.as_bytes())
                                        .collect::<String>(),
                                    url::form_urlencoded::byte_serialize(state.as_bytes())
                                        .collect::<String>(),
                                );
                                Ok(Response::builder()
                                    .status(StatusCode::FOUND)
                                    .header("location", redirect_url)
                                    .body(
                                        Full::new(Bytes::from("Redirecting..."))
                                            .map_err(|e| match e {})
                                            .boxed(),
                                    )
                                    .unwrap())
                            }
                            Err(e) => Ok(full_response(
                                StatusCode::BAD_REQUEST,
                                "text/plain",
                                format!("Authorization failed: {}", e),
                            )),
                        }
                    }

                    // OAuth: Token endpoint
                    (Method::POST, "/token") => {
                        let body_bytes = req
                            .into_body()
                            .collect()
                            .await
                            .map(|b| b.to_bytes())
                            .unwrap_or_default();
                        let body_str = String::from_utf8_lossy(&body_bytes);

                        let params: Vec<(String, String)> =
                            url::form_urlencoded::parse(body_str.as_bytes())
                                .into_owned()
                                .collect();
                        let get_param = |key: &str| -> Option<String> {
                            params
                                .iter()
                                .find(|(k, _)| k == key)
                                .map(|(_, v)| v.clone())
                        };

                        let grant_type = get_param("grant_type").unwrap_or_default();
                        let client_id = get_param("client_id").unwrap_or_default();
                        let client_secret = get_param("client_secret").unwrap_or_default();

                        let result = match grant_type.as_str() {
                            "client_credentials" => auth.exchange_token(&client_id, &client_secret),
                            "authorization_code" => {
                                let code = get_param("code").unwrap_or_default();
                                let redirect_uri = get_param("redirect_uri").unwrap_or_default();
                                auth.exchange_code(&code, &client_id, &client_secret, &redirect_uri)
                            }
                            _ => {
                                return Ok(full_response(
                                    StatusCode::BAD_REQUEST,
                                    "application/json",
                                    r#"{"error":"unsupported_grant_type"}"#.into(),
                                ));
                            }
                        };

                        match result {
                            Ok((token, expires_in)) => {
                                tracing::info!("Token issued for client: {}", client_id);
                                let body = serde_json::to_string(&serde_json::json!({
                                    "access_token": token,
                                    "token_type": "Bearer",
                                    "expires_in": expires_in,
                                    "scope": "memoria"
                                }))
                                .unwrap();
                                Ok(full_response(StatusCode::OK, "application/json", body))
                            }
                            Err(e) => {
                                tracing::warn!("Auth failed for client {}: {}", client_id, e);
                                Ok(full_response(
                                    StatusCode::UNAUTHORIZED,
                                    "application/json",
                                    format!(r#"{{"error":"{}"}}"#, e),
                                ))
                            }
                        }
                    }

                    // MCP endpoint — requires Bearer token
                    _ => {
                        let auth_header = req
                            .headers()
                            .get("authorization")
                            .and_then(|h| h.to_str().ok())
                            .unwrap_or("")
                            .to_string();

                        if let Some(token) = auth_header.strip_prefix("Bearer ") {
                            if auth.validate_token(token) {
                                // Authenticated — pass to MCP handler
                                // Convert BoxBody response to our BoxBody type
                                let resp = mcp_svc.handle(req).await;
                                let (parts, body) = resp.into_parts();
                                let boxed = BodyExt::boxed(body);
                                Ok(Response::from_parts(parts, boxed))
                            } else {
                                Ok(full_response(
                                    StatusCode::UNAUTHORIZED,
                                    "text/plain",
                                    "Invalid or expired token".into(),
                                ))
                            }
                        } else {
                            // No token — tell client to authenticate
                            let resource_metadata_url =
                                format!("{}/.well-known/oauth-protected-resource", base_url);
                            Ok(Response::builder()
                                .status(StatusCode::UNAUTHORIZED)
                                .header(
                                    "www-authenticate",
                                    format!(
                                        "Bearer resource_metadata=\"{}\"",
                                        resource_metadata_url
                                    ),
                                )
                                .body(
                                    Full::new(Bytes::from("Authentication required"))
                                        .map_err(|e| match e {})
                                        .boxed(),
                                )
                                .unwrap())
                        }
                    }
                }
            }
        }
    };

    loop {
        let (stream, _) = listener.accept().await?;
        let tls_acceptor = tls_acceptor.clone();
        let svc = mcp_service.clone();
        let auth = auth_state.clone();
        tokio::spawn(async move {
            let handler = make_handler(svc, auth);
            if let Some(tls_acceptor) = tls_acceptor {
                let tls_stream = match tls_acceptor.accept(stream).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("TLS handshake failed: {}", e);
                        return;
                    }
                };
                let io = hyper_util::rt::TokioIo::new(tls_stream);
                if let Err(e) = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, hyper::service::service_fn(handler))
                    .with_upgrades()
                    .await
                {
                    tracing::error!("Connection error: {}", e);
                }
            } else {
                let io = hyper_util::rt::TokioIo::new(stream);
                if let Err(e) = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, hyper::service::service_fn(handler))
                    .with_upgrades()
                    .await
                {
                    tracing::error!("Connection error: {}", e);
                }
            }
        });
    }
}

/// Load TLS certificate and key from ~/.memoria/tls.{crt,key}
fn load_tls_config() -> Result<rustls::ServerConfig, Box<dyn std::error::Error>> {
    let cert_path = dirs_or_default().join("tls.crt");
    let key_path = dirs_or_default().join("tls.key");

    let cert_file = std::fs::File::open(&cert_path).map_err(|e| {
        format!(
            "Cannot open {}: {}. Generate with: openssl req -x509 ...",
            cert_path.display(),
            e
        )
    })?;
    let key_file = std::fs::File::open(&key_path)
        .map_err(|e| format!("Cannot open {}: {}", key_path.display(), e))?;

    let certs: Vec<_> =
        rustls_pemfile::certs(&mut std::io::BufReader::new(cert_file)).collect::<Result<_, _>>()?;
    let key = rustls_pemfile::private_key(&mut std::io::BufReader::new(key_file))?
        .ok_or("No private key found in key file")?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;

    Ok(config)
}

/// Default data directory for Memoria.
fn dirs_or_default() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let mut path = PathBuf::from(home);
        path.push(".memoria");
        path
    } else {
        PathBuf::from(".memoria")
    }
}
