use git2::{Repository, Status, StatusOptions};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StatusError {
    #[error(transparent)]
    Git(#[from] git2::Error),
}

/// A rename, keeping both ends so the UI can show `old -> new`. Populated
/// from either the index-side or worktree-side rename, whichever git
/// detected (index-side wins when both exist).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenamedFile {
    pub from: String,
    pub to: String,
}

/// The real working-tree state, classified the way the workflow cares about
/// it rather than the way git's raw bitflags express it. A single file lands
/// in exactly one bucket: its most significant change wins (renamed >
/// deleted > added > modified), and an untracked path is never also counted
/// as added.
///
/// Ignored files are deliberately excluded -- they never make the tree
/// "dirty" for workflow purposes (branch-guard, Save Work).
///
/// This is a read-only view: building it never touches the working tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkingTreeStatus {
    pub modified: Vec<String>,
    pub added: Vec<String>,
    pub deleted: Vec<String>,
    pub renamed: Vec<RenamedFile>,
    pub untracked: Vec<String>,
}

impl WorkingTreeStatus {
    /// Any tracked change or untracked file present. Ignored files excluded
    /// (they're never collected in the first place).
    pub fn is_dirty(&self) -> bool {
        self.total_count() > 0
    }

    /// Total number of changed paths across every bucket.
    pub fn total_count(&self) -> usize {
        self.modified.len()
            + self.added.len()
            + self.deleted.len()
            + self.renamed.len()
            + self.untracked.len()
    }
}

/// Reads and classifies the working tree of `repo`. Combines the index side
/// (staged) and the worktree side (unstaged) into one per-file verdict --
/// the workflow doesn't distinguish "staged modified" from "unstaged
/// modified", only "this file changed".
pub fn read_working_tree_status(repo: &Repository) -> Result<WorkingTreeStatus, StatusError> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true)
        .include_ignored(false)
        .exclude_submodules(true);

    let statuses = repo.statuses(Some(&mut opts))?;
    let mut out = WorkingTreeStatus::default();

    for entry in statuses.iter() {
        let s = entry.status();

        if s.intersects(Status::WT_NEW | Status::INDEX_NEW) && is_untracked_only(s) {
            if let Some(p) = entry.path() {
                out.untracked.push(p.to_string());
            }
            continue;
        }

        if s.intersects(Status::INDEX_RENAMED | Status::WT_RENAMED) {
            let diff = if s.contains(Status::INDEX_RENAMED) {
                entry.head_to_index()
            } else {
                entry.index_to_workdir()
            };
            if let Some(rename) = diff
                .and_then(|d| rename_paths(d.old_file().path(), d.new_file().path()))
            {
                out.renamed.push(rename);
                continue;
            }
        }

        if s.intersects(Status::INDEX_DELETED | Status::WT_DELETED) {
            if let Some(p) = entry.path() {
                out.deleted.push(p.to_string());
            }
            continue;
        }

        if s.contains(Status::INDEX_NEW) {
            if let Some(p) = entry.path() {
                out.added.push(p.to_string());
            }
            continue;
        }

        if s.intersects(Status::INDEX_MODIFIED | Status::WT_MODIFIED | Status::INDEX_TYPECHANGE | Status::WT_TYPECHANGE)
        {
            if let Some(p) = entry.path() {
                out.modified.push(p.to_string());
            }
        }
    }

    Ok(out)
}

/// A path git flagged as new but that isn't a staged add-with-later-edits --
/// i.e. it exists only in the worktree, never in the index.
fn is_untracked_only(s: Status) -> bool {
    s.contains(Status::WT_NEW) && !s.contains(Status::INDEX_NEW)
}

