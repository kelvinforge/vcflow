use std::path::Path;

use rusqlite::{params, Connection};

use crate::audit::AuditError;

const MIGRATION_0004: &str = include_str!("../migrations/0004_work_items.sql");
const MIGRATION_0008: &str = include_str!("../migrations/0008_work_items_repo.sql");

/// One MR opened for a branch: which branch it targets and the provider's
/// MR iid to poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItemMr {
    pub target_branch: String,
    pub mr_iid: String,
}

pub struct WorkItemLog {
    conn: Connection,
}

impl WorkItemLog {
    /// Opens (creating if needed) the work-item log. Shares the file with
    /// `AuditLog` -- pass `AuditLog::default_path()`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AuditError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(MIGRATION_0004)?;
        // ponytail: no migration runner. 0008 replaces a table-level UNIQUE,
        // which ALTER TABLE cannot do, so it rebuilds the table and is NOT
        // idempotent -- run it only while `repo_path` is still absent.
        if !has_column(&conn, "work_items", "repo_path")? {
            conn.execute_batch(MIGRATION_0008)?;
        }
        Ok(Self { conn })
    }

    /// Records (or replaces) the MR opened for `branch` against `target_branch`
    /// in `repo_path`.
    pub fn add_mr(
        &self,
        repo_path: &str,
        branch: &str,
        target_branch: &str,
        mr_iid: &str,
    ) -> Result<(), AuditError> {
        self.conn.execute(
            "INSERT INTO work_items (repo_path, branch, target_branch, mr_iid)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(repo_path, branch, target_branch)
             DO UPDATE SET mr_iid = excluded.mr_iid",
            params![repo_path, branch, target_branch, mr_iid],
        )?;
        Ok(())
    }

    /// Every MR tracked for `branch` in `repo_path`. Legacy rows with a NULL
    /// `repo_path` (written before the MR-cache was repo-scoped) match any repo.
    pub fn mrs_for_branch(
        &self,
        repo_path: &str,
        branch: &str,
    ) -> Result<Vec<WorkItemMr>, AuditError> {
        let mut stmt = self.conn.prepare(
            "SELECT target_branch, mr_iid FROM work_items
             WHERE branch = ?2 AND (repo_path = ?1 OR repo_path IS NULL)
             ORDER BY id",
        )?;
        let rows = stmt.query_map(params![repo_path, branch], |row| {
            Ok(WorkItemMr {
                target_branch: row.get(0)?,
                mr_iid: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, AuditError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn open_log() -> (tempfile::TempDir, WorkItemLog) {
        let dir = tempdir().unwrap();
        let log = WorkItemLog::open(dir.path().join("audit.sqlite3")).unwrap();
        (dir, log)
    }

    #[test]
    fn add_and_list_mrs() {
        let (_d, log) = open_log();
        log.add_mr("/repo", "hotfix/x", "master", "10").unwrap();
        log.add_mr("/repo", "hotfix/x", "develop", "11").unwrap();

        let mrs = log.mrs_for_branch("/repo", "hotfix/x").unwrap();
        assert_eq!(mrs.len(), 2);
        assert_eq!(mrs[0], WorkItemMr { target_branch: "master".into(), mr_iid: "10".into() });
    }

    #[test]
    fn re_adding_same_target_upserts() {
        let (_d, log) = open_log();
        log.add_mr("/repo", "feature/a", "develop", "1").unwrap();
        log.add_mr("/repo", "feature/a", "develop", "2").unwrap();

        let mrs = log.mrs_for_branch("/repo", "feature/a").unwrap();
        assert_eq!(mrs.len(), 1);
        assert_eq!(mrs[0].mr_iid, "2");
    }

    #[test]
    fn unknown_branch_is_empty() {
        let (_d, log) = open_log();
        assert!(log.mrs_for_branch("/repo", "nope").unwrap().is_empty());
    }

    #[test]
    fn reopening_does_not_rerun_the_rebuild_or_lose_rows() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.sqlite3");
        {
            let log = WorkItemLog::open(&path).unwrap();
            log.add_mr("/repo", "feature/a", "develop", "7").unwrap();
        }
        // Second open: `repo_path` already exists, so 0008 must be skipped and
        // the row must survive (a re-run would blank repo_path back to NULL).
        let log = WorkItemLog::open(&path).unwrap();
        let mrs = log.mrs_for_branch("/repo", "feature/a").unwrap();
        assert_eq!(mrs.len(), 1);
        assert_eq!(mrs[0].mr_iid, "7");
    }

    #[test]
    fn same_branch_name_is_isolated_per_repo() {
        let (_d, log) = open_log();
        log.add_mr("/repo-a", "feature/initial", "develop", "100").unwrap();
        log.add_mr("/repo-b", "feature/initial", "develop", "200").unwrap();

        let a = log.mrs_for_branch("/repo-a", "feature/initial").unwrap();
        let b = log.mrs_for_branch("/repo-b", "feature/initial").unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].mr_iid, "100");
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].mr_iid, "200");
    }
}
