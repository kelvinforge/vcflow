-- Audit log: one row per sensitive action the app takes. No tokens,
-- passwords, SSH keys, or full diffs are ever written here.
CREATE TABLE IF NOT EXISTS audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL,
    user TEXT NOT NULL,
    provider TEXT NOT NULL,
    repository TEXT NOT NULL,
    branch TEXT,
    mr_pr TEXT,
    action TEXT NOT NULL,
    result TEXT NOT NULL,
    error TEXT
);
