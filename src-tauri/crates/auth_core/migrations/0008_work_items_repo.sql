-- Scopes the MR-cache to the repository each row belongs to.
--
-- The 0004 table had a table-level UNIQUE(branch, target_branch). Two repos
-- using the same branch name (notably the fixed `feature/initial`) could not
-- both cache an MR, and `mrs_for_branch` could hand a poll the wrong repo's
-- MR iid. ALTER TABLE cannot replace a table-level constraint, so this rebuilds
-- the table with UNIQUE(repo_path, branch, target_branch).
--
-- Pre-existing rows carry repo_path = NULL: they predate multi-repo use, and
-- reads treat NULL as "matches any repo" (see `mrs_for_branch`).
--
-- ponytail: no migration runner, and this is NOT idempotent. It is guarded in
-- WorkItemLog::open -- run only when the `repo_path` column is still absent.
BEGIN;

CREATE TABLE work_items_new (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    repo_path     TEXT,
    branch        TEXT NOT NULL,
    target_branch TEXT NOT NULL,
    mr_iid        TEXT NOT NULL,
    UNIQUE(repo_path, branch, target_branch)
);

INSERT INTO work_items_new (id, repo_path, branch, target_branch, mr_iid)
    SELECT id, NULL, branch, target_branch, mr_iid FROM work_items;

DROP TABLE work_items;
ALTER TABLE work_items_new RENAME TO work_items;

COMMIT;
