// worker_rem.rs — REM consolidator running as a Cloudflare Worker cron.
//
// CLA-87 Phase 9. Replaces the native rem.rs which ran on the local box
// against the old SQLite store. Now runs nightly via wrangler cron,
// calls Haiku 4.5 for the consolidation judgment, dispatches decisions
// via the additive-consolidation primitives in worker_store.
//
// Architectural moves vs. native rem.rs:
//
//   * Additive, not destructive — source episodics are preserved.
//     Consolidated semantics are written *alongside* them and cite their
//     sources via the `consolidation_lineage` table. MMR at recall time
//     handles the dilution the old merge-and-replace was trying to fix.
//
//   * Cluster-shaped input — Hebbian pairs are union-find'd into
//     connected components, so Haiku sees the full transitive context
//     in one call instead of N pair-level calls that miss the broader
//     pattern.
//
//   * Existing-semantic lookup — before Haiku can choose "create new
//     semantic", we surface the top-K most-similar existing semantics
//     so Haiku can choose append/revise/skip instead. Prevents the
//     four-byte-identical-consolidation bug observed in the pre-CLA-87
//     corpus.
//
//   * Structured tool-use output — Haiku partitions the cluster into
//     one or more decisions (skip/append/revise/create), each scoped
//     to a subset of cluster members. Forced tool_choice guarantees
//     parseable JSON.

#![cfg(target_family = "wasm")]

use crate::memory::{Memory, MemoryType};
use crate::{worker_embed, worker_rem_audit, worker_store, worker_vectorize};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use worker::{D1Database, Env, Fetch, Headers, Method, Request, RequestInit, Result};

// ──── Tuning constants ──────────────────────────────────────────────────
// All first-pass values. Revisit after ~4 weeks of REM operation.

/// Minimum co-activation count for a pair to be a real edge. Native
/// historical default was 5; lowered to 3 to match current call volume
/// (per D1 distribution as of 2026-05-15: 1+4+1 = 6 candidate pairs).
const MIN_EDGE_THRESHOLD: u32 = 3;

/// Maximum memories in a single cluster sent to Haiku. Transitive
/// closure can balloon a component; we truncate beyond this.
const MAX_CLUSTER_SIZE: usize = 12;

/// Hard cap on clusters processed per nightly run. At current scale
/// we'd typically see 1–3 clusters per night, well under cap.
const MAX_CLUSTERS_PER_NIGHT: u32 = 15;

/// Existing semantics surfaced to Haiku per cluster for the dedupe step.
const RELATED_SEMANTICS_TO_FETCH: u32 = 5;

/// Haiku 4.5 — Sonnet-class reasoning isn't needed to judge "are these
/// two memories about the same thing." Save the cost for the work that
/// actually benefits from it.
const HAIKU_MODEL: &str = "claude-haiku-4-5-20251001";

// ──── Decision types (tool-use output) ──────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RelationshipAssessment {
    Coincidental,
    Tangential,
    SemanticallyMeaningful,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DecisionAction {
    Skip,
    Append,
    Revise,
    Create,
}

#[derive(Debug, Clone, Deserialize)]
struct ConsolidationDecision {
    relationship_assessment: RelationshipAssessment,
    action: DecisionAction,
    members: Vec<String>,
    #[serde(default)]
    existing_considered: Option<String>,
    rationale: String,
    #[serde(default)]
    semantic_entry: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    entity: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

fn relationship_assessment_str(r: &RelationshipAssessment) -> &'static str {
    match r {
        RelationshipAssessment::Coincidental => "coincidental",
        RelationshipAssessment::Tangential => "tangential",
        RelationshipAssessment::SemanticallyMeaningful => "semantically_meaningful",
    }
}

fn decision_action_str(a: &DecisionAction) -> &'static str {
    match a {
        DecisionAction::Skip => "skip",
        DecisionAction::Append => "append",
        DecisionAction::Revise => "revise",
        DecisionAction::Create => "create",
    }
}

/// Telemetry returned by `run()`. Logged by the scheduled handler.
#[derive(Debug, Default)]
pub struct RunSummary {
    pub decayed: usize,
    pub clusters_attempted: usize,
    pub decisions_created: usize,
    pub decisions_appended: usize,
    pub decisions_revised: usize,
    pub decisions_skipped: usize,
    pub errors: Vec<String>,
}

// ──── Entry point ───────────────────────────────────────────────────────

