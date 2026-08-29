use std::path::Path;

use rusqlite::{params, Connection};

use crate::audit::AuditError;

const MIGRATION_0005: &str = include_str!("../migrations/0005_conflicts.sql");

/// The single in-flight conflict resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictRecord {
    pub branch: String,
    pub target_branch: String,
    /// The `target_branch` commit merged in, as a hex OID string.
    pub target_commit: String,
}

pub struct ConflictLog {
    conn: Connection,
}

impl ConflictLog {
    /// Opens (creating if needed) the conflict log. Shares the file with
    /// `AuditLog` -- pass `AuditLog::default_path()`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AuditError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(MIGRATION_0005)?;
        Ok(Self { conn })
    }

    /// Replaces any prior in-flight conflict with this one.
    pub fn start(
        &self,
        branch: &str,
        target_branch: &str,
        target_commit: &str,
    ) -> Result<(), AuditError> {
        self.conn.execute("DELETE FROM conflicts", [])?;
        self.conn.execute(
            "INSERT INTO conflicts (branch, target_branch, target_commit) VALUES (?1, ?2, ?3)",
            params![branch, target_branch, target_commit],
        )?;
        Ok(())
    }

    /// The in-flight conflict, if one is being resolved.
    pub fn current(&self) -> Result<Option<ConflictRecord>, AuditError> {
        let mut stmt = self.conn.prepare(
            "SELECT branch, target_branch, target_commit FROM conflicts ORDER BY id LIMIT 1",
        )?;
        let mut rows = stmt.query_map([], |row| {
            Ok(ConflictRecord {
                branch: row.get(0)?,
                target_branch: row.get(1)?,
                target_commit: row.get(2)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    pub fn clear(&self) -> Result<(), AuditError> {
        self.conn.execute("DELETE FROM conflicts", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn open_log() -> (tempfile::TempDir, ConflictLog) {
        let dir = tempdir().unwrap();
        let log = ConflictLog::open(dir.path().join("audit.sqlite3")).unwrap();
        (dir, log)
    }

    #[test]
    fn start_current_clear_roundtrip() {
        let (_d, log) = open_log();
        assert!(log.current().unwrap().is_none());

        log.start("feature/a", "develop", "abc123").unwrap();
        assert_eq!(
            log.current().unwrap().unwrap(),
            ConflictRecord {
                branch: "feature/a".into(),
                target_branch: "develop".into(),
                target_commit: "abc123".into(),
            }
        );

        log.clear().unwrap();
        assert!(log.current().unwrap().is_none());
    }

    #[test]
    fn start_replaces_prior() {
        let (_d, log) = open_log();
        log.start("feature/a", "develop", "aaa").unwrap();
        log.start("feature/b", "develop", "bbb").unwrap();

        assert_eq!(log.current().unwrap().unwrap().branch, "feature/b");
    }
}
