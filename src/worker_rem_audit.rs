// worker_rem_audit.rs — Persistent observability for REM runs and decisions.
//
// CLA-87 observability layer. Writes to rem_runs and rem_decisions
// (migration 0003) so post-hoc investigation can answer:
//
//   - What did REM reject recently and why?
//   - Which nights produced consolidations vs. nothing?
//   - What decisions led to a particular consolidated semantic?
//
// All writes are best-effort — the REM dispatch itself never blocks on
// audit success. If audit writes fail (e.g. D1 transient), the run
// still completes and its work still persists; we lose the audit trail
// for that run only.

#![cfg(target_family = "wasm")]

use crate::worker_rem::RunSummary;
use uuid::Uuid;
use worker::{D1Database, Result};

/// Insert a fresh rem_runs row at the start of a REM invocation.
/// Returns the run_id; caller passes it back to `record_decision` and
/// `record_run_finish` so all writes correlate to this row.
pub async fn record_run_start(db: &D1Database) -> Result<String> {
    let run_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    db.prepare("INSERT INTO rem_runs (run_id, started_at) VALUES (?, ?)")
        .bind(&[run_id.clone().into(), now.into()])?
        .run()
        .await?;
    Ok(run_id)
}

/// Update the rem_runs row with final stats once the run completes.
/// Idempotent — if this is called twice with the same run_id the
/// second call overwrites the first.
pub async fn record_run_finish(
    db: &D1Database,
    run_id: &str,
    pairs_found: usize,
    summary: &RunSummary,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let errors_json =
        serde_json::to_string(&summary.errors).unwrap_or_else(|_| "[]".to_string());
    db.prepare(
        "UPDATE rem_runs SET
            finished_at = ?,
            decayed = ?,
            pairs_found = ?,
            clusters_attempted = ?,
            decisions_created = ?,
            decisions_appended = ?,
            decisions_revised = ?,
            decisions_skipped = ?,
            errors_count = ?,
            errors_summary = ?
         WHERE run_id = ?",
    )
    .bind(&[
        now.into(),
        (summary.decayed as i64).into(),
        (pairs_found as i64).into(),
        (summary.clusters_attempted as i64).into(),
        (summary.decisions_created as i64).into(),
        (summary.decisions_appended as i64).into(),
        (summary.decisions_revised as i64).into(),
        (summary.decisions_skipped as i64).into(),
        (summary.errors.len() as i64).into(),
        errors_json.into(),
        run_id.into(),
    ])?
    .run()
    .await?;
    Ok(())
}

/// Persist one Haiku decision. Called from the per-cluster loop after
/// dispatch — `result_memory_id` is the UUID of the created/updated
/// semantic, or None for skips and errors. `existing_considered` is
/// only set for append/revise actions.
#[allow(clippy::too_many_arguments)]
pub async fn record_decision(
    db: &D1Database,
    run_id: &str,
    cluster_idx: usize,
    relationship_assessment: &str,
    action: &str,
    members: &[String],
    existing_considered: Option<&str>,
    result_memory_id: Option<&str>,
    rationale: &str,
) -> Result<()> {
    let decision_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let members_json = serde_json::to_string(members).unwrap_or_else(|_| "[]".to_string());

    db.prepare(
        "INSERT INTO rem_decisions
            (decision_id, run_id, cluster_idx, relationship_assessment, action,
             members, existing_considered, result_memory_id, rationale, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&[
        decision_id.into(),
        run_id.into(),
        (cluster_idx as i64).into(),
        relationship_assessment.into(),
        action.into(),
        members_json.into(),
        match existing_considered {
            Some(s) => s.into(),
            None => worker::wasm_bindgen::JsValue::NULL,
        },
        match result_memory_id {
            Some(s) => s.into(),
            None => worker::wasm_bindgen::JsValue::NULL,
        },
        rationale.into(),
        now.into(),
    ])?
    .run()
    .await?;
    Ok(())
}