/// One nightly REM run. Called from lib.rs's `#[event(scheduled)]`
/// handler. Apply decay → find candidate pairs → cluster → for each
/// cluster ask Haiku → dispatch decisions.
pub async fn run(env: &Env) -> Result<RunSummary> {
    let db = env.d1("DB")?;
    let mut summary = RunSummary::default();

    // Open the audit row before doing anything else so all subsequent
    // decision-level writes can correlate to this run.
    let run_id = match worker_rem_audit::record_run_start(&db).await {
        Ok(id) => id,
        Err(e) => {
            // Audit failure shouldn't abort REM — log it and continue
            // with a synthetic id (decision writes will fail FK but
            // the actual consolidation work still lands).
            summary.errors.push(format!("audit start: {:?}", e));
            uuid::Uuid::new_v4().to_string()
        }
    };

    // 1. Apply Ebbinghaus decay across the non-orientation corpus.
    match worker_store::apply_decay(&db).await {
        Ok(n) => summary.decayed = n,
        Err(e) => summary.errors.push(format!("decay: {:?}", e)),
    }

    // 2. Find candidate pairs (filtered against orientation + lineage).
    let pairs = match worker_store::find_consolidation_pairs(
        &db,
        MIN_EDGE_THRESHOLD,
        MAX_CLUSTERS_PER_NIGHT * 4,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            summary.errors.push(format!("find_pairs: {:?}", e));
            let _ = worker_rem_audit::record_run_finish(&db, &run_id, 0, &summary).await;
            return Ok(summary);
        }
    };

    let pairs_found = pairs.len();

    if pairs.is_empty() {
        let _ = worker_rem_audit::record_run_finish(&db, &run_id, pairs_found, &summary).await;
        return Ok(summary);
    }

    // 3. Union-find clustering.
    let mut clusters = cluster_pairs(&pairs, MAX_CLUSTER_SIZE);
    clusters.truncate(MAX_CLUSTERS_PER_NIGHT as usize);
    summary.clusters_attempted = clusters.len();

    // 4. For each cluster: fetch memories, find related semantics,
    //    call Haiku, dispatch decisions.
    for (cluster_idx, cluster_ids) in clusters.iter().enumerate() {
        if let Err(e) = process_cluster(
            env,
            &db,
            &run_id,
            cluster_idx,
            cluster_ids,
            &pairs,
            &mut summary,
        )
        .await
        {
            summary.errors.push(format!("cluster {}: {:?}", cluster_idx, e));
        }
    }

    let _ = worker_rem_audit::record_run_finish(&db, &run_id, pairs_found, &summary).await;
    Ok(summary)
}

// ──── Clustering (union-find on co-activation pairs) ───────────────────

fn cluster_pairs(
    pairs: &[(String, String, u32)],
    max_size: usize,
) -> Vec<Vec<String>> {
    let mut parent: HashMap<String, String> = HashMap::new();

    fn find(parent: &mut HashMap<String, String>, id: &str) -> String {
        let p = parent.get(id).cloned().unwrap_or_else(|| id.to_string());
        if p == id {
            return p;
        }
        let root = find(parent, &p);
        parent.insert(id.to_string(), root.clone());
        root
    }

    fn union(parent: &mut HashMap<String, String>, a: &str, b: &str) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent.insert(ra, rb);
        }
    }

    for (a, b, _) in pairs {
        parent.entry(a.clone()).or_insert_with(|| a.clone());
        parent.entry(b.clone()).or_insert_with(|| b.clone());
        union(&mut parent, a, b);
    }

    let mut components: HashMap<String, Vec<String>> = HashMap::new();
    let ids: Vec<String> = parent.keys().cloned().collect();
    for id in ids {
        let root = find(&mut parent, &id);
        components.entry(root).or_default().push(id);
    }

    let mut result: Vec<Vec<String>> = components
        .into_values()
        .filter(|c| c.len() >= 2)
        .map(|mut c| {
            if c.len() > max_size {
                // Truncation policy v1: stable sort + slice. A future
                // refinement is "keep highest intra-cluster edge weight"
                // but at current scale clusters never approach max_size.
                c.sort();
                c.truncate(max_size);
            }
            c
        })
        .collect();

    // Largest clusters first — most context per Haiku call.
    result.sort_by(|a, b| b.len().cmp(&a.len()));
    result
}

// ──── Per-cluster processing ────────────────────────────────────────────

