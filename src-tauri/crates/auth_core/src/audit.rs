use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuditError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not determine OS data directory")]
    NoDataDir,
}

const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub timestamp: DateTime<Utc>,
    pub user: String,
    pub provider: String,
    pub repository: String,
    pub branch: Option<String>,
    pub mr_pr: Option<String>,
    pub action: String,
    pub result: String,
    pub error: Option<String>,
}

pub struct AuditLog {
    conn: Connection,
}

impl AuditLog {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AuditError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(MIGRATION_0001)?;
        Ok(Self { conn })
    }

    /// `<OS data dir>/git-workflow-engine/audit.sqlite3`, or
    /// `$GWE_DATA_DIR/audit.sqlite3` when that env var is set (tests point it at
    /// a tempdir so they never touch the real database).
    pub fn default_path() -> Result<PathBuf, AuditError> {
        if let Some(dir) = std::env::var_os("GWE_DATA_DIR") {
            return Ok(PathBuf::from(dir).join("audit.sqlite3"));
        }
        let dir = dirs::data_dir().ok_or(AuditError::NoDataDir)?;
        Ok(dir.join("git-workflow-engine").join("audit.sqlite3"))
    }

    pub fn log(&self, entry: &AuditEntry) -> Result<(), AuditError> {
        self.conn.execute(
            "INSERT INTO audit_log (timestamp, user, provider, repository, branch, mr_pr, action, result, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                entry.timestamp.to_rfc3339(),
                entry.user,
                entry.provider,
                entry.repository,
                entry.branch,
                entry.mr_pr,
                entry.action,
                entry.result,
                entry.error,
            ],
        )?;
        Ok(())
    }

    pub fn recent(&self, limit: u32) -> Result<Vec<AuditEntry>, AuditError> {
        let mut stmt = self.conn.prepare(
            "SELECT timestamp, user, provider, repository, branch, mr_pr, action, result, error
             FROM audit_log ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            let timestamp: String = row.get(0)?;
            Ok(AuditEntry {
                timestamp: DateTime::parse_from_rfc3339(&timestamp)
                    .unwrap()
                    .with_timezone(&Utc),
                user: row.get(1)?,
                provider: row.get(2)?,
                repository: row.get(3)?,
                branch: row.get(4)?,
                mr_pr: row.get(5)?,
                action: row.get(6)?,
                result: row.get(7)?,
                error: row.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_entry() -> AuditEntry {
        AuditEntry {
            timestamp: Utc::now(),
            user: "alice".into(),
            provider: "gitlab".into(),
            repository: "group/repo".into(),
            branch: Some("feature/x".into()),
            mr_pr: Some("42".into()),
            action: "create_merge_request".into(),
            result: "success".into(),
            error: None,
        }
    }

    #[test]
    fn inserts_and_reads_back() {
        let dir = tempdir().unwrap();
        let log = AuditLog::open(dir.path().join("audit.sqlite3")).unwrap();

        log.log(&sample_entry()).unwrap();
        let rows = log.recent(10).unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].user, "alice");
        assert_eq!(rows[0].action, "create_merge_request");
    }

    #[test]
    fn schema_has_no_token_or_diff_columns() {
        let dir = tempdir().unwrap();
        let log = AuditLog::open(dir.path().join("audit.sqlite3")).unwrap();

        let mut stmt = log
            .conn
            .prepare("SELECT name FROM pragma_table_info('audit_log')")
            .unwrap();
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        for forbidden in ["token", "password", "ssh_key", "diff"] {
            assert!(
                !columns.iter().any(|c| c.contains(forbidden)),
                "schema must not have a column matching '{forbidden}'"
            );
        }
    }
}
