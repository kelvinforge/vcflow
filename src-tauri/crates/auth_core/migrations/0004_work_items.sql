-- Maps a local branch to the MR(s) opened for it, so a later poll
-- (`get_mr_status`/`get_hotfix_status`) knows which MR(s) to ask the provider
-- about without the user re-entering anything. A hotfix branch has two rows
-- (master + develop); a feature/bug/chore branch has one (develop). Not a
-- source of truth -- only a convenience cache the provider API can outrank.
-- UNIQUE(branch, target_branch) makes re-opening an MR an upsert. No token
-- or diff columns -- secrets must never reach this table.
CREATE TABLE IF NOT EXISTS work_items (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    branch        TEXT NOT NULL,
    target_branch TEXT NOT NULL,
    mr_iid        TEXT NOT NULL,
    UNIQUE(branch, target_branch)
);
