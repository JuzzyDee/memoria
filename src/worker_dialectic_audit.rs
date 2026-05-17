// worker_dialectic_audit.rs — Persistent observability for dialectic runs and decisions.
//
// CLA-95 observability layer. Writes to dialectic_runs and
// dialectic_decisions (migration 0005) so post-hoc investigation can
// answer:
//
//   - What did the dialectic flag recently and why?
//   - Which memories has the dialectic looked at?
//   - What reframes did Stage 3 produce on which memories?
//
// All writes are best-effort — the dialectic dispatch itself never blocks
// on audit success. If audit writes fail (e.g. D1 transient), the run
// still completes and we lose the trail for that run only.
//
// Schema is designed across all three stages (CLA-95 ticket). Stage 1
// records the neutral-assessor judgment (assessment + rationale); the
// transcript / resolution / action / action_payload columns stay NULL
// until later stages light them up.
//
// Parallels worker_rem_audit's shape deliberately — same return-the-id
// pattern at run-start, same best-effort idempotent UPDATE at run-finish,
// same per-decision INSERT.

#![cfg(target_family = "wasm")]

use crate::worker_dialectic::RunSummary;
use uuid::Uuid;
use worker::{D1Database, Result};

/// Insert a fresh dialectic_runs row at the start of a dialectic
/// invocation. Returns the run_id; caller passes it back to
/// `record_decision` and `record_run_finish` so all writes correlate.
pub async fn record_run_start(db: &D1Database) -> Result<String> {
    let run_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    db.prepare("INSERT INTO dialectic_runs (run_id, started_at) VALUES (?, ?)")
        .bind(&[run_id.clone().into(), now.into()])?
        .run()
        .await?;
    Ok(run_id)
}

/// Update the dialectic_runs row with final stats once the run completes.
/// Idempotent — if called twice with the same run_id the second call
/// overwrites the first.
pub async fn record_run_finish(
    db: &D1Database,
    run_id: &str,
    summary: &RunSummary,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let errors_json =
        serde_json::to_string(&summary.errors).unwrap_or_else(|_| "[]".to_string());
    db.prepare(
        "UPDATE dialectic_runs SET
            finished_at = ?,
            candidates_reviewed = ?,
            decisions_count = ?,
            errors_count = ?,
            errors_summary = ?
         WHERE run_id = ?",
    )
    .bind(&[
        now.into(),
        (summary.candidates_reviewed as i32).into(),
        (summary.decisions_count as i32).into(),
        (summary.errors.len() as i32).into(),
        errors_json.into(),
        run_id.into(),
    ])?
    .run()
    .await?;
    Ok(())
}

/// Persist one dialectic judgment.
///
/// Stage 1 callers set `assessment` + `rationale` and leave everything
/// else `None`. Stage 2 will populate `transcript` + `resolution`.
/// Stage 3 will populate `action` + `action_payload`.
#[allow(clippy::too_many_arguments)]
pub async fn record_decision(
    db: &D1Database,
    run_id: &str,
    memory_id: &str,
    assessment: &str,
    rationale: &str,
    transcript: Option<&str>,
    resolution: Option<&str>,
    action: Option<&str>,
    action_payload: Option<&str>,
) -> Result<()> {
    let decision_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    db.prepare(
        "INSERT INTO dialectic_decisions
            (decision_id, run_id, memory_id, assessment, rationale,
             transcript, resolution, action, action_payload, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&[
        decision_id.into(),
        run_id.into(),
        memory_id.into(),
        assessment.into(),
        rationale.into(),
        match transcript {
            Some(s) => s.into(),
            None => worker::wasm_bindgen::JsValue::NULL,
        },
        match resolution {
            Some(s) => s.into(),
            None => worker::wasm_bindgen::JsValue::NULL,
        },
        match action {
            Some(s) => s.into(),
            None => worker::wasm_bindgen::JsValue::NULL,
        },
        match action_payload {
            Some(s) => s.into(),
            None => worker::wasm_bindgen::JsValue::NULL,
        },
        now.into(),
    ])?
    .run()
    .await?;
    Ok(())
}
