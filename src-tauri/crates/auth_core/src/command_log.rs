use std::path::Path;
use std::time::Instant;

use chrono::Utc;
use rusqlite::{params, Connection};

use crate::audit::AuditError;

const MIGRATION_0002: &str = include_str!("../migrations/0002_command_log.sql");

/// Known personal-access-token shapes. Anything starting with one of these
/// prefixes followed by a token body is reduced to `<prefix>***` before it is
/// logged.
const TOKEN_PREFIXES: [&str; 5] = ["glpat-", "github_pat_", "ghp_", "gho_", "glptt-"];

/// One mutating operation as it was actually executed: what ran, against
/// which repo, how long it took, and whether it worked. No stdout/stderr/diff
/// is ever stored -- only a masked one-line summary and, on failure, a masked
/// error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRecord {
    pub timestamp: String,
    pub repository: String,
    pub operation: String,
    pub args: String,
    /// `"success"` | `"failure"`.
    pub outcome: String,
    pub duration_ms: i64,
    pub error: Option<String>,
}

pub struct CommandLog {
    conn: Connection,
}

impl CommandLog {
    /// Opens (creating if needed) the command log. Shares the file with
    /// `AuditLog` -- pass `AuditLog::default_path()`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AuditError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(MIGRATION_0002)?;
        Ok(Self { conn })
    }

    pub fn record(&self, r: &CommandRecord) -> Result<(), AuditError> {
        self.conn.execute(
            "INSERT INTO command_log (timestamp, repository, operation, args, outcome, duration_ms, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                r.timestamp,
                r.repository,
                r.operation,
                r.args,
                r.outcome,
                r.duration_ms,
                r.error,
            ],
        )?;
        Ok(())
    }

    pub fn recent(&self, limit: u32) -> Result<Vec<CommandRecord>, AuditError> {
        let mut stmt = self.conn.prepare(
            "SELECT timestamp, repository, operation, args, outcome, duration_ms, error
             FROM command_log ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], row_to_record)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Rows for one repository, newest first. `repository` matches whatever
    /// key `record_op` was called with (the command layer passes the repo
    /// path).
    pub fn for_repo(&self, repository: &str, limit: u32) -> Result<Vec<CommandRecord>, AuditError> {
        let mut stmt = self.conn.prepare(
            "SELECT timestamp, repository, operation, args, outcome, duration_ms, error
             FROM command_log WHERE repository = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![repository, limit], row_to_record)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<CommandRecord> {
    Ok(CommandRecord {
        timestamp: row.get(0)?,
        repository: row.get(1)?,
        operation: row.get(2)?,
        args: row.get(3)?,
        outcome: row.get(4)?,
        duration_ms: row.get(5)?,
        error: row.get(6)?,
    })
}

/// Runs `f`, timing it, and records the outcome to `log`. Logging failure is
/// swallowed -- it must never mask the real result. `args` and any error text
/// are secret-masked before they touch the DB.
pub fn record_op<T, E: std::fmt::Display>(
    log: &CommandLog,
    repository: &str,
    operation: &str,
    args: &str,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let started = Instant::now();
    let result = f();
    let record = CommandRecord {
        timestamp: Utc::now().to_rfc3339(),
        repository: repository.to_string(),
        operation: operation.to_string(),
        args: mask_secrets(args),
        outcome: if result.is_ok() { "success" } else { "failure" }.to_string(),
        duration_ms: started.elapsed().as_millis() as i64,
        error: result.as_ref().err().map(|e| mask_secrets(&e.to_string())),
    };
    log.record(&record).ok();
    result
}

/// Redacts credentials before a string is logged: `user:pass@` in any URL
/// becomes `***@`, and known personal-access-token forms keep only their
/// prefix.
pub fn mask_secrets(s: &str) -> String {
    mask_token_prefixes(&mask_url_userinfo(s))
}

fn mask_url_userinfo(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(idx) = rest.find("://") {
        out.push_str(&rest[..idx + 3]);
        rest = &rest[idx + 3..];
        let end = rest
            .find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace())
            .unwrap_or(rest.len());
        let authority = &rest[..end];
        match authority.rfind('@') {
            Some(at) if authority[..at].contains(':') => {
                out.push_str("***@");
                out.push_str(&authority[at + 1..]);
            }
            _ => out.push_str(authority),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn mask_token_prefixes(s: &str) -> String {
    let is_body = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        // earliest prefix hit anywhere in the remaining string
        let hit = TOKEN_PREFIXES
            .iter()
            .filter_map(|p| rest.find(p).map(|i| (i, *p)))
            .min_by_key(|(i, _)| *i);
        let Some((i, p)) = hit else { break };
        let body_start = i + p.len();
        let body_len = rest[body_start..]
            .find(|c: char| !is_body(c))
            .unwrap_or(rest.len() - body_start);
        out.push_str(&rest[..i]);
        if body_len >= 3 {
            out.push_str(p);
            out.push_str("***");
        } else {
            out.push_str(&rest[i..body_start]);
        }
        rest = &rest[body_start + body_len..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn masks_url_credentials() {
        assert_eq!(
            mask_secrets("push failed: https://kelvin:glpat-abcd1234@gitlab.example.com/g/r.git"),
            "push failed: https://***@gitlab.example.com/g/r.git"
        );
    }

    #[test]
    fn masks_bare_token() {
        assert_eq!(mask_secrets("token glpat-SECRETVALUE123 rejected"), "token glpat-*** rejected");
        assert_eq!(mask_secrets("using ghp_0123456789abcdef"), "using ghp_***");
    }

    #[test]
    fn leaves_clean_text_untouched() {
        let s = "push origin feature/login-form: non-fast-forward";
        assert_eq!(mask_secrets(s), s);
    }

    #[test]
    fn records_and_reads_back_with_masking() {
        let dir = tempdir().unwrap();
        let log = CommandLog::open(dir.path().join("audit.sqlite3")).unwrap();

        let out: Result<(), String> = record_op(&log, "g/r", "push", "origin feature/x", || {
            Err("remote: https://u:glpat-xxxxxxxx@h/r.git rejected".to_string())
        });
        assert!(out.is_err());

        let rows = log.recent(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].operation, "push");
        assert_eq!(rows[0].outcome, "failure");
        let err = rows[0].error.as_deref().unwrap();
        assert!(err.contains("://***@h/r.git"), "{err}");
        assert!(!err.contains("glpat-xxxxxxxx"), "{err}");
    }

    #[test]
    fn for_repo_filters_and_orders() {
        let dir = tempdir().unwrap();
        let log = CommandLog::open(dir.path().join("audit.sqlite3")).unwrap();
        let _: Result<(), String> = record_op(&log, "/a", "commit", "one", || Ok(()));
        let _: Result<(), String> = record_op(&log, "/b", "commit", "two", || Ok(()));
        let _: Result<(), String> = record_op(&log, "/a", "push", "three", || Ok(()));

        let rows = log.for_repo("/a", 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].operation, "push");
        assert!(rows.iter().all(|r| r.repository == "/a"));
    }

    #[test]
    fn success_has_no_error() {
        let dir = tempdir().unwrap();
        let log = CommandLog::open(dir.path().join("audit.sqlite3")).unwrap();
        let out: Result<i32, String> = record_op(&log, "g/r", "commit", "msg", || Ok(1));
        assert_eq!(out.unwrap(), 1);
        assert_eq!(log.recent(1).unwrap()[0].outcome, "success");
        assert!(log.recent(1).unwrap()[0].error.is_none());
    }
}
