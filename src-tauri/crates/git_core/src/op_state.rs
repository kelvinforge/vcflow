use git2::{Repository, RepositoryState};

/// A git operation left half-finished in the repo -- a merge, rebase,
/// cherry-pick, revert or bisect that stopped (usually on a conflict) and was
/// never concluded or aborted.
///
/// Work Safe treats any of these as a hard STOP: the workflow must not run
/// branch/fetch/pull/MR actions while one is open. The user finishes or aborts
/// it in the real working directory first.
///
/// Read-only: derived purely from `repo.state()`, which only inspects the
/// presence of `.git/MERGE_HEAD`, `.git/rebase-merge/`, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InProgressOp {
    Merge,
    Revert,
    CherryPick,
    Rebase,
    Bisect,
    ApplyMailbox,
}

impl InProgressOp {
    /// Short lower-case label for logs / UI (`"merge"`, `"rebase"`, ...).
    pub fn label(self) -> &'static str {
        match self {
            InProgressOp::Merge => "merge",
            InProgressOp::Revert => "revert",
            InProgressOp::CherryPick => "cherry-pick",
            InProgressOp::Rebase => "rebase",
            InProgressOp::Bisect => "bisect",
            InProgressOp::ApplyMailbox => "am",
        }
    }
}

/// `None` -> repo is `RepositoryState::Clean`, nothing half-done.
/// `Some(op)` -> that operation is open and must be resolved before the
/// workflow proceeds.
pub fn in_progress_operation(repo: &Repository) -> Option<InProgressOp> {
    match repo.state() {
        RepositoryState::Clean => None,
        RepositoryState::Merge => Some(InProgressOp::Merge),
        RepositoryState::Revert | RepositoryState::RevertSequence => Some(InProgressOp::Revert),
        RepositoryState::CherryPick | RepositoryState::CherryPickSequence => {
            Some(InProgressOp::CherryPick)
        }
        RepositoryState::Bisect => Some(InProgressOp::Bisect),
        RepositoryState::Rebase
        | RepositoryState::RebaseInteractive
        | RepositoryState::RebaseMerge => Some(InProgressOp::Rebase),
        RepositoryState::ApplyMailbox | RepositoryState::ApplyMailboxOrRebase => {
            Some(InProgressOp::ApplyMailbox)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use tempfile::{tempdir, TempDir};

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("failed to run git");
        assert!(status.success(), "git {:?} failed", args);
    }

    fn run_allow_fail(dir: &Path, args: &[&str]) {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("failed to run git");
    }

    fn commit(dir: &Path, file: &str, content: &str, msg: &str) {
        std::fs::write(dir.join(file), content).unwrap();
        run(dir, &["add", file]);
        run(dir, &["commit", "-m", msg]);
    }

    /// Repo on `develop` with commit A, plus branch `other` that conflicts
    /// with a later `develop` commit on the same file.
    fn conflicting_branches() -> TempDir {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "develop"]);
        run(dir.path(), &["config", "user.email", "t@e.com"]);
        run(dir.path(), &["config", "user.name", "T"]);
        commit(dir.path(), "f.txt", "base\n", "A");
        run(dir.path(), &["checkout", "-b", "other"]);
        commit(dir.path(), "f.txt", "other side\n", "B-other");
        run(dir.path(), &["checkout", "develop"]);
        commit(dir.path(), "f.txt", "develop side\n", "B-develop");
        dir
    }

    #[test]
    fn clean_repo_has_no_operation() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "develop"]);
        run(dir.path(), &["config", "user.email", "t@e.com"]);
        run(dir.path(), &["config", "user.name", "T"]);
        commit(dir.path(), "a.txt", "a\n", "A");

        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(in_progress_operation(&repo), None);
    }

    #[test]
    fn stopped_merge_is_detected() {
        let dir = conflicting_branches();
        run_allow_fail(dir.path(), &["merge", "other"]); // conflicts, stops

        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(in_progress_operation(&repo), Some(InProgressOp::Merge));
    }

    #[test]
    fn stopped_rebase_is_detected() {
        let dir = conflicting_branches();
        run_allow_fail(dir.path(), &["rebase", "other"]); // conflicts, stops

        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(in_progress_operation(&repo), Some(InProgressOp::Rebase));
    }

    #[test]
    fn stopped_cherry_pick_is_detected() {
        let dir = conflicting_branches();
        run_allow_fail(dir.path(), &["cherry-pick", "other"]); // conflicts, stops

        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(in_progress_operation(&repo), Some(InProgressOp::CherryPick));
    }

    #[test]
    fn concluded_merge_clears_the_flag() {
        let dir = conflicting_branches();
        run_allow_fail(dir.path(), &["merge", "other"]);
        // resolve + finish
        std::fs::write(dir.path().join("f.txt"), "resolved\n").unwrap();
        run(dir.path(), &["add", "f.txt"]);
        run(dir.path(), &["commit", "--no-edit"]);

        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(in_progress_operation(&repo), None);
    }

    #[test]
    fn aborted_merge_clears_the_flag() {
        let dir = conflicting_branches();
        run_allow_fail(dir.path(), &["merge", "other"]);
        run(dir.path(), &["merge", "--abort"]);

        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(in_progress_operation(&repo), None);
    }
}
