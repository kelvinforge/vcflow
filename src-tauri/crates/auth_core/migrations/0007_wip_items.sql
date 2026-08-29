-- Work-in-progress lifecycle records: one row per workflow branch the user
-- has started in a repo, so the app can show "what am I in the middle of,
-- and can I safely go back to it" without shelling out to `git checkout`.
--
-- Deliberately NOT a task tracker: no title, body, assignee, labels, or
-- ordering. Status is driven by git/MR facts (branch created -> `active`,
-- MR opened -> `waiting`, MR merged -> `completed`) plus one explicit user
-- action (`dropped`).
--
-- Separate concept from `saved_work` (that table stays exactly as-is): a
-- WIP item is linked to its Saved Work purely by `repository + branch`,
-- never by a foreign key. `develop`/`master` are never rows here.
--
--   status: 'active'    -- unfinished, user can continue
--           'waiting'    -- handed off (MR open), nothing to do now
--           'completed'  -- MR merged; drops out of the list
--           'dropped'    -- user abandoned it; drops out of the list
--
-- Additive only. No token, diff, or file-content columns.
CREATE TABLE IF NOT EXISTS wip_items (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    repository  TEXT NOT NULL,
    branch      TEXT NOT NULL,
    work_type   TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    UNIQUE(repository, branch)
);
