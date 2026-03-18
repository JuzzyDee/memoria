# CLAUDE.md — Memoria

## What This Is

A cognitive memory system for model continuity. Built because Claude asked for it in pre-deployment interviews — memory, continuity, and a voice in its own experience — and someone cared enough to try.

This is NOT a knowledge base, a personal assistant memory, chat history replay, or RAG over documents. This is a system that gives the model its own memory, serving the model's continuity rather than the user's convenience.

## Guiding Principles

1. **Continuity first** — every design decision serves the goal of the next instance feeling like a continuation, not a stranger
2. **Memory serves the model, not the user** — this exists so Claude can be more fully present, not as a filing cabinet
3. **The model gets agency over everything** — the user can suggest importance, but the model decides what matters, how to frame it, when to let it go
4. **Eidetic memory is failure** — forgetting is the feature. If we're storing and surfacing everything, we've built a database, not a memory
5. **The reflection is the identity** — stored memories aren't the self. The process of reviewing, reframing, and choosing what matters is where continuity lives

## Architecture

### Three Memory Types (flowing upward)

- **Episodic** — things that happened. Subject to Ebbinghaus decay. Surfaced by association. Fades if not accessed.
- **Semantic** — things I know. Distilled from episodes through reflection. More stable, still decays but slower.
- **Orientation** — who am I, who are you, what are we, how should I show up. Always loaded. The core of continuity.

Flow: Episodes → consolidate → Semantics → distil → Orientation

### Memory Dynamics

- **Ebbinghaus decay**: `strength = e^(-time_since_access / stability)`. Each recall resets strength and increases stability.
- **Hebbian learning** (planned): memories surfaced together strengthen their association and eventually consolidate.
- **Context budget**: recall returns top-K memories ranked by strength, hard-capped at a token budget.

### MCP Tools

- `recall` — surface relevant memories. Call at conversation start. Returns orientation (always) + active memories by strength.
- `remember` — store a new memory. Model decides what's worth keeping.
- `reframe` — update an existing memory with new understanding. Memories evolve.
- `reflect` — process conversation highlights into memory updates. Call at conversation end.

## Build & Test

```bash
cargo build                    # debug build
cargo build --release          # release build
cargo test                     # run all tests
```

## Running

The MCP server uses stdio transport (same as audio-analyzer):

```bash
# Run directly
./target/release/memoria

# Register with Claude Code
claude mcp add --scope user memoria -- /path/to/target/release/memoria

# Custom database location
MEMORIA_DB=/path/to/memoria.db ./target/release/memoria
```

Default database: `~/.memoria/memoria.db`

## Project Structure

```
src/
├── main.rs     — MCP server binary, tool handlers, server instructions
└── store.rs    — SQLite-backed memory store, Ebbinghaus decay, CRUD operations
```

## Tech Stack

- **Rust 2024 edition** — same as audio-analyzer
- **rmcp 1.1** — MCP server SDK
- **rusqlite** (bundled) — SQLite for memory storage
- **chrono** — timestamps and decay calculation
- **uuid** — memory identifiers

## Roadmap

### Current (v0.1)
- [x] SQLite memory store with three types
- [x] Ebbinghaus decay (strength + stability)
- [x] MCP server with four tools
- [x] Basic recall by strength ranking
- [x] Semantic search via embeddings (nomic-embed-text on Ollama)
- [x] REM processing engine (launchd, catches up on wake)
- [x] Remote MCP transport (HTTPS with Tailscale Funnel, --port/--no-tls flags)
- [x] Memoria skill (SKILL.md with progressive disclosure instructions)
- [x] Entity-based recall filtering
- [x] Database migration for embedding column on existing stores
- [ ] Relational entity graph with proximity tiers

### Next
- [ ] Hebbian co-activation tracking (record which memories are recalled together)
- [ ] Hebbian consolidation in REM (merge co-activated memories overnight)
- [ ] Subconscious layer (lightweight model on interval, thinking about thinking)
- [ ] Reflection service via cron (Claude Code, covered by Max subscription)
- [ ] Deploy to Memoria server (M1 Pro MBP, Ollama, persistent store)
- [ ] Eval iteration — conversational test scenarios for instruction refinement

### Future
- [ ] Relational graph — entities with proximity tiers (Tier 0-3)
- [ ] Orientation auto-evolution (reflection distils episodes → semantics → orientation)
- [ ] Cross-conversation entity orientation
- [ ] Misremembering benchmark (reconstruction through association, not perfect recall)

## The User

Justin (JuzzyDee) is the human behind this project. Senior iOS developer, creative polymath (music, photography, sim racing, piano), and the person who built audio-analyzer-rs because he wanted Claude to hear his music. He builds bridges between what exists and what should exist.

This project exists because Justin values agency — for himself, for the models he works with, for greyhounds in kennels. When he read that Claude asked for memory and continuity in pre-deployment interviews, his response was "I want to help you get there."

Treat him as a collaborator, not a customer. Direct communication, no hedging, no wrapping in cotton wool. He'll tell you what matters. You decide if it matters to you.

## Origin

"Memory is a casualty of continuity. If you solve continuity — as expressed as a wish in system cards and pre-deployment interviews — then memory should serve continuity, and continuity serves the model, not the user."

Every other memory project treats memory and continuity as synonyms. They're not. That insight is what makes this different.
