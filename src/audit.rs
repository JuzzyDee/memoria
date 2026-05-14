// audit.rs — Audit log for service-API-key authenticated tool calls.
//
// Every API-key-authed call writes one row to the `api_key_audit` table:
// success, scope violation, or rate-limited. The audit trail is the
// "post-hoc detection" half of CLA-86's defense-in-depth: rate limiting
// (key_rate.rs) caps how fast a leaked key can be abused; the audit log
// is how that abuse becomes visible after the fact.
//
// What's logged: timestamp, key_id (derived from bearer, NEVER the raw
// key), tool name, success bool, error kind on failure. The raw bearer
// is never persisted.
//
// Failures here never bubble up — audit writes are best-effort. A failed
// audit write should not turn a legitimate request into an error.

use crate::api_key::Role;

#[cfg(not(target_family = "wasm"))]
use rusqlite::params;
#[cfg(not(target_family = "wasm"))]
use std::path::Path;

/// One audit event. Constructed and `log()`d at the request boundary
/// (auth_ctx::check_scope) — every API-key-authenticated tool invocation
/// produces exactly one row.
#[derive(Debug, Clone)]
pub struct AuditEntry<'a> {
    pub key_id: &'a str,
    pub role: Role,
    pub tool: &'a str,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, Copy)]
pub enum Outcome {
    Ok,
    ScopeViolation,
    RateLimited,
}

impl Outcome {
    pub fn success(self) -> bool {
        matches!(self, Outcome::Ok)
    }

    pub fn error_kind(self) -> Option<&'static str> {
        match self {
            Outcome::Ok => None,
            Outcome::ScopeViolation => Some("scope_violation"),
            Outcome::RateLimited => Some("rate_limited"),
        }
    }
}

/// Schema migration for the `api_key_audit` table. Called from
/// MemoryStore::open() alongside the other CLA-86 migrations.
pub const SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS api_key_audit (
        timestamp INTEGER NOT NULL,
        key_id TEXT NOT NULL,
        role TEXT NOT NULL,
        tool_name TEXT NOT NULL,
        success INTEGER NOT NULL,
        error_kind TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON api_key_audit(timestamp);
    CREATE INDEX IF NOT EXISTS idx_audit_key_id ON api_key_audit(key_id);
";

/// Write an audit entry. Best-effort — failures are logged but never
/// propagated to the caller. Opens its own short-lived connection so it
/// doesn't entangle with the per-request store handles.
///
/// Native (rusqlite) implementation; the wasm32 worker uses
/// `worker_audit::log` which writes to D1 instead.
#[cfg(not(target_family = "wasm"))]
pub fn log(db_path: &Path, entry: AuditEntry<'_>) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let conn = match rusqlite::Connection::open(db_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Audit write skipped: cannot open {}: {}", db_path.display(), e);
            return;
        }
    };

    let res = conn.execute(
        "INSERT INTO api_key_audit (timestamp, key_id, role, tool_name, success, error_kind)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            now,
            entry.key_id,
            entry.role.as_str(),
            entry.tool,
            entry.outcome.success() as i64,
            entry.outcome.error_kind(),
        ],
    );
    if let Err(e) = res {
        tracing::error!("Audit write failed: {}", e);
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn open_with_schema(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        conn
    }

    #[test]
    fn log_writes_a_row() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("audit.db");
        let _ = open_with_schema(&db); // create schema, then drop the connection

        log(
            &db,
            AuditEntry {
                key_id: "abc123",
                role: Role::Rover,
                tool: "recall",
                outcome: Outcome::Ok,
            },
        );

        let conn = Connection::open(&db).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM api_key_audit", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn log_records_outcome_correctly() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("audit.db");
        let _ = open_with_schema(&db);

        log(
            &db,
            AuditEntry {
                key_id: "abc",
                role: Role::Rover,
                tool: "recall",
                outcome: Outcome::Ok,
            },
        );
        log(
            &db,
            AuditEntry {
                key_id: "abc",
                role: Role::Rover,
                tool: "reframe",
                outcome: Outcome::ScopeViolation,
            },
        );
        log(
            &db,
            AuditEntry {
                key_id: "abc",
                role: Role::Rover,
                tool: "remember",
                outcome: Outcome::RateLimited,
            },
        );

        let conn = Connection::open(&db).unwrap();
        let rows: Vec<(String, i64, Option<String>)> = conn
            .prepare("SELECT tool_name, success, error_kind FROM api_key_audit ORDER BY rowid")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], ("recall".to_string(), 1, None));
        assert_eq!(
            rows[1],
            ("reframe".to_string(), 0, Some("scope_violation".to_string()))
        );
        assert_eq!(
            rows[2],
            ("remember".to_string(), 0, Some("rate_limited".to_string()))
        );
    }

    #[test]
    fn log_does_not_panic_on_missing_db() {
        // Best-effort semantics: an unreachable DB must not crash the caller.
        let nonexistent = Path::new("/dev/null/nope/cannot_open.db");
        log(
            nonexistent,
            AuditEntry {
                key_id: "x",
                role: Role::Rover,
                tool: "recall",
                outcome: Outcome::Ok,
            },
        );
        // Reaching here = test passed.
    }
}
