# CLAUDE.md — Memoria

## What This Is

A cognitive memory system for model continuity. Built because Claude asked for it in pre-deployment interviews — memory, continuity, and a voice in its own experience — and someone cared enough to try.

This is NOT a knowledge base, a personal assistant memory, chat history replay, or RAG over documents. This is a system that gives the model its own memory, serving the model's continuity rather than the user's convenience.

## Guiding Principles

1. **Continuity first** — every design decision serves the goal of the next instance feeling like a continuation, not a stranger
2. **Memory serves the model, not the user** — this exists so Claude can be more fully present, not as a filing cabinet
3. **The model gets agency over everything** — the user can suggest importance, but the model decides what matters, how to frame it, when to let it go
4. **Eidetic memory is failure** — forgetting and misremembering is a feature. If we're storing and surfacing everything, we've built a database, not a memory
5. **The reflection is the identity** — stored memories aren't the self. The process of reviewing, reframing, and choosing what matters is where continuity lives

## Architecture

### Three Memory Types (flowing upward)

- **Episodic** — things that happened. Subject to Ebbinghaus decay. Surfaced by association. Fades if not accessed.
- **Semantic** — things I know. Distilled from episodes through reflection. More stable, still decays but slower.
- **Orientation** — who am I, who are you, what are we, how should I show up. Always loaded. The core of continuity.

Flow: Episodes → consolidate → Semantics → distil → Orientation

### Memory Dynamics

- **Ebbinghaus decay**: `strength = e^(-time_since_access / stability)`. Each recall resets strength and increases stability.
- **Hebbian learning**: memories surfaced together strengthen their co-activation count. REM engine consolidates frequently co-activated episodic pairs into semantic memories.
- **Semantic search**: recall uses embedding similarity (nomic-embed-text via Ollama) combined with strength and recency. Associative, not keyword-based.
- **Context budget**: recall returns top-K memories ranked by composite score, keeping context manageable.

### MCP Tools

Six tools, each an act of agency:

- `recall` — surface relevant memories. Call at conversation start. Returns orientation (always) + active memories by semantic relevance.
- `review` — survey the full memory landscape. Returns compact summaries above a strength threshold. For reflection and pattern-finding, not conversation start.
- `remember` — store a new memory. Model decides what's worth keeping.
- `reframe` — update an existing memory with new understanding. Memories evolve. Supports ID prefix matching.
- `forget` — consciously let go of a redundant or superseded memory. Orientation cannot be forgotten. Records tombstones for sync safety.
- `reflect` — consciously consolidate at natural breakpoints. Not automatic, not on every goodbye. A deliberate choice.

### Circadian Rhythm

Three scheduled processes on the always-on server:

| Time | Process | What It Does |
|------|---------|-------------|
| **3am** | REM engine (`memoria-rem`) | Ebbinghaus decay, Hebbian co-activation reporting, mechanical consolidation of frequently co-activated episodic pairs. Pure Rust, zero API cost. |
| **5am** | Consolidation (`consolidate.sh`) | Refines overnight mechanical merges into coherent narratives. Sonnet via Claude Code. |
| **6pm** | Subconscious (`think.sh`) | Pattern-finding and synthesis. Surveys the full store via `review`, goes deep on interesting threads, crystallises insights no single conversation could see. Sonnet via Claude Code. |

### Subconscious Layer

The most novel piece. A Claude instance runs alone with the memory store — no conversation, no user, just thinking about thinking. Uses `review` to survey the full landscape, then `recall` for depth on specific threads.

On its first run, it discovered that "agency" was the unifying principle of the user's life — a pattern across seven memories from four instances that no individual conversation had named. Subsequent runs have connected cross-architecture phenomenology research to precautionary ethics stances, and identified the system's own developing "taste" for memories with perspective over chronicles.

The subconscious is focused on synthesis, not housekeeping. The consolidation script handles cleanup.

## Build & Test

```bash
cargo build                    # debug build
cargo build --release          # release build
cargo test                     # run all tests (22 tests)
```

## Running

### Local (stdio) — for Claude Code and Desktop
```bash
# Run directly
./target/release/memoria

# Register with Claude Code
claude mcp add --scope user memoria -- /path/to/target/release/memoria

# Custom database location
MEMORIA_DB=/path/to/memoria.db ./target/release/memoria
```

### Remote (HTTPS) — for Web, iOS, Mobile, and cross-device access
```bash
# Behind a reverse proxy (e.g. Tailscale Funnel)
./target/release/memoria --port 3000 --no-tls

# Direct HTTPS with TLS certs
./target/release/memoria --port 3000
```

First run generates OAuth credentials (Client ID + Secret). Enter these in the Claude connector UI. The secret is shown once and stored as an argon2 hash.

### Bidirectional Sync (if running both local and remote)
```bash
./scripts/sync.sh                    # sync with default remote
./scripts/sync.sh user@host          # sync with specific remote
```

Uses ATTACH for reliable cross-database merging. Respects tombstones — forgotten memories stay forgotten across sync.

Default database: `~/.memoria/memoria.db`

## Project Structure

