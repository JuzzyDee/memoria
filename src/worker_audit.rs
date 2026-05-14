// worker_audit.rs — D1-backed audit log for the wasm32 worker.
//
// Mirrors audit::log's behaviour but writes to a D1 binding instead of a
// local SQLite file. Best-effort, async: a failed audit write logs a
// tracing::error but never bubbles up to the caller — the security model
// is "audit-on-success" not "audit-or-fail".
//
// AuditEntry + Outcome types live in audit.rs (shared between targets).

use crate::audit::AuditEntry;
use worker::{wasm_bindgen::JsValue, D1Database};

/// Write one audit row to the `api_key_audit` table. Best-effort.
pub async fn log(db: &D1Database, entry: AuditEntry<'_>) {
    let now = chrono::Utc::now().timestamp();

    let success: i64 = if entry.outcome.success() { 1 } else { 0 };
    let error_kind_value: JsValue = match entry.outcome.error_kind() {
        Some(s) => s.into(),
        None => JsValue::NULL,
    };

    let stmt = db.prepare(
        "INSERT INTO api_key_audit (timestamp, key_id, role, tool_name, success, error_kind)
         VALUES (?, ?, ?, ?, ?, ?)",
    );
    let bound = match stmt.bind(&[
        now.into(),
        entry.key_id.into(),
        entry.role.as_str().into(),
        entry.tool.into(),
        success.into(),
        error_kind_value,
    ]) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("D1 audit bind failed: {:?}", e);
            return;
        }
    };
    if let Err(e) = bound.run().await {
        tracing::error!("D1 audit write failed: {:?}", e);
    }
}