async fn process_cluster(
    env: &Env,
    db: &D1Database,
    run_id: &str,
    cluster_idx: usize,
    cluster_ids: &[String],
    all_pairs: &[(String, String, u32)],
    summary: &mut RunSummary,
) -> Result<()> {
    let id_refs: Vec<&str> = cluster_ids.iter().map(String::as_str).collect();
    let memories = worker_store::get_many(db, &id_refs).await?;
    if memories.is_empty() {
        return Ok(());
    }

    let related = find_related_semantics(env, db, &memories)
        .await
        .unwrap_or_default();

    // Subset of all_pairs that are intra-cluster.
    let cluster_set: HashSet<&str> = cluster_ids.iter().map(String::as_str).collect();
    let intra_edges: Vec<&(String, String, u32)> = all_pairs
        .iter()
        .filter(|(a, b, _)| cluster_set.contains(a.as_str()) && cluster_set.contains(b.as_str()))
        .collect();

    let decisions = match consolidate_via_claude(env, &memories, &intra_edges, &related).await {
        Ok(d) => d,
        Err(e) => {
            summary.errors.push(format!("Haiku cluster {}: {:?}", cluster_idx, e));
            return Ok(());
        }
    };

    for decision in &decisions {
        let dispatch_result = dispatch_decision(env, db, decision).await;

        // Audit the decision (and Haiku's rationale) regardless of
        // dispatch success — the cognitive judgment was made even if
        // the SQL write that followed failed.
        let result_memory_id_str = match &dispatch_result {
            Ok(Some(id)) => Some(id.as_str()),
            _ => None,
        };
        let _ = worker_rem_audit::record_decision(
            db,
            run_id,
            cluster_idx,
            relationship_assessment_str(&decision.relationship_assessment),
            decision_action_str(&decision.action),
            &decision.members,
            decision.existing_considered.as_deref(),
            result_memory_id_str,
            &decision.rationale,
        )
        .await;

        match dispatch_result {
            Ok(_) => match decision.action {
                DecisionAction::Skip => summary.decisions_skipped += 1,
                DecisionAction::Append => summary.decisions_appended += 1,
                DecisionAction::Revise => summary.decisions_revised += 1,
                DecisionAction::Create => summary.decisions_created += 1,
            },
            Err(e) => summary
                .errors
                .push(format!("dispatch cluster {}: {:?}", cluster_idx, e)),
        }
    }

    Ok(())
}

async fn find_related_semantics(
    env: &Env,
    db: &D1Database,
    cluster: &[Memory],
) -> Result<Vec<Memory>> {
    let combined = cluster
        .iter()
        .map(|m| m.summary.as_str())
        .collect::<Vec<_>>()
        .join(" || ");

    let emb = worker_embed::embed_query(env, &combined).await?;
    let matches = worker_vectorize::query_top_k(env, &emb, RELATED_SEMANTICS_TO_FETCH).await?;

    // Exclude cluster members; keep only semantics (orientation is
    // surfaced separately and we don't want it competing for context).
    let cluster_ids: HashSet<&str> = cluster.iter().map(|m| m.id.as_str()).collect();
    let related_ids: Vec<&str> = matches
        .iter()
        .map(|m| m.id.as_str())
        .filter(|id| !cluster_ids.contains(id))
        .collect();

    if related_ids.is_empty() {
        return Ok(Vec::new());
    }

    let memories = worker_store::get_many(db, &related_ids).await?;
    Ok(memories
        .into_iter()
        .filter(|m| m.memory_type == MemoryType::Semantic)
        .collect())
}

// ──── Haiku call ────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = "You are a semantic consolidation agent.

Hebbian consolidation has identified a set of episodic memories as possible candidates for semantic escalation. Your task is to determine whether the memories merely co-occurred, or whether they contain durable knowledge worth converting into semantic memory.

Episodic memories are records of events, conversations, or moments.
Semantic memories are reusable knowledge about concepts, topics, people, projects, preferences, capabilities, relationships, recurring patterns, or interpretive frames.

The episodic memories must remain intact. Do not overwrite or summarise them as events. If escalation is warranted, extract only the durable knowledge they imply.

Input is a *cluster* of candidate memories that have been transitively connected via co-activation. Your output partitions the cluster into one or more decisions. The union of all decisions' members must equal the full cluster exactly — every cluster member must appear in exactly one decision.

Process for each subgroup of the cluster:

1. Inspect the candidate episodic memories.
2. Determine whether their connection is:
   - Coincidental: surface-level overlap only; no durable knowledge.
   - Tangential: weak but useful shared concept or pattern.
   - Semantically meaningful: clear reusable knowledge emerges.
