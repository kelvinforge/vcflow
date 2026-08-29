-- The single in-flight conflict resolution: which branch the resolved merge
-- gets pushed back to (`branch`), which branch was merged in (`target_branch`)
-- and the exact commit merged (`target_commit`, hex OID) so the resolution
-- can be recorded as a real two-parent merge commit. At most one row -- the
-- three separate Tauri commands (start/open/verify) read it to agree on the
-- same merge. Not a source of truth: `MERGE_HEAD` in the repo is. No token
-- or file-content columns.
CREATE TABLE IF NOT EXISTS conflicts (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    branch        TEXT NOT NULL,
    target_branch TEXT NOT NULL,
    target_commit TEXT NOT NULL
);