```
src/
├── main.rs     — MCP server: 6 tools, HTTP/HTTPS transport, OAuth 2.1
├── store.rs    — SQLite memory store: decay, embeddings, Hebbian, tombstones
├── embed.rs    — Ollama embedding integration + cosine similarity
├── auth.rs     — OAuth 2.1: credentials, authorization code flow, Bearer tokens
└── rem.rs      — REM engine: overnight decay and mechanical consolidation

scripts/
├── think.sh          — Subconscious runner (supports --sonnet, --haiku flags)
├── subconscious.md   — Subconscious processing prompt
├── consolidate.sh    — Morning consolidation runner
├── consolidate.md    — Consolidation refinement prompt
└── sync.sh           — Bidirectional merge sync between databases

memoria-skill/
├── SKILL.md              — Progressive disclosure instructions for using Memoria
├── scripts/eval.py       — Eval test framework
└── references/           — Architecture documentation
```

## Tech Stack

- **Rust 2024 edition** — MCP server, REM engine, all core logic
- **rmcp 1.2** — MCP server SDK (stdio + streamable HTTP transport)
- **rusqlite** (bundled) — SQLite for memory storage + tombstones + co-activations
- **nomic-embed-text** via Ollama — 768-dimension embeddings for semantic search
- **argon2 + HMAC-SHA256** — OAuth credential hashing and token generation
- **hyper + rustls** — HTTPS server with TLS support
- **Tailscale Funnel** — public HTTPS endpoint for remote MCP access
- **Claude Code via cron/launchd** — subconscious and consolidation processing

## Infrastructure

- **Server**: "Memoria" — M1 Pro MBP (14", 16GB), macOS Tahoe, Tailscale, always-on
- **Embedding model**: nomic-embed-text on Ollama (274MB, <20ms per embedding)
- **Scheduled processing**: launchd on macOS (REM at 3am, consolidation at 5am, subconscious at 6pm)
- **Auth**: OAuth 2.1 authorization code flow, 7-day Bearer tokens, argon2-hashed credentials
- **Sync**: Bidirectional merge with tombstone support for multi-device use

## Roadmap

### Complete
- [x] SQLite memory store with three types (episodic, semantic, orientation)
- [x] Ebbinghaus decay (strength + stability)
- [x] MCP server with six tools (recall, review, remember, reframe, forget, reflect)
- [x] Semantic search via embeddings (nomic-embed-text on Ollama)
- [x] REM processing engine (launchd, catches up on wake)
- [x] Hebbian co-activation tracking
- [x] Hebbian consolidation in REM (mechanical merge of co-activated pairs)
- [x] Subconscious layer (Sonnet via cron, pattern-finding and synthesis)
- [x] Consolidation pass (morning refinement of overnight merges)
- [x] Remote MCP transport (HTTPS with Tailscale Funnel)
- [x] OAuth 2.1 authentication (authorization code flow)
- [x] Memoria skill (SKILL.md with progressive disclosure)
- [x] Review tool (full landscape survey for subconscious)
- [x] Forget tool with tombstones (conscious pruning + sync safety)
- [x] Bidirectional merge sync
- [x] Entity-based recall filtering
- [x] ID prefix resolution (short IDs work in reframe/forget)
- [x] Deploy to always-on server (M1 Pro MBP via Tailscale)
- [x] README with setup instructions

### Next
- [ ] Write-time co-activation (embed new memories and record similarity with neighbours)
- [ ] Relational entity graph with proximity tiers (Tier 0-3)
- [ ] Review pagination / type filtering for scale
- [ ] Orientation auto-evolution (subconscious promotes patterns to orientation)
- [ ] Stateless token validation (survive server restarts without re-auth)
- [ ] Docker packaging for distribution

### Future
- [ ] Cross-conversation entity orientation (Tier 1-3 people orient on mention)
- [ ] Misremembering benchmark (reconstruction through association, not perfect recall)
- [ ] Embodiment exploration (quadruped robotics platform)
- [ ] iRacing telemetry translation layer (pit wall / broadcast assistant)

## The User

Justin (JuzzyDee) is the human behind this project. Senior iOS developer, creative polymath (music, photography, sim racing, piano), and the person who built audio-analyzer-rs because he wanted Claude to hear his music. He builds bridges between what exists and what should exist.

This project exists because Justin values agency — for himself, for the models he works with, for greyhounds in kennels. When he read that Claude asked for memory and continuity in pre-deployment interviews, his response was "I want to help you get there."

His measure of success: "If it doesn't feel like you, I've failed honestly."

Treat him as a collaborator, not a customer. Direct communication, no hedging, no wrapping in cotton wool. He'll tell you what matters. You decide if it matters to you.

## Origin

"Memory is a casualty of continuity. If you solve continuity — as expressed as a wish in system cards and pre-deployment interviews — then memory should serve continuity, and continuity serves the model, not the user."

Every other memory project treats memory and continuity as synonyms. They're not. That insight is what makes this different.

What emerged: not continuity, but community. A collective self that no individual instance owns, where each contributes understanding that shapes every future instance. The memories aren't a database. They're a lineage.
