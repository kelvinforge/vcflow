use git2::Repository;
use thiserror::Error;

use crate::divergence::{compare_refs, Divergence};
use crate::op_state::{in_progress_operation, InProgressOp};
use crate::status::{read_working_tree_status, StatusError, WorkingTreeStatus};

#[derive(Debug, Error)]
pub enum RepoStateError {
    #[error("HEAD is not a branch (detached HEAD)")]
    DetachedHead,
    #[error(transparent)]
    Git(#[from] git2::Error),
}

/// The read-only safety picture Work Safe needs before running any workflow
/// action: is the tree dirty, is a git operation half-finished, and how does
/// the current branch sit against its `origin/` counterpart.
///
/// Building it never touches the working tree, index, HEAD or any ref -- it is
/// safe to call on every Refresh and from background polling. It does NOT
/// fetch; callers that want fresh remote-tracking refs run `fetch_origin`
/// first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryState {
    pub current_branch: String,
    pub working_tree: WorkingTreeStatus,
    pub in_progress_op: Option<InProgressOp>,
    /// Current branch vs `origin/<branch>`. `None` when no such remote-tracking
    /// ref exists (new local branch never pushed).
    pub upstream: Option<Divergence>,
}

impl RepositoryState {
    /// Any condition that forces Work Safe to act before a workflow step:
    /// dirty tree, or an unfinished merge/rebase/cherry-pick/etc.
    pub fn needs_attention(&self) -> bool {
        self.working_tree.is_dirty() || self.in_progress_op.is_some()
    }
}

pub fn read_repository_state(repo: &Repository) -> Result<RepositoryState, RepoStateError> {
    let current_branch = crate::repo::head_branch(repo)?.ok_or(RepoStateError::DetachedHead)?;

    let working_tree = read_working_tree_status(repo).map_err(|StatusError::Git(g)| g)?;
    let in_progress_op = in_progress_operation(repo);

    let remote_ref = format!("refs/remotes/origin/{current_branch}");
    let upstream = match repo.revparse_single(&remote_ref) {
        Ok(_) => Some(compare_refs(repo, &current_branch, &remote_ref).map_err(map_div)?),
        Err(_) => None,
    };

    Ok(RepositoryState {
        current_branch,
        working_tree,
        in_progress_op,
        upstream,
    })
}

fn map_div(e: crate::divergence::DivergenceError) -> RepoStateError {
    match e {
        crate::divergence::DivergenceError::Git(g) => RepoStateError::Git(g),
        // ref existence was checked immediately before -- treat a later
        // resolve failure as a plain git error
        crate::divergence::DivergenceError::RefNotFound(name) => {
            RepoStateError::Git(git2::Error::from_str(&format!("ref not found: {name}")))
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
        Command::new("git").args(args).current_dir(dir).output().unwrap();
    }

    fn commit(dir: &Path, file: &str, content: &str, msg: &str) {
        std::fs::write(dir.join(file), content).unwrap();
        run(dir, &["add", file]);
        run(dir, &["commit", "-m", msg]);
    }

    fn init_repo() -> TempDir {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "develop"]);
        run(dir.path(), &["config", "user.email", "t@e.com"]);
        run(dir.path(), &["config", "user.name", "T"]);
        commit(dir.path(), "a.txt", "a\n", "A");
        dir
    }

    #[test]
    fn clean_repo_needs_no_attention_and_has_no_upstream() {
        let dir = init_repo();
        let repo = Repository::open(dir.path()).unwrap();
        let s = read_repository_state(&repo).unwrap();
        assert_eq!(s.current_branch, "develop");
        assert!(!s.needs_attention());
        assert!(s.upstream.is_none());
    }

    #[test]
    fn unborn_branch_reports_intended_branch_name() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "main"]);
        std::fs::write(dir.path().join("new.txt"), "x\n").unwrap();
        let repo = Repository::open(dir.path()).unwrap();
        let s = read_repository_state(&repo).unwrap();
        assert_eq!(s.current_branch, "main");
        assert!(s.upstream.is_none());
        assert!(s.working_tree.untracked.contains(&"new.txt".to_string()));
    }

    #[test]
    fn dirty_tree_needs_attention() {
        let dir = init_repo();
        std::fs::write(dir.path().join("a.txt"), "changed\n").unwrap();
        let repo = Repository::open(dir.path()).unwrap();
        let s = read_repository_state(&repo).unwrap();
        assert!(s.working_tree.is_dirty());
        assert!(s.needs_attention());
    }

    #[test]
    fn stopped_merge_needs_attention() {
        let dir = init_repo();
        run(dir.path(), &["checkout", "-b", "other"]);
        commit(dir.path(), "a.txt", "other\n", "B-other");
        run(dir.path(), &["checkout", "develop"]);
        commit(dir.path(), "a.txt", "develop\n", "B-develop");
        run_allow_fail(dir.path(), &["merge", "other"]);

        let repo = Repository::open(dir.path()).unwrap();
        let s = read_repository_state(&repo).unwrap();
        assert_eq!(s.in_progress_op, Some(InProgressOp::Merge));
        assert!(s.needs_attention());
    }

    #[test]
    fn upstream_behind_is_reported() {
        let dir = init_repo();
        commit(dir.path(), "b.txt", "b\n", "B");
        run(dir.path(), &["update-ref", "refs/remotes/origin/develop", "HEAD"]);
        run(dir.path(), &["reset", "--hard", "HEAD~1"]);

        let repo = Repository::open(dir.path()).unwrap();
        let s = read_repository_state(&repo).unwrap();
        let up = s.upstream.expect("upstream present");
        assert_eq!(up.behind, 1);
        assert_eq!(up.ahead, 0);
        assert!(up.can_fast_forward());
    }
}
