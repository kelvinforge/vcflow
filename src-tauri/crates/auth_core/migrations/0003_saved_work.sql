-- One row per Work Safe "Saved Work" entry: a stash the app created (or the
-- user asked for) so a workflow step could run on a clean tree. `stash_oid`
-- ties the row to the exact git stash commit; the stash itself holds the
-- data, this table is the durable, human-labelled index and lifecycle.
-- `status` is 'saved' | 'restored' | 'discarded'. No file contents / diff /
-- token columns -- secrets must never reach this table.
CREATE TABLE IF NOT EXISTS saved_work (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp  TEXT    NOT NULL,
    repository TEXT    NOT NULL,
    branch     TEXT    NOT NULL,
    stash_oid  TEXT    NOT NULL,
    label      TEXT    NOT NULL DEFAULT '',
    status     TEXT    NOT NULL DEFAULT 'saved'
);
