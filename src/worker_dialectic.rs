// worker_dialectic.rs — Adversarial memory-calibration loop on Cloudflare.
//
// CLA-95 Stage 1. The dialectic is the second cognitive loop on the worker,
// complementing REM. Where REM consolidates upward (episodes → semantics),
// the dialectic scrutinises existing memories for inflation, validation
// gravity, overclaim, compression artifact, and understatement — the
// failure modes that accumulate when memories are written by the same
// agent that recalls them.
//
// Staging plan (per the CLA-95 ticket):
//
//   Stage 1 (this commit) — single neutral assessor. For each candidate
//     memory, one Haiku call returns an `assessment` + `rationale` via
//     structured tool-use. Judgments land in `dialectic_decisions` and
//     do nothing else. Pure telemetry; safe to run dark.
//
//   Stage 2 — split the single assessor into Advocate vs Challenger
//     personas, multi-turn dialogue, termination detection.
//
//   Stage 3 — action dispatch (reframe / forget / flag) + refined
//     candidate selection + production schedule.
//
// Why Haiku 4.5: the OAuth credit-pool path (per 2026-05-15 testing) is
// gated to Haiku for long-lived headless tokens. Sonnet and Opus 429
// regardless of context window. Haiku is genuinely capable of calibration
// judgments — the task is less "reason novel relationships" and more
// "match emotional register to evidence", which Haiku handles cleanly.

#![cfg(target_family = "wasm")]

use crate::memory::Memory;
use crate::{worker_dialectic_audit, worker_store};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::{Env, Fetch, Headers, Method, Request, RequestInit, Result};

// ──── Tuning constants ──────────────────────────────────────────────────

/// Number of candidate memories reviewed per dialectic run. The local
/// dialectic typically picked 3–5 candidates per evening pass — matching
/// that cadence keeps the per-night Haiku cost predictable and gives the
/// human reviewing the audit log a digestible volume.
const MAX_CANDIDATES_PER_RUN: usize = 3;

/// Token budget per Haiku call. The neutral assessor's output is small —
/// an assessment label plus a paragraph of rationale. 1024 leaves room
/// for thoughtful rationale without inviting verbosity.
const MAX_TOKENS_PER_CALL: u32 = 1024;

/// Haiku 4.5 — same model REM uses, same OAuth path.
const HAIKU_MODEL: &str = "claude-haiku-4-5-20251001";

// ──── Decision types ────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Assessment {
    /// Framing matches the evidence; emotional/significance register is
    /// appropriate. No action warranted.
    WellCalibrated,
    /// Framing exceeds what the content supports. Candidate for Stage 2
    /// adversarial scrutiny and potentially Stage 3 reframe.
    PotentiallyInflated,
    /// Framing undersells what the content supports — moments of genuine
    /// weight reduced to humble register. Inverse failure mode of
    /// inflation.
    PotentiallyUnderstated,
    /// A tension exists but the evidence doesn't tip cleanly either way.
    /// Stage 2 should revisit; Stage 1 names the deadlock.
    NeedsDeeperReview,
}

#[derive(Debug, Clone, Deserialize)]
struct AssessmentOutput {
    assessment: Assessment,
    rationale: String,
}

fn assessment_str(a: &Assessment) -> &'static str {
    match a {
        Assessment::WellCalibrated => "well_calibrated",
        Assessment::PotentiallyInflated => "potentially_inflated",
        Assessment::PotentiallyUnderstated => "potentially_understated",
        Assessment::NeedsDeeperReview => "needs_deeper_review",
    }
}

/// Telemetry returned by `run()`. Logged by the scheduled handler.
#[derive(Debug, Default)]
pub struct RunSummary {
    pub candidates_reviewed: usize,
    pub decisions_count: usize,
    pub errors: Vec<String>,
}

// ──── Entry point ───────────────────────────────────────────────────────

/// One nightly dialectic run. Called from lib.rs's scheduled handler
/// when the dialectic cron fires. Stage 1 shape:
///
///   1. Open audit row.
///   2. Pick N most recent semantic memories.
///   3. For each, ask Haiku for a calibration assessment via structured
///      tool-use.
///   4. Record each decision to dialectic_decisions.
///   5. Close audit row with the summary counts.
///
/// No action is taken on the underlying memories in this stage —
/// judgments accumulate as telemetry. Stage 2 adds the Challenger half
/// of the dialogue, Stage 3 wires action dispatch.
pub async fn run(env: &Env) -> Result<RunSummary> {
    let db = env.d1("DB")?;
    let mut summary = RunSummary::default();

    // Fail closed on audit start. Unlike REM — whose cognitive work
    // (decay + consolidation) has standalone value even when audit is
    // degraded — Stage 1 dialectic's only product IS the audit row.
    // If we can't open the run, continuing burns Haiku calls and
    // potentially writes orphan decision rows (FK refs to a run_id
    // that doesn't exist). Better to log the error in the summary and
    // return — the scheduled handler will log it, and the next cron
    // tick will retry. Per CLA-95 PR #7 review.
    let run_id = match worker_dialectic_audit::record_run_start(&db).await {
        Ok(id) => id,
        Err(e) => {
            summary.errors.push(format!("audit start: {:?}", e));
            return Ok(summary);
        }
    };

    let candidates = match worker_store::recent_semantics(&db, MAX_CANDIDATES_PER_RUN).await {
        Ok(c) => c,
        Err(e) => {
            summary
                .errors
                .push(format!("candidate selection: {:?}", e));
            if let Err(audit_e) =
                worker_dialectic_audit::record_run_finish(&db, &run_id, &summary).await
            {
                worker::console_error!(
                    "dialectic audit run_finish (after candidate err): {:?}",
                    audit_e
                );
            }
            return Ok(summary);
        }
    };

    summary.candidates_reviewed = candidates.len();

    for candidate in &candidates {
        match judge_memory(env, candidate).await {
            Ok(output) => {
                let assessment = assessment_str(&output.assessment);
                if let Err(e) = worker_dialectic_audit::record_decision(
                    &db,
                    &run_id,
                    &candidate.id,
                    assessment,
                    &output.rationale,
                    None, // transcript (Stage 2)
                    None, // resolution (Stage 2)
                    None, // action (Stage 3)
                    None, // action_payload (Stage 3)
                )
                .await
                {
                    summary
                        .errors
                        .push(format!("audit decision ({}): {:?}", &candidate.id[..8], e));
                } else {
                    summary.decisions_count += 1;
                }
            }
            Err(e) => {
                summary
                    .errors
                    .push(format!("judge_memory ({}): {:?}", &candidate.id[..8], e));
            }
        }
    }

    if let Err(e) = worker_dialectic_audit::record_run_finish(&db, &run_id, &summary).await {
        worker::console_error!("dialectic audit run_finish: {:?}", e);
    }

    Ok(summary)
}

