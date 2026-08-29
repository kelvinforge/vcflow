-- One row per mutating git/provider operation actually executed by the app.
-- Deliberately has NO stdout / stderr / diff / token columns: only a masked
-- one-line arg summary and, on failure, a masked error message. Secrets must
-- never reach this table.
CREATE TABLE IF NOT EXISTS command_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp   TEXT    NOT NULL,
    repository  TEXT    NOT NULL,
    operation   TEXT    NOT NULL,
    args        TEXT    NOT NULL DEFAULT '',
    outcome     TEXT    NOT NULL,
    duration_ms INTEGER NOT NULL,
    error       TEXT
);
