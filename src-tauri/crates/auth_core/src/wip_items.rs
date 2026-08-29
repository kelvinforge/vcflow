use std::path::Path;

use chrono::Utc;
use rusqlite::{params, Connection};

use crate::audit::AuditError;

const MIGRATION_0007: &str = include_str!("../migrations/0007_wip_items.sql");

/// A work-in-progress lifecycle record. Separate concept from `saved_work`
/// (linked only by `repository + branch`). See `0007_wip_items.sql`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WipItem {
    pub id: i64,
    pub repository: String,
    pub branch: String,
    /// `feature` | `bug` | `chore` | `hotfix`.
    pub work_type: String,
    /// `active` | `waiting` | `completed` | `dropped`.
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

const SELECT_COLS: &str = "id, repository, branch, work_type, status, created_at, updated_at";

pub struct WipItemLog {
    conn: Connection,
}

impl WipItemLog {
    /// Opens (creating if needed) the WIP-item log. Shares the file with
    /// `AuditLog` -- pass `AuditLog::default_path()`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AuditError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(MIGRATION_0007)?;
        Ok(Self { conn })
    }

    /// Records that the user started work on `branch`, or -- if a row for this
    /// `(repository, branch)` already exists -- re-activates it (a branch the
    /// user came back to). `created_at` is preserved on re-activation.
    pub fn start(&self, repository: &str, branch: &str, work_type: &str) -> Result<(), AuditError> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO wip_items (repository, branch, work_type, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'active', ?4, ?4)
             ON CONFLICT(repository, branch) DO UPDATE SET
                 status = 'active',
                 work_type = excluded.work_type,
                 updated_at = excluded.updated_at",
            params![repository, branch, work_type, now],
        )?;
        Ok(())
    }

    /// Adds a row for a branch that predates this table (no `create_work_item`
    /// ran through the app), but only when none exists -- an existing row,
    /// including `dropped` / `completed`, is left untouched so a deliberately
    /// dropped branch does not resurrect itself.
    pub fn backfill(
        &self,
        repository: &str,
        branch: &str,
        work_type: &str,
    ) -> Result<(), AuditError> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO wip_items (repository, branch, work_type, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'active', ?4, ?4)
             ON CONFLICT(repository, branch) DO NOTHING",
            params![repository, branch, work_type, now],
        )?;
        Ok(())
    }

    /// Sets the lifecycle status for a `(repository, branch)` pair. No-op if
    /// no such row exists (a branch created before this table shipped).
    pub fn set_status(&self, repository: &str, branch: &str, status: &str) -> Result<(), AuditError> {
        self.conn.execute(
            "UPDATE wip_items SET status = ?3, updated_at = ?4
             WHERE repository = ?1 AND branch = ?2",
            params![repository, branch, status, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Items still worth showing for `repository`: `active` and `waiting`,
    /// newest first. `completed` and `dropped` are excluded.
    pub fn actionable(&self, repository: &str) -> Result<Vec<WipItem>, AuditError> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM wip_items
             WHERE repository = ?1 AND status IN ('active', 'waiting')
             ORDER BY id DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![repository], row_to_item)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get(&self, id: i64) -> Result<Option<WipItem>, AuditError> {
        let sql = format!("SELECT {SELECT_COLS} FROM wip_items WHERE id = ?1");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map(params![id], row_to_item)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }
}

fn row_to_item(row: &rusqlite::Row) -> rusqlite::Result<WipItem> {
    Ok(WipItem {
        id: row.get(0)?,
        repository: row.get(1)?,
        branch: row.get(2)?,
        work_type: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn open_log() -> (tempfile::TempDir, WipItemLog) {
        let dir = tempdir().unwrap();
        let log = WipItemLog::open(dir.path().join("audit.sqlite3")).unwrap();
        (dir, log)
    }

    #[test]
    fn start_lists_as_active_scoped_by_repo() {
        let (_d, log) = open_log();
        log.start("/a", "feature/x", "feature").unwrap();
        log.start("/b", "bug/y", "bug").unwrap();

        let a = log.actionable("/a").unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].branch, "feature/x");
        assert_eq!(a[0].status, "active");
    }

    #[test]
    fn lifecycle_waiting_then_completed_drops_out() {
        let (_d, log) = open_log();
        log.start("/a", "feature/x", "feature").unwrap();
        log.set_status("/a", "feature/x", "waiting").unwrap();
        assert_eq!(log.actionable("/a").unwrap()[0].status, "waiting");

        log.set_status("/a", "feature/x", "completed").unwrap();
        assert!(log.actionable("/a").unwrap().is_empty());
    }

    #[test]
    fn re_start_reactivates_and_keeps_created_at() {
        let (_d, log) = open_log();
        log.start("/a", "feature/x", "feature").unwrap();
        let created = log.actionable("/a").unwrap()[0].created_at.clone();
        log.set_status("/a", "feature/x", "dropped").unwrap();
        assert!(log.actionable("/a").unwrap().is_empty());

        log.start("/a", "feature/x", "feature").unwrap();
        let rows = log.actionable("/a").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "active");
        assert_eq!(rows[0].created_at, created);
    }

    #[test]
    fn set_status_on_unknown_branch_is_noop() {
        let (_d, log) = open_log();
        log.set_status("/a", "feature/nope", "completed").unwrap();
        assert!(log.actionable("/a").unwrap().is_empty());
    }

    #[test]
    fn backfill_inserts_once_and_never_resurrects_dropped() {
        let (_d, log) = open_log();
        log.backfill("/a", "feature/old", "feature").unwrap();
        assert_eq!(log.actionable("/a").unwrap()[0].status, "active");

        log.set_status("/a", "feature/old", "dropped").unwrap();
        log.backfill("/a", "feature/old", "feature").unwrap();
        assert!(log.actionable("/a").unwrap().is_empty(), "dropped stays dropped");
    }

    #[test]
    fn get_missing_is_none() {
        let (_d, log) = open_log();
        assert!(log.get(999).unwrap().is_none());
    }
}
