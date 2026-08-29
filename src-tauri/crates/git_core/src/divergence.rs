use git2::Repository;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DivergenceError {
    #[error("ref not found: {0}")]
    RefNotFound(String),
    #[error(transparent)]
    Git(#[from] git2::Error),
}

/// How one ref sits relative to another, in commit counts.
///
/// `ahead` = commits reachable from `from` but not `to`.
/// `behind` = commits reachable from `to` but not `from`.
///
/// Used two ways:
/// - local vs its origin ref (is `develop` behind `origin/develop`? diverged?)
/// - `origin/master` vs `origin/develop` (does master carry commits develop
///   lacks -> a master->develop sync MR is owed, spec §13)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Divergence {
    pub ahead: usize,
    pub behind: usize,
}

impl Divergence {
    /// Same commit -- nothing to do.
    pub fn in_sync(&self) -> bool {
        self.ahead == 0 && self.behind == 0
    }

    /// `to` moved on, `from` did not -- a plain fast-forward pull would catch up.
    pub fn can_fast_forward(&self) -> bool {
        self.behind > 0 && self.ahead == 0
    }

    /// Both sides have unique commits -- never auto-resolve (spec §18).
    pub fn is_diverged(&self) -> bool {
        self.ahead > 0 && self.behind > 0
    }
}

fn resolve(repo: &Repository, name: &str) -> Result<git2::Oid, DivergenceError> {
    repo.revparse_single(name)
        .map_err(|_| DivergenceError::RefNotFound(name.to_string()))?
        .peel_to_commit()
        .map(|c| c.id())
        .map_err(|_| DivergenceError::RefNotFound(name.to_string()))
}

/// Compares two refs by name. Read-only; walks history only.
pub fn compare_refs(
    repo: &Repository,
    from: &str,
    to: &str,
) -> Result<Divergence, DivergenceError> {
    let from_oid = resolve(repo, from)?;
    let to_oid = resolve(repo, to)?;
    let (ahead, behind) = repo.graph_ahead_behind(from_oid, to_oid)?;
    Ok(Divergence { ahead, behind })
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

    fn commit(dir: &Path, file: &str, msg: &str) {
        std::fs::write(dir.join(file), msg).unwrap();
        run(dir, &["add", file]);
        run(dir, &["commit", "-m", msg]);
    }

    /// Repo on `develop` and `master`, both at the same initial commit.
    fn init_repo() -> TempDir {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "develop"]);
        run(dir.path(), &["config", "user.email", "t@e.com"]);
        run(dir.path(), &["config", "user.name", "T"]);
        commit(dir.path(), "a.txt", "A");
        commit(dir.path(), "b.txt", "B");
        commit(dir.path(), "c.txt", "C");
        run(dir.path(), &["branch", "master"]);
        dir
    }

    #[test]
    fn identical_refs_are_in_sync() {
        let dir = init_repo();
        let repo = Repository::open(dir.path()).unwrap();
        let d = compare_refs(&repo, "develop", "master").unwrap();
        assert!(d.in_sync());
        assert!(!d.is_diverged());
    }

    #[test]
    fn master_ahead_of_develop_means_sync_owed() {
        // spec §13: develop A-B-C, master A-B-C-D  ->  master->develop sync needed
        let dir = init_repo();
        run(dir.path(), &["checkout", "master"]);
        commit(dir.path(), "d.txt", "D hotfix");

        let repo = Repository::open(dir.path()).unwrap();
        let d = compare_refs(&repo, "master", "develop").unwrap();
        assert_eq!(d.ahead, 1, "master has 1 commit develop lacks");
        assert_eq!(d.behind, 0);
        assert!(!d.is_diverged());
        assert!(!d.in_sync());
    }

    #[test]
    fn equal_history_means_no_sync_owed() {
        // spec §13: A-B-C-D on both  ->  no sync
        let dir = init_repo();
        run(dir.path(), &["checkout", "master"]);
        commit(dir.path(), "d.txt", "D");
        run(dir.path(), &["checkout", "develop"]);
        run(dir.path(), &["merge", "--ff-only", "master"]);

        let repo = Repository::open(dir.path()).unwrap();
        let d = compare_refs(&repo, "master", "develop").unwrap();
        assert!(d.in_sync());
    }

    #[test]
    fn local_behind_origin_can_fast_forward() {
        let dir = init_repo();
        // simulate origin/develop ahead by tagging a ref, then moving develop back
        run(dir.path(), &["checkout", "develop"]);
        commit(dir.path(), "e.txt", "E");
        run(dir.path(), &["update-ref", "refs/remotes/origin/develop", "HEAD"]);
        run(dir.path(), &["reset", "--hard", "HEAD~1"]);

        let repo = Repository::open(dir.path()).unwrap();
        let d = compare_refs(&repo, "develop", "refs/remotes/origin/develop").unwrap();
        assert_eq!(d.behind, 1);
        assert_eq!(d.ahead, 0);
        assert!(d.can_fast_forward());
        assert!(!d.is_diverged());
    }

    #[test]
    fn both_sides_unique_commits_is_diverged() {
        let dir = init_repo();
        run(dir.path(), &["update-ref", "refs/remotes/origin/develop", "develop"]);
        // local gets its own commit
        commit(dir.path(), "local.txt", "local only");
        // origin/develop gets a different one
        run(dir.path(), &["checkout", "-b", "tmp", "refs/remotes/origin/develop"]);
        commit(dir.path(), "remote.txt", "remote only");
        run(dir.path(), &["update-ref", "refs/remotes/origin/develop", "tmp"]);
        run(dir.path(), &["checkout", "develop"]);

        let repo = Repository::open(dir.path()).unwrap();
        let d = compare_refs(&repo, "develop", "refs/remotes/origin/develop").unwrap();
        assert_eq!(d.ahead, 1);
        assert_eq!(d.behind, 1);
        assert!(d.is_diverged());
    }

    #[test]
    fn unknown_ref_is_reported() {
        let dir = init_repo();
        let repo = Repository::open(dir.path()).unwrap();
        assert!(matches!(
            compare_refs(&repo, "develop", "refs/remotes/origin/nope"),
            Err(DivergenceError::RefNotFound(_))
        ));
    }
}