3. Skip coincidental candidates without modification.
4. For tangential or meaningful candidates, extract the working knowledge into semantic form.
5. Before writing, check the related existing semantic memories provided below — they have stable identifiers shown as the full UUID.
6. Decide whether the extracted knowledge is:
   - already represented (action: skip),
   - a supplement to an existing semantic memory (action: append),
   - a correction or refinement of an existing semantic memory (action: revise),
   - or genuinely new semantic knowledge (action: create).
7. For append/revise, set existing_considered to the exact full UUID of the existing semantic you're modifying.
8. Avoid creating duplicate semantic entries.

Semantic writing rules:

- Write what is known, not what happened.
- Do not centre the event history unless necessary for meaning.
- Do not use episodic framing such as \"the user said\" or \"during an event.\"
- Prefer stable, reusable statements.
- Preserve uncertainty where appropriate.
- Do not overgeneralise from weak evidence.
- Do not transform emotional coincidence into durable truth unless supported by multiple memories or strong context.

Respond by calling the consolidation_decisions tool. The members field in each decision must contain full UUIDs from the cluster, never the 8-character prefixes used for display.";

async fn consolidate_via_claude(
    env: &Env,
    cluster: &[Memory],
    intra_edges: &[&(String, String, u32)],
    related: &[Memory],
) -> Result<Vec<ConsolidationDecision>> {
    let api_key = env.secret("ANTHROPIC_API_KEY")?.to_string();
    let user_message = format_cluster_for_haiku(cluster, intra_edges, related);

    let tool_definition = json!({
        "name": "consolidation_decisions",
        "description": "Submit consolidation decisions for the candidate cluster. The members across all decisions must exactly cover the cluster.",
        "input_schema": {
            "type": "object",
            "properties": {
                "decisions": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "relationship_assessment": {
                                "type": "string",
                                "enum": ["coincidental", "tangential", "semantically_meaningful"]
                            },
                            "action": {
                                "type": "string",
                                "enum": ["skip", "append", "revise", "create"]
                            },
                            "members": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Full UUIDs of the cluster members this decision applies to."
                            },
                            "existing_considered": {
                                "type": "string",
                                "description": "Full UUID of existing semantic, if action is append or revise."
                            },
                            "rationale": { "type": "string" },
                            "semantic_entry": {
                                "type": "string",
                                "description": "The distilled semantic content. Required for append/revise/create."
                            },
                            "summary": {
                                "type": "string",
                                "description": "One-line summary. Required for append/revise/create."
                            },
                            "entity": { "type": "string" },
                            "tags": {
                                "type": "array",
                                "items": { "type": "string" }
                            }
                        },
                        "required": ["relationship_assessment", "action", "members", "rationale"]
                    }
                }
            },
            "required": ["decisions"]
        }
    });

    let body = json!({
        "model": HAIKU_MODEL,
        "max_tokens": 4096,
        "system": SYSTEM_PROMPT,
        "tools": [tool_definition],
        "tool_choice": { "type": "tool", "name": "consolidation_decisions" },
        "messages": [{
            "role": "user",
            "content": user_message
        }]
    });

    let mut headers = Headers::new();
    headers.set("x-api-key", &api_key)?;
    headers.set("anthropic-version", "2023-06-01")?;
    headers.set("content-type", "application/json")?;

    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(body.to_string().into()));

    let req = Request::new_with_init("https://api.anthropic.com/v1/messages", &init)?;
    let mut resp = Fetch::Request(req).send().await?;

    if resp.status_code() >= 400 {
        let err_text = resp.text().await.unwrap_or_else(|_| "no body".to_string());
        return Err(worker::Error::RustError(format!(
            "Anthropic API {} : {}",
            resp.status_code(),
            err_text
        )));
    }

    let body_text = resp.text().await?;
    let response_json: Value = serde_json::from_str(&body_text)
        .map_err(|e| worker::Error::RustError(format!("parse response: {}", e)))?;

    let content = response_json
        .get("content")
        .and_then(|c| c.as_array())
        .ok_or_else(|| worker::Error::RustError("no content array".to_string()))?;

    let tool_use = content
        .iter()
        .find(|item| item.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
        .ok_or_else(|| worker::Error::RustError("no tool_use block".to_string()))?;

    let input = tool_use
        .get("input")
        .ok_or_else(|| worker::Error::RustError("tool_use missing input".to_string()))?;

    let decisions_value = input
        .get("decisions")
        .ok_or_else(|| worker::Error::RustError("missing decisions".to_string()))?;

    let decisions: Vec<ConsolidationDecision> = serde_json::from_value(decisions_value.clone())
        .map_err(|e| worker::Error::RustError(format!("parse decisions: {}", e)))?;

    Ok(decisions)
}