fn rename_paths(old: Option<&std::path::Path>, new: Option<&std::path::Path>) -> Option<RenamedFile> {
    Some(RenamedFile {
        from: old?.to_string_lossy().to_string(),
        to: new?.to_string_lossy().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::{tempdir, TempDir};

    fn run(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("failed to run git");
        assert!(status.success(), "git {:?} failed", args);
    }

    /// Repo with one committed file `a.txt` on `develop`.
    fn init_repo() -> TempDir {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "develop"]);
        run(dir.path(), &["config", "user.email", "test@example.com"]);
        run(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        run(dir.path(), &["add", "a.txt"]);
        run(dir.path(), &["commit", "-m", "init"]);
        dir
    }

    #[test]
    fn clean_tree_is_not_dirty() {
        let dir = init_repo();
        let repo = Repository::open(dir.path()).unwrap();
        let status = read_working_tree_status(&repo).unwrap();
        assert!(!status.is_dirty());
        assert_eq!(status.total_count(), 0);
    }

    #[test]
    fn classifies_modified_added_deleted_untracked() {
        let dir = init_repo();
        // c.txt needs to already be committed so we can delete it below
        std::fs::write(dir.path().join("c.txt"), "c\n").unwrap();
        run(dir.path(), &["add", "c.txt"]);
        run(dir.path(), &["commit", "-m", "add c"]);

        // modified (tracked)
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        // added (staged new)
        std::fs::write(dir.path().join("b.txt"), "b\n").unwrap();
        run(dir.path(), &["add", "b.txt"]);
        // deleted (tracked, removed from disk)
        std::fs::remove_file(dir.path().join("c.txt")).unwrap();
        // untracked
        std::fs::write(dir.path().join("d.txt"), "d\n").unwrap();

        let repo = Repository::open(dir.path()).unwrap();
        let status = read_working_tree_status(&repo).unwrap();

        assert_eq!(status.modified, vec!["a.txt"]);
        assert_eq!(status.added, vec!["b.txt"]);
        assert_eq!(status.deleted, vec!["c.txt"]);
        assert_eq!(status.untracked, vec!["d.txt"]);
        assert!(status.is_dirty());
        assert_eq!(status.total_count(), 4);
    }

    #[test]
    fn detects_rename() {
        let dir = init_repo();
        run(dir.path(), &["mv", "a.txt", "renamed.txt"]);

        let repo = Repository::open(dir.path()).unwrap();
        let status = read_working_tree_status(&repo).unwrap();

        assert_eq!(
            status.renamed,
            vec![RenamedFile { from: "a.txt".into(), to: "renamed.txt".into() }]
        );
        assert!(status.modified.is_empty());
        assert!(status.deleted.is_empty());
        assert!(status.added.is_empty());
        assert_eq!(status.total_count(), 1);
    }

    #[test]
    fn untracked_is_not_counted_as_added() {
        let dir = init_repo();
        std::fs::write(dir.path().join("new.txt"), "x\n").unwrap();

        let repo = Repository::open(dir.path()).unwrap();
        let status = read_working_tree_status(&repo).unwrap();

        assert_eq!(status.untracked, vec!["new.txt"]);
        assert!(status.added.is_empty());
    }

    #[test]
    fn ignored_files_do_not_make_tree_dirty() {
        let dir = init_repo();
        std::fs::write(dir.path().join(".gitignore"), "ignored/\n*.log\n").unwrap();
        run(dir.path(), &["add", ".gitignore"]);
        run(dir.path(), &["commit", "-m", "add gitignore"]);

        std::fs::create_dir(dir.path().join("ignored")).unwrap();
        std::fs::write(dir.path().join("ignored/x.txt"), "x\n").unwrap();
        std::fs::write(dir.path().join("debug.log"), "log\n").unwrap();

        let repo = Repository::open(dir.path()).unwrap();
        let status = read_working_tree_status(&repo).unwrap();

        assert!(!status.is_dirty(), "ignored files must not count as dirty");
        assert_eq!(status.total_count(), 0);
    }

    #[test]
    fn staged_add_with_later_edit_is_added_not_untracked() {
        let dir = init_repo();
        std::fs::write(dir.path().join("e.txt"), "one\n").unwrap();
        run(dir.path(), &["add", "e.txt"]);
        std::fs::write(dir.path().join("e.txt"), "one\ntwo\n").unwrap();

        let repo = Repository::open(dir.path()).unwrap();
        let status = read_working_tree_status(&repo).unwrap();

        assert_eq!(status.added, vec!["e.txt"]);
        assert!(status.untracked.is_empty());
    }
}
