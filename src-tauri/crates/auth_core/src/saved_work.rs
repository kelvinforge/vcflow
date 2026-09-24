use std::path::Path;

use chrono::Utc;
use rusqlite::{params, Connection};

use crate::audit::AuditError;

const MIGRATION_0003: &str = include_str!("../migrations/0003_saved_work.sql");
const MIGRATION_0006: &str = include_str!("../migrations/0006_saved_work_origin.sql");

/// A Work Safe saved-work entry.
///
/// - `timestamp` is the creation time (the frontend contract's `created_at`).
/// - `branch` is the branch the work was saved from (the `original_branch`).
/// - `original_commit` is that branch's HEAD OID at save time (`""` if it
///   could not be read).
/// - `status` is `"saved"` | `"conflict"` | `"restored"` | `"discarded"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedWorkRecord {
    pub id: i64,
    pub timestamp: String,
    pub repository: String,
    pub branch: String,
    pub stash_oid: String,
    pub label: String,
    pub status: String,
    pub original_commit: String,
}

const SELECT_COLS: &str =
    "id, timestamp, repository, branch, stash_oid, label, status, original_commit";

pub struct SavedWorkLog {
    conn: Connection,
}

impl SavedWorkLog {
    /// Opens (creating if needed) the saved-work log. Shares the file with
    /// `AuditLog` -- pass `AuditLog::default_path()`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AuditError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(MIGRATION_0003)?;
        // ponytail: no migration runner; ADD COLUMN errors once the column
        // exists, so the failure is swallowed. Upgrade path: a real migrations
        // table if the schema ever grows past a handful of these.
        let _ = conn.execute_batch(MIGRATION_0006);
        Ok(Self { conn })
    }

    /// Records a new `saved` entry, returning its row id. `original_commit` is
    /// the branch HEAD OID at save time (`""` if unavailable).
    pub fn record(
        &self,
        repository: &str,
        branch: &str,
        stash_oid: &str,
        label: &str,
        original_commit: &str,
    ) -> Result<i64, AuditError> {
        self.conn.execute(
            "INSERT INTO saved_work
             (timestamp, repository, branch, stash_oid, label, status, original_commit)
             VALUES (?1, ?2, ?3, ?4, ?5, 'saved', ?6)",
            params![
                Utc::now().to_rfc3339(),
                repository,
                branch,
                stash_oid,
                label,
                original_commit
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_status(&self, id: i64, status: &str) -> Result<(), AuditError> {
        self.conn
            .execute("UPDATE saved_work SET status = ?2 WHERE id = ?1", params![id, status])?;
        Ok(())
    }

    /// Entries still available to resume for `repository`, newest first.
    pub fn open_entries(&self, repository: &str) -> Result<Vec<SavedWorkRecord>, AuditError> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM saved_work
             WHERE repository = ?1 AND status = 'saved' ORDER BY id DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![repository], row_to_record)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Entries the frontend should still surface for `repository`, newest
    /// first: `saved` (resumable), `conflict` (a resume collided and the entry
    /// was preserved), and `restored` (kept visible as history so the user sees
    /// a resume succeeded -- no longer actionable). `discarded` is dropped.
    /// Capped at 50 so restored history can't grow without bound.
    pub fn actionable_entries(&self, repository: &str) -> Result<Vec<SavedWorkRecord>, AuditError> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM saved_work
             WHERE repository = ?1 AND status IN ('saved', 'conflict', 'restored')
             ORDER BY id DESC LIMIT 50"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![repository], row_to_record)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// The newest still-resumable (`status = 'saved'`) entry for a specific
    /// branch in `repository`, if any. Used to answer "does this branch have
    /// Saved Work to restore" when the user comes back to it -- the
    /// `stash_oid` in the returned record is exact, never "latest stash".
    pub fn saved_for_branch(
        &self,
        repository: &str,
        branch: &str,
    ) -> Result<Option<SavedWorkRecord>, AuditError> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM saved_work
             WHERE repository = ?1 AND branch = ?2 AND status = 'saved'
             ORDER BY id DESC LIMIT 1"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map(params![repository, branch], row_to_record)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    pub fn get(&self, id: i64) -> Result<Option<SavedWorkRecord>, AuditError> {
        let sql = format!("SELECT {SELECT_COLS} FROM saved_work WHERE id = ?1");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map(params![id], row_to_record)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Atomically claims `id` for exclusive processing (resume/discard):
    /// flips it to the private `"processing"` status via a single
    /// `UPDATE ... WHERE status IN (...)`, but only when its current status
    /// is one of `allowed_from`. This -- not the per-repository lock in
    /// `repo_lock.rs` -- is what closes the TOCTOU between a caller's status
    /// check and its own status write: the repository and this log row are
    /// two different resources, so serializing git ops alone does not stop
    /// two commands from both reading `"saved"` before either writes back.
    ///
    /// Returns the record *as it was before the claim* when the claim
    /// succeeded -- the caller now exclusively owns finishing the
    /// transition, and on any failure MUST call
    /// `set_status(id, &that_record.status)` to release the claim, or the
    /// row is stuck as `"processing"` forever. Returns `None` when another
    /// caller already claimed/finished it, the row doesn't exist, or its
    /// status wasn't in `allowed_from` -- the caller cannot tell those apart
    /// and does not need to: all mean "there is nothing left for you to do
    /// here".
    pub fn try_claim(
        &self,
        id: i64,
        allowed_from: &[&str],
    ) -> Result<Option<SavedWorkRecord>, AuditError> {
        let before = match self.get(id)? {
            Some(r) => r,
            None => return Ok(None),
        };
        if !allowed_from.contains(&before.status.as_str()) {
            return Ok(None);
        }
        let placeholders: Vec<String> =
            (0..allowed_from.len()).map(|i| format!("?{}", i + 2)).collect();
        let sql = format!(
            "UPDATE saved_work SET status = 'processing' WHERE id = ?1 AND status IN ({})",
            placeholders.join(", ")
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&id];
        for s in allowed_from {
            params.push(s);
        }
        let changed = self.conn.execute(&sql, params.as_slice())?;
        Ok(if changed == 1 { Some(before) } else { None })
    }
}

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<SavedWorkRecord> {
    Ok(SavedWorkRecord {
        id: row.get(0)?,
        timestamp: row.get(1)?,
        repository: row.get(2)?,
        branch: row.get(3)?,
        stash_oid: row.get(4)?,
        label: row.get(5)?,
        status: row.get(6)?,
        original_commit: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn open_log() -> (tempfile::TempDir, SavedWorkLog) {
        let dir = tempdir().unwrap();
        let log = SavedWorkLog::open(dir.path().join("audit.sqlite3")).unwrap();
        (dir, log)
    }

    #[test]
    fn record_lists_and_status_transitions() {
        let (_d, log) = open_log();
        let id = log.record("/repo", "feature/x", "abc123", "wip", "headoid").unwrap();

        let open = log.open_entries("/repo").unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].branch, "feature/x");
        assert_eq!(open[0].status, "saved");
        assert_eq!(open[0].original_commit, "headoid");

        log.set_status(id, "restored").unwrap();
        assert!(log.open_entries("/repo").unwrap().is_empty());
        assert_eq!(log.get(id).unwrap().unwrap().status, "restored");
    }

    #[test]
    fn actionable_keeps_restored_history_drops_discarded() {
        let (_d, log) = open_log();
        let a = log.record("/repo", "b1", "o1", "", "").unwrap();
        let b = log.record("/repo", "b2", "o2", "", "").unwrap();
        let c = log.record("/repo", "b3", "o3", "", "").unwrap();
        log.set_status(a, "conflict").unwrap();
        log.set_status(b, "discarded").unwrap();
        log.set_status(c, "restored").unwrap();
        let got: Vec<_> = log
            .actionable_entries("/repo")
            .unwrap()
            .into_iter()
            .map(|r| r.status)
            .collect();
        assert_eq!(got, vec!["restored", "conflict"]);
    }

    #[test]
    fn open_entries_scoped_by_repo() {
        let (_d, log) = open_log();
        log.record("/a", "b1", "o1", "", "").unwrap();
        log.record("/b", "b2", "o2", "", "").unwrap();
        assert_eq!(log.open_entries("/a").unwrap().len(), 1);
    }

    #[test]
    fn saved_for_branch_picks_newest_saved_only() {
        let (_d, log) = open_log();
        let old = log.record("/repo", "feature/x", "oid-old", "", "").unwrap();
        log.record("/repo", "feature/x", "oid-new", "", "").unwrap();
        log.record("/repo", "feature/y", "oid-y", "", "").unwrap();
        log.set_status(old, "restored").unwrap();

        let hit = log.saved_for_branch("/repo", "feature/x").unwrap().unwrap();
        assert_eq!(hit.stash_oid, "oid-new");
        assert!(log.saved_for_branch("/repo", "feature/z").unwrap().is_none());
    }

    #[test]
    fn get_missing_is_none() {
        let (_d, log) = open_log();
        assert!(log.get(999).unwrap().is_none());
    }

    #[test]
    fn try_claim_succeeds_once_and_locks_out_a_second_racer() {
        let (_d, log) = open_log();
        let id = log.record("/repo", "feature/x", "abc123", "wip", "").unwrap();

        let first = log.try_claim(id, &["saved"]).unwrap();
        assert_eq!(first.unwrap().status, "saved");

        // A second caller racing the same id -- same call a real resume_work
        // and discard_work would both make -- must not also claim it.
        let second = log.try_claim(id, &["saved"]).unwrap();
        assert!(second.is_none());
        assert_eq!(log.get(id).unwrap().unwrap().status, "processing");
    }

    #[test]
    fn try_claim_rejects_a_status_not_in_allowed_from() {
        let (_d, log) = open_log();
        let id = log.record("/repo", "feature/x", "abc123", "wip", "").unwrap();
        log.set_status(id, "restored").unwrap();

        assert!(log.try_claim(id, &["saved", "conflict"]).unwrap().is_none());
        // Rejected claim must not touch the row.
        assert_eq!(log.get(id).unwrap().unwrap().status, "restored");
    }

    #[test]
    fn try_claim_on_missing_id_is_none() {
        let (_d, log) = open_log();
        assert!(log.try_claim(999, &["saved"]).unwrap().is_none());
    }

    #[test]
    fn caller_can_release_a_claim_back_to_its_original_status() {
        let (_d, log) = open_log();
        let id = log.record("/repo", "feature/x", "abc123", "wip", "").unwrap();

        let claimed = log.try_claim(id, &["saved"]).unwrap().unwrap();
        // Simulate the caller's git op failing: release the claim.
        log.set_status(id, &claimed.status).unwrap();

        assert_eq!(log.get(id).unwrap().unwrap().status, "saved");
        // Now resumable again by a fresh claim.
        assert!(log.try_claim(id, &["saved"]).unwrap().is_some());
    }
}