fn format_cluster_for_haiku(
    cluster: &[Memory],
    intra_edges: &[&(String, String, u32)],
    related: &[Memory],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Cluster of {} memories transitively connected via Hebbian co-activation.\n\n",
        cluster.len()
    ));

    out.push_str("CANDIDATES:\n\n");
    for (idx, m) in cluster.iter().enumerate() {
        let tags = if m.tags.is_empty() {
            String::new()
        } else {
            format!(" tags: [{}]", m.tags.join(", "))
        };
        let entity = m
            .entity
            .as_deref()
            .map(|e| format!(" entity: {}", e))
            .unwrap_or_default();
        out.push_str(&format!(
            "[{}] uuid: {}\n    type: {}{}{}\n    summary: {}\n    content: {}\n\n",
            idx + 1,
            m.id,
            m.memory_type.as_str(),
            entity,
            tags,
            m.summary,
            m.content
        ));
    }

    if !intra_edges.is_empty() {
        out.push_str("CO-ACTIVATION EDGES (within cluster, count = times surfaced together):\n");
        for (a, b, count) in intra_edges {
            out.push_str(&format!("  {} ↔ {} : {}\n", a, b, count));
        }
        out.push('\n');
    }

    if !related.is_empty() {
        out.push_str(
            "EXISTING SEMANTICS (consider these before creating new semantic entries — \
             reference by uuid in existing_considered if you append or revise):\n\n",
        );
        for m in related {
            out.push_str(&format!("  uuid: {}\n  summary: {}\n\n", m.id, m.summary));
        }
    }

    out.push_str(
        "Now apply the consolidation framework. Partition the cluster into one or more decisions; \
         every candidate UUID must appear in exactly one decision's members array.",
    );
    out
}

// ──── Decision dispatch ─────────────────────────────────────────────────

/// Returns the memory_id of the created/updated semantic, or None for
/// `Skip`. The audit layer uses this to populate `result_memory_id`
/// in `rem_decisions`.
async fn dispatch_decision(
    env: &Env,
    db: &D1Database,
    decision: &ConsolidationDecision,
) -> Result<Option<String>> {
    match decision.action {
        DecisionAction::Skip => Ok(None),

        DecisionAction::Create => {
            let content = decision.semantic_entry.as_ref().ok_or_else(|| {
                worker::Error::RustError("create missing semantic_entry".to_string())
            })?;
            let summary = decision.summary.as_ref().ok_or_else(|| {
                worker::Error::RustError("create missing summary".to_string())
            })?;
            let source_ids: Vec<&str> = decision.members.iter().map(String::as_str).collect();

            let created = worker_store::create_semantic_with_lineage(
                db,
                content.clone(),
                summary.clone(),
                decision.entity.clone(),
                decision.tags.clone().unwrap_or_default(),
                &source_ids,
            )
            .await?;

            let emb = worker_embed::embed_document(env, content).await?;
            worker_vectorize::upsert_one(env, &created.id, &emb).await?;
            Ok(Some(created.id))
        }

        DecisionAction::Append | DecisionAction::Revise => {
            let target_id = decision.existing_considered.as_ref().ok_or_else(|| {
                worker::Error::RustError("append/revise missing existing_considered".to_string())
            })?;
            let content = decision.semantic_entry.as_ref().ok_or_else(|| {
                worker::Error::RustError("append/revise missing semantic_entry".to_string())
            })?;
            let summary = decision.summary.as_ref().ok_or_else(|| {
                worker::Error::RustError("append/revise missing summary".to_string())
            })?;
            let source_ids: Vec<&str> = decision.members.iter().map(String::as_str).collect();

            let updated = worker_store::update_semantic_with_lineage(
                db,
                target_id,
                content,
                summary,
                &source_ids,
            )
            .await?;

            if updated {
                let emb = worker_embed::embed_document(env, content).await?;
                worker_vectorize::upsert_one(env, target_id, &emb).await?;
                Ok(Some(target_id.clone()))
            } else {
                Ok(None)
            }
        }
    }
}
