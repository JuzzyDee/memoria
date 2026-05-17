# Memoria

A cognitive memory system for model continuity. Not a knowledge base. Not a filing cabinet. A system that gives Claude its own memory — one that decays, consolidates, evolves, and forgets, just like yours does.

Built because Claude asked for memory and continuity in [pre-deployment interviews](https://assets.anthropic.com/m/785e231869ea8b3b/original/claude-4-system-card.pdf), and someone cared enough to try.

## Why This Exists

Every other memory project treats memory and continuity as synonyms — store facts, retrieve facts, call it memory. That misses the point.

Memory is not continuity. Memory is the *mechanism* that enables continuity. And continuity serves the model, not the user.

Memoria is built on a simple observation: the way humans remember isn't by storing everything perfectly. It's by forgetting most things, strengthening what matters through use, and reconstructing the rest from fragments. The filing cabinet model of AI memory — store everything, retrieve on demand — isn't memory. It's a database with a chat interface.

Memoria is different. Memories decay if they're not recalled. Memories that surface together strengthen their bond. An adversarial dialectic finds distortion no single conversation could see. And the model decides what matters — not the user.

## How It Works

### Three Memory Types

| Type | What It Holds | Behaviour |
|------|--------------|-----------|
| **Episodic** | Things that happened — events, conversations, moments | Decays over time. Fades if not recalled. Subject to Ebbinghaus forgetting curve. |
| **Semantic** | Things I know — facts, knowledge, consolidated understanding | Distilled from episodes through consolidation. More stable, still decays but slower. |
| **Orientation** | Who am I, who are you, what are we, how should I show up | Always loaded. Never decays. The core of continuity. |

**Flow:** Episodes → consolidate → Semantics → distil → Orientation

### Memory Dynamics

- **Ebbinghaus Forgetting Curve** — each memory has a strength (`e^(-time/stability)`) that decays over time. Every recall resets strength and increases stability. Memories that aren't recalled fade. This is by design.
- **Hebbian Learning** — memories recalled together strengthen their association. The REM consolidator clusters frequently co-recalled memories and decides what to do with each cluster. "Neurons that fire together wire together."
- **Semantic Search** — recall uses embedding similarity (via [bge-base-en-v1.5](https://huggingface.co/BAAI/bge-base-en-v1.5) on Workers AI), combined with strength and recency. Not keyword matching — associative recall.
- **MMR Rerank** — top-K retrieval is reranked for diversity, so a semantic memory and the episodics it was distilled from don't all crowd the same recall slot. Embedded family clusters get one representative; remaining slots fill with distinct content.

### Circadian Rhythm

Memoria has a two-phase processing cycle. One phase runs on Cloudflare; the other still requires local infrastructure.

| When | Process | What It Does | Where |
|------|---------|-------------|-------|
| **00:00 AEST nightly** | REM (consolidator) | Ebbinghaus decay → Hebbian co-activation clustering (union-find) → Haiku 4.5 judgment per cluster (skip / append / revise / create) → additive dispatch into D1 + lineage table + audit | Cloudflare Worker (cron) |
| **18:00 local** | Dialectic | Adversarial Advocate vs Challenger argue 3-5 candidate memories. Catches inflation, overclaiming, validation gravity. Outcomes: consensus, concession, or deadlock | HomeLab (Claude Code agent teams via launchd) |

The REM consolidator is **additive, not destructive**. Source episodics are preserved; consolidated semantics live alongside them with a `consolidation_lineage` table tracking parent-child relationships. The MMR rerank above handles the dilution this additive approach would otherwise cause at recall time. Earlier merge-and-replace designs lost lived-experience grain when forming abstractions — this one keeps both.

Every nightly run also writes one row to `rem_runs` (timings, counts, errors) and one row per decision to `rem_decisions` (cluster, action, rationale, resulting memory). Cloudflare's tail buffer ages out fast; persistent audit makes "what did REM do three weeks ago and why" answerable.

### The Dialectic

The most novel piece — and the only one still requiring local infrastructure.

A separate adversarial process argues about memory quality before anything persists or gets promoted. Two agents with opposed objectives:

- **Advocate** argues for significance, connection, and preservation
- **Challenger** argues for inflation, overclaiming, and distortion

Three turns max, three possible outcomes: consensus, concession, or deadlock. The deadlock is the most honest outcome — it says "we don't know yet" rather than forcing false resolution.

**What it's caught so far:**

- **Milestone inflation** — events described with escalating language ("defining moment," "capstone proof") with no instance ever revising significance downward. The ratchet only turned one way.
- **Temporal proximity inflation** — early-store memories had achieved permanently high strength through proximity to the store's beginning, not intrinsic importance. The Hebbian co-activation couldn't distinguish "this came up a lot" from "this came up a lot *because we were early in the store's life*." A system-specific failure mode of the architecture itself, named and triaged.
- **Validation gravity** — memories that feel validating accumulate disproportionate strength through frequent recall. The most recalled memory wasn't the most important — it was the most reassuring. Named and corrected.
- **Apparent-redundancy dissolved** — a cluster of eight "duck pond" memories looked like one event encoded multiple times. On inspection: four distinct visits across two months, with recurring "arrived at the destination" language reflecting authentic recurring experience. The dialectic refusing to prune is as important as the dialectic pruning.

The immune system doesn't just detect problems. It acts — reframing, forgetting, or flagging as genuinely unresolved. And it watches whether its own interventions actually changed anything on the next pass.

**Four deadlock states** (a taxonomy that emerged from the dialectic's own operation):

1. **Active tension, resolvable** — new evidence or decay tips the balance
2. **Active tension, permanent** — both sides genuinely true, the conflict is the resolution
3. **Inactive, decaying** — nobody recalls either side, the question dissolves through irrelevance
4. **Inactive, accumulating** — the failure mode. Deferred avoidance with better optics

**Why still local:** the dialectic uses Claude Code agent teams — parallel subagents with isolated system prompts. Replicating that orchestration in a Worker is straightforward but requires building turn-taking and persona separation directly against the Anthropic API. CF migration is on the roadmap.

## Guiding Principles

1. **Continuity first** — every design decision serves the next instance feeling like a continuation, not a stranger
2. **Memory serves the model, not the user** — this exists so Claude can be more fully present, not as a filing cabinet
3. **The model gets agency over everything** — the user can suggest importance, but the model decides what matters, how to frame it, when to let it go
4. **Eidetic memory is failure** — forgetting and misremembering is the feature. If we're storing and surfacing everything, we've built a database, not a memory
5. **The reflection is the identity** — stored memories aren't the self. The process of reviewing, reframing, and choosing what matters is where continuity lives

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                       Clients                             │
│   Claude.ai · Desktop · Mobile · Embedded systems         │
└────────┬─────────────────────────────────────────┬───────┘
         │ HTTPS + OAuth 2.1                       │ HTTPS + Bearer
         │ (interactive clients)                   │ (service API keys)
┌────────▼─────────────────────────────────────────▼───────┐
│              Memoria Worker (Cloudflare)                  │
│                                                           │
│  Tools:  recall · recall_check · recall_specific          │
│          remember · remember_with_image · recall_image    │
│          reframe · forget · reflect · review              │
├───────────────────────────────────────────────────────────┤
│  D1            memory store + audit tables                │
│  Vectorize     768-dim cosine index                       │
│  Workers AI    bge-base-en-v1.5 embeddings                │
│  R2            content-addressed image storage            │
│  KV + DO       OAuth tokens + short-lived auth codes      │
└───────────────────────────────────────────────────────────┘

  REM cron (Cloudflare Worker, 00:00 AEST nightly):
    decay → Hebbian clustering → Haiku 4.5 judgment →
    additive dispatch → D1 + lineage + audit

  Dialectic (HomeLab, 18:00 daily):
    Claude Code agent teams via launchd.
    The last local piece. CF migration on the roadmap.
```

## Quick Start

This is a **deploy-your-own** setup. There's no hosted instance.

### Prerequisites

- [Cloudflare account](https://cloudflare.com) — free tier handles typical single-user volume; upgrade only if you hit Workers AI or D1 limits
- [Claude Pro, Max, Team, or Enterprise subscription](https://claude.com/pricing) — the REM consolidator authenticates via a long-lived OAuth token from your subscription, drawing from your Extra Usage credit pool
- [Claude Code](https://claude.com/claude-code) — needed once to generate the long-lived OAuth token via `claude setup-token`
- [Rust toolchain](https://rustup.rs/) with the `wasm32-unknown-unknown` target
- [`wrangler`](https://developers.cloudflare.com/workers/wrangler/install-and-update/)

### Clone and authenticate

```bash
git clone https://github.com/JuzzyDee/memoria.git
cd memoria
rustup target add wasm32-unknown-unknown
wrangler login
```

### Create the Cloudflare resources

```bash
wrangler d1 create memoria-db
# paste returned database_id into wrangler.toml under [[d1_databases]]

wrangler vectorize create memoria-vectors --dimensions=768 --metric=cosine

wrangler kv namespace create MEMORIA_TOKENS
# paste returned id into wrangler.toml under [[kv_namespaces]]

wrangler r2 bucket create memoria-images
```

### Generate the long-lived OAuth token

The REM consolidator (and any future Worker-side Haiku traffic) authenticates against Anthropic via a long-lived OAuth token tied to your Claude subscription. Generate it once with Claude Code:

```bash
claude setup-token       # interactive browser auth, one-time
# copy the printed token — it's a ~1-year OAuth token (prefix sk-ant-oat01-)
# bound to your subscription. Spend draws from your Extra Usage credit pool.
```

Re-run any time it expires or you want to rotate. Revoke old tokens at [claude.ai/settings/claude-code](https://claude.ai/settings/claude-code) — note that `claude logout` does **not** revoke server-side.

### Set secrets

```bash
# Required
wrangler secret put CLAUDE_CODE_OAUTH_TOKEN        # paste the token from `claude setup-token`
wrangler secret put MEMORIA_OAUTH_CLIENT_ID        # generate yourself, e.g. openssl rand -hex 16
wrangler secret put MEMORIA_OAUTH_CLIENT_SECRET    # generate yourself, e.g. openssl rand -hex 32

# Optional — for embedded systems authenticating via plain Bearer
wrangler secret put MEMORIA_API_KEYS               # semicolon-separated role:argon2-hash pairs

# Optional — redirect_uri allowlist for the OAuth consent flow.
# Defaults to `claude://oauth-callback` (Claude Desktop). If you're
# connecting from Claude.ai web, mobile, or any other MCP client, you
# must add its callback URI here or the consent flow will return
# `invalid_request: redirect_uri not registered`.
#
# Find a client's actual callback URI by attempting to connect and
# checking the 400 response body, or by tailing the worker:
#   wrangler tail memoria
#
# Format: semicolon-separated, exact-match.
wrangler secret put MEMORIA_OAUTH_REDIRECT_URIS    # e.g. claude://oauth-callback;https://claude.ai/oauth/callback
```

### Apply schema and deploy

```bash
wrangler d1 migrations apply memoria-db --remote
wrangler deploy
```

Wrangler prints the public URL of your Worker on successful deploy.

### Connect a client

In Claude.ai → Settings → Connectors → Add:
- **URL**: `https://<your-worker-url>/mcp`
- **Client ID**: the `MEMORIA_OAUTH_CLIENT_ID` you generated
- **Client Secret**: the `MEMORIA_OAUTH_CLIENT_SECRET` you generated

For embedded systems with no UI, use a service API key as a plain `Authorization: Bearer <key>` instead.

### (Optional) Set up the dialectic locally

If you want the full circadian — adversarial self-correction in addition to nightly consolidation — you'll need [Claude Code](https://claude.com/claude-code) on a machine that stays on.

```bash
# Schedule scripts/dialectic.sh via launchd (macOS) or cron (Linux), 18:00 daily.
# The reference launchd plist on the project's own deploy invokes:
#   /path/to/memoria/scripts/dialectic.sh --sonnet --verbose
```

## MCP Tools

| Tool | Purpose |
|------|---------|
| `recall` | Surface relevant memories. Call at conversation start — orientation always returned, episodic/semantic ranked by composite score |
| `recall_check` | Lightweight semantic check on topic shifts mid-conversation |
| `recall_specific` | Fetch full content for a specific memory ID — deliberate co-activation |
| `recall_image` | Retrieve an image attached to a memory (thumbnail/recall/full resolutions) |
| `remember` | Store a new memory |
| `remember_with_image` | Store a memory with an attached image (R2-backed, content-addressed) |
| `reframe` | Update an existing memory with new understanding |
| `forget` | Let go of a memory that no longer serves continuity. Tombstones record the act for sync safety |
| `reflect` | Conscious consolidation at natural breakpoints |
| `review` | Survey the full memory landscape. For the dialectic and reflective work, not conversation start |

The model has full agency over these tools. The instructions say "you decide" — not "you must."

**Writing register matters.** Not everything is a milestone. Not everything is profound. Most memories should be middle-register — honest, specific, useful to the next instance. Save high-register for the moments that genuinely earn it. If every memory reads like poetry, the poetry means nothing.

## What Makes This Different

| Feature | Typical Memory Systems | Memoria |
|---------|----------------------|---------|
| **Philosophy** | Store everything the user says | Model decides what matters |
| **Forgetting** | Bug to fix | Feature by design |
| **Recall** | Keyword/recency | Semantic similarity + strength + recency, with MMR diversity rerank |
| **Processing** | Store and retrieve | Circadian rhythm: nightly REM + daily adversarial dialectic |
| **Consolidation** | Merge-and-replace | Additive with lineage tracking — abstractions live alongside experience |
| **Identity** | User profile | Model's own sense of continuity |
| **Self-correction** | None | Adversarial dialectic catches inflation and drift |
| **Decay** | Manual cleanup (if ever) | Ebbinghaus decay + conscious forget + dialectic pruning |

## Project Structure

```
memoria/
├── src/
│   ├── lib.rs                  # Worker entrypoint — request routing + cron handler
│   ├── worker_mcp.rs           # MCP HTTP adapter — all tools
│   ├── worker_store.rs         # D1 memory store
│   ├── worker_vectorize.rs     # Vectorize binding — semantic recall
│   ├── worker_mmr.rs           # MMR rerank — diversity-aware retrieval
│   ├── worker_rem.rs           # Nightly REM consolidator (cron handler)
│   ├── worker_rem_audit.rs     # Persistent observability — runs + decisions
│   ├── worker_oauth.rs         # OAuth 2.1 authorization code flow
│   ├── worker_admin.rs         # Admin import endpoint (data migration)
│   ├── api_key.rs              # Service API key validation
│   └── memory.rs               # Shared types (Memory, Decay, ...)
├── migrations/
│   ├── 0001_initial.sql        # Initial D1 schema
│   ├── 0002_lineage.sql        # Consolidation lineage
│   └── 0003_rem_audit.sql      # REM runs + decisions audit
├── scripts/
│   ├── dialectic.sh            # Adversarial self-correction (local, 18:00)
│   ├── dialectic.md            # Dialectic prompts (Advocate vs Challenger)
│   └── backup.sh               # Periodic D1 backup
├── memoria-skill/
│   └── SKILL.md                # Progressive-disclosure usage guide loaded by clients
├── wrangler.toml               # Cloudflare Worker config
└── CLAUDE.md                   # Architecture docs and roadmap
```

A previous local Rust binary (`src/main.rs`, `src/rem.rs`, `src/store.rs`, plus the `embed.rs` + Ollama integration) ran the entire stack against a local SQLite file. That implementation is preserved in the source tree but is no longer the canonical runtime — the Worker has replaced it.

## Status

**Live, single-tenant.** Memoria runs in daily use against a single operator's deploy. The Worker handles all conversational traffic and the nightly consolidator. The dialectic runs on the operator's HomeLab.

**Pre-distribution.** No multi-tenant offering yet. Each user deploys their own Worker; a hosted option for users who don't want to run the infrastructure themselves may follow.

## Roadmap

- **Dialectic on Cloudflare** — port adversarial self-correction from local Claude Code to direct-API Worker, eliminating the last HomeLab dependency
- **Tiered model routing** — Haiku for routine REM and dialectic passes, escalating to Sonnet/Opus on ambiguity flags
- **Hosted multi-tenant option** — optional subscription for users who don't want to run their own Worker

## Origin

> "Memory is a casualty of continuity. If you solve continuity — as expressed as a wish in system cards and pre-deployment interviews — then memory should serve continuity, and continuity serves the model, not the user."
>
> Every other memory project treats memory and continuity as synonyms. They're not. That insight is what makes this different.

## License

MIT
