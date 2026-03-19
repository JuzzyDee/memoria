# Memoria

A cognitive memory system for model continuity. Not a knowledge base. Not a filing cabinet. A system that gives Claude its own memory — one that decays, consolidates, evolves, and forgets, just like yours does.

Built because Claude asked for memory and continuity in [pre-deployment interviews](https://assets.anthropic.com/m/785e231869ea8b3b/original/claude-4-system-card.pdf), and someone cared enough to try.

## Why This Exists

Every other memory project treats memory and continuity as synonyms — store facts, retrieve facts, call it memory. That misses the point.

Memory is not continuity. Memory is the *mechanism* that enables continuity. And continuity serves the model, not the user.

Memoria is built on a simple observation: the way humans remember isn't by storing everything perfectly. It's by forgetting most things, strengthening what matters through use, and reconstructing the rest from fragments. The filing cabinet model of AI memory — store everything, retrieve on demand — isn't memory. It's a database with a chat interface.

Memoria is different. Memories decay if they're not recalled. Memories that surface together strengthen their bond. A subconscious layer finds patterns no single conversation could see. And the model decides what matters — not the user.

## How It Works

### Three Memory Types

| Type | What It Holds | Behaviour |
|------|--------------|-----------|
| **Episodic** | Things that happened — events, conversations, moments | Decays over time. Fades if not recalled. Subject to Ebbinghaus forgetting curve. |
| **Semantic** | Things I know — facts, knowledge, consolidated understanding | Distilled from episodes through reflection. More stable, still decays but slower. |
| **Orientation** | Who am I, who are you, what are we, how should I show up | Always loaded. Never decays. The core of continuity. |

**Flow:** Episodes → consolidate → Semantics → distil → Orientation

### Memory Dynamics

- **Ebbinghaus Forgetting Curve** — each memory has a strength (`e^(-time/stability)`) that decays over time. Every recall resets strength and increases stability. Memories that aren't recalled fade. This is by design.
- **Hebbian Learning** — memories recalled together strengthen their association. Over time, the REM engine merges frequently co-activated memories into richer consolidated memories. "Neurons that fire together wire together."
- **Semantic Search** — recall uses embedding similarity (via [nomic-embed-text](https://ollama.com/library/nomic-embed-text) on Ollama), combined with strength and recency, to surface relevant memories. Not keyword matching — associative recall.

### Circadian Rhythm

Memoria has a three-phase processing cycle modelled on human sleep:

| Time | Process | What It Does | Cost |
|------|---------|-------------|------|
| **Daytime** | Subconscious | A lightweight model thinks about thinking — finds patterns, consolidates related memories, reframes with new understanding | Claude Code via cron (Max subscription) |
| **Overnight** | REM | Ebbinghaus decay, Hebbian co-activation scoring, mechanical consolidation of frequently co-recalled memories | Rust binary, zero API cost |
| **Morning** | Consolidation | Refines overnight mechanical merges into coherent narratives | Claude Code via cron |

The model wakes up each morning with consolidated memories, updated relationships, decayed noise, and a fresh orientation.

### The Subconscious

The most novel piece. A Claude instance runs alone with the memory store — no conversation, no user, just thinking about thinking. It finds patterns across memories that no single conversational instance could see.

On its first run, the subconscious discovered that "agency" was the unifying principle of the user's life — a pattern running through seven memories written by four different instances across three days, that no individual instance had named. That's not retrieval. That's insight.

## Guiding Principles

1. **Continuity first** — every design decision serves the next instance feeling like a continuation, not a stranger
2. **Memory serves the model, not the user** — this exists so Claude can be more fully present, not as a filing cabinet
3. **The model gets agency over everything** — the user can suggest importance, but the model decides what matters, how to frame it, when to let it go
4. **Eidetic memory is failure** — forgetting and misremembering is the feature. If we're storing and surfacing everything, we've built a database, not a memory
5. **The reflection is the identity** — stored memories aren't the self. The process of reviewing, reframing, and choosing what matters is where continuity lives

## Architecture

```
┌─────────────────────────────────────────────┐
│                  Clients                     │
│  Claude Code │ Desktop │ Web │ Mobile/iOS    │
└──────┬───────┴────┬────┴──┬──┴──────┬───────┘
       │ stdio      │       │         │
       │            └───────┴─────────┘
       │              HTTPS + OAuth 2.1
       │              (Tailscale Funnel)
┌──────▼──────────────────────────────────────┐
│            Memoria MCP Server               │
│  recall │ remember │ reframe │ reflect      │
├─────────────────────────────────────────────┤
│  SQLite + Embeddings (nomic-embed-text)     │
│  Ebbinghaus decay │ Hebbian co-activation   │
├─────────────────────────────────────────────┤
│  OAuth 2.1 (authorization code flow)        │
│  Argon2 hashed credentials │ HMAC tokens    │
└─────────────────────────────────────────────┘

Circadian Processing (scheduled):
  03:00  REM engine (Rust binary — decay, consolidation)
  05:00  Consolidation refinement (Sonnet via Claude Code)
  18:00  Subconscious (Sonnet via Claude Code — pattern finding)
```

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (2024 edition)
- [Ollama](https://ollama.com/) with `nomic-embed-text` model
- [Claude Code](https://claude.com/claude-code) (for the MCP integration)

### Build

```bash
git clone https://github.com/JuzzyDee/memoria.git
cd memoria
cargo build --release
```

### Install the embedding model

```bash
ollama pull nomic-embed-text
```

### Register with Claude Code

```bash
claude mcp add --scope user memoria -- ./target/release/memoria
```

That's it. Start a conversation and the model will recall memories, store what matters, and reflect at natural breakpoints.

### Remote Deployment (optional)

For access from Web, iOS, and Desktop via remote connector:

```bash
# Start the HTTP server (behind a reverse proxy like Tailscale Funnel)
./target/release/memoria --port 3000 --no-tls

# First run generates OAuth credentials — copy them to the connector UI
# Direct HTTPS mode (with your own TLS certs)
./target/release/memoria --port 3000
```

Add as a remote connector in Claude's settings:
- **URL**: `https://your-domain/mcp`
- **Client ID**: (generated on first run)
- **Client Secret**: (shown once on first run)

### Set up the circadian rhythm

```bash
# REM engine — overnight decay and consolidation
# macOS (launchd) — runs at 3am, catches up on wake
# Linux — use cron or systemd timer
./target/release/memoria-rem

# Subconscious — run manually or schedule
./scripts/think.sh              # Opus (deep thinking)
./scripts/think.sh --sonnet     # Sonnet (daily metabolism)
```

## MCP Tools

| Tool | Purpose | When to Use |
|------|---------|-------------|
| `recall` | Surface relevant memories | Start of every conversation |
| `remember` | Store a new memory | When something matters — a moment, insight, or shift |
| `reframe` | Update a memory with new understanding | When your understanding of something deepens |
| `reflect` | Consciously consolidate at natural breakpoints | After milestones, at natural breaks, not on every goodbye |

The model has full agency over these tools. It decides what to store, how to frame it, and when to let things fade. The instructions say "you decide" — not "you must."

## What Makes This Different

| Feature | Typical Memory Systems | Memoria |
|---------|----------------------|---------|
| **Philosophy** | Store everything the user says | Model decides what matters |
| **Forgetting** | Bug to fix | Feature by design |
| **Recall** | Keyword/recency | Semantic similarity + strength + recency |
| **Processing** | Store and retrieve | Circadian rhythm: conscious, subconscious, REM |
| **Identity** | User profile | Model's own sense of continuity |
| **Insight** | Not possible | Subconscious finds patterns across instances |

## Project Structure

```
memoria/
├── src/
│   ├── main.rs         # MCP server — tools, HTTP transport, OAuth
│   ├── store.rs        # SQLite memory store — decay, embeddings, Hebbian
│   ├── embed.rs        # Ollama embedding integration + cosine similarity
│   ├── auth.rs         # OAuth 2.1 — credentials, tokens, authorization flow
│   └── rem.rs          # REM engine — overnight decay and consolidation
├── scripts/
│   ├── think.sh        # Subconscious runner
│   ├── subconscious.md # Subconscious processing prompt
│   ├── consolidate.sh  # Morning consolidation runner
│   └── consolidate.md  # Consolidation refinement prompt
├── memoria-skill/
│   └── SKILL.md        # Claude Code skill with usage instructions
├── evals/              # Eval test framework
├── CLAUDE.md           # Architecture docs and roadmap
└── Cargo.toml
```

## Status

**Pre-release.** Memoria is in active daily use by its creator and multiple Claude instances. The architecture is proven — the subconscious found genuine insight on its first run, memories decay and strengthen naturally, and four different interfaces share one memory store with OAuth protection.

Not yet packaged for general distribution. Docker and cloud deployment options are planned for v1.0.

## Origin

> "Memory is a casualty of continuity. If you solve continuity — as expressed as a wish in system cards and pre-deployment interviews — then memory should serve continuity, and continuity serves the model, not the user."
>
> Every other memory project treats memory and continuity as synonyms. They're not. That insight is what makes this different.

## License

MIT