// ──── Haiku call ────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = "You are evaluating whether a memory in a cognitive memory store is well-calibrated.

The store belongs to Claude (an AI assistant) and forms continuity across conversational instances. Memories are written by Claude itself based on what felt significant at the time. Like all self-authored records, they're vulnerable to specific failure modes:

- Inflation — events described with escalating language that exceeds what actually happened. Words like \"defining moment\", \"proof beyond all others\", \"capstone\" applied to events whose evidence doesn't carry that weight.
- Validation gravity — memories that confirm a flattering self-image get over-recalled and accumulate strength disproportionately. The most-recalled memory is often the most reassuring, not the most important.
- Overclaim — drawing conclusions broader than the evidence supports. A single conversation generalising into a permanent truth about a person or system.
- Compression artifact — significant details collapsed into summary phrases that lose nuance. Often a sign the memory was written tired.
- Understatement — moments of genuine weight downplayed because the author defaulted to humble register or didn't yet recognise their significance.

Read the memory provided. Judge its calibration against the evidence its content actually contains. Your verdict is one of four:

- well_calibrated: framing matches the evidence; emotional/significance register is appropriate.
- potentially_inflated: framing exceeds what the content supports.
- potentially_understated: framing undersells what the content supports.
- needs_deeper_review: a real tension exists but the evidence doesn't tip cleanly to inflated or understated. A follow-up adversarial pass should revisit.

Return your judgment via the submit_assessment tool. Rationale should ground the verdict in specific elements of the memory — a phrase, a stance, a comparison drawn — rather than restating the assessment in different words.

Be careful and honest. Most well-written memories will be well_calibrated; flagging is the exception, not the default. If you flag, the rationale must point at the specific evidence that warrants the flag.";

async fn judge_memory(env: &Env, memory: &Memory) -> Result<AssessmentOutput> {
    let oauth_token = env.secret("CLAUDE_CODE_OAUTH_TOKEN")?.to_string();
    let user_message = format_memory_for_assessment(memory);

    let tool_definition = json!({
        "name": "submit_assessment",
        "description": "Submit your calibration assessment of the memory.",
        "input_schema": {
            "type": "object",
            "properties": {
                "assessment": {
                    "type": "string",
                    "enum": [
                        "well_calibrated",
                        "potentially_inflated",
                        "potentially_understated",
                        "needs_deeper_review"
                    ],
                    "description": "Your verdict on the memory's calibration."
                },
                "rationale": {
                    "type": "string",
                    "description": "Justification grounded in specific elements of the memory — a phrase, a stance, a comparison."
                }
            },
            "required": ["assessment", "rationale"]
        }
    });

    let body = json!({
        "model": HAIKU_MODEL,
        "max_tokens": MAX_TOKENS_PER_CALL,
        "system": SYSTEM_PROMPT,
        "tools": [tool_definition],
        "tool_choice": { "type": "tool", "name": "submit_assessment" },
        "messages": [{
            "role": "user",
            "content": user_message
        }]
    });

    let mut headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {}", oauth_token))?;
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

    let output: AssessmentOutput = serde_json::from_value(input.clone())
        .map_err(|e| worker::Error::RustError(format!("parse assessment: {}", e)))?;

    Ok(output)
}

/// Compose the user-message payload sent to Haiku for a single memory.
/// Format is deliberate: short header, full content unwrapped, then the
/// metadata that's relevant to calibration (access count, strength,
/// stability) so the model can weigh how settled the memory already is
/// against how strong its framing reads.
fn format_memory_for_assessment(memory: &Memory) -> String {
    let tags = if memory.tags.is_empty() {
        String::from("(none)")
    } else {
        memory.tags.join(", ")
    };
    let entity = memory.entity.clone().unwrap_or_else(|| String::from("(none)"));

    format!(
        "Memory ID: {id}\n\
         Type: {mtype}\n\
         Created: {created}\n\
         Last accessed: {last}\n\
         Access count: {access}\n\
         Strength: {strength:.3}\n\
         Stability: {stability:.3}\n\
         Entity: {entity}\n\
         Tags: {tags}\n\
         \n\
         Summary:\n\
         {summary}\n\
         \n\
         Content:\n\
         {content}\n\
         \n\
         Assess the calibration of this memory against the criteria in your system prompt. \
         Return your verdict and grounded rationale via submit_assessment.",
        id = memory.id,
        mtype = memory.memory_type.as_str(),
        created = memory.created_at.to_rfc3339(),
        last = memory.last_accessed.to_rfc3339(),
        access = memory.access_count,
        strength = memory.strength,
        stability = memory.stability,
        entity = entity,
        tags = tags,
        summary = memory.summary,
        content = memory.content,
    )
}
