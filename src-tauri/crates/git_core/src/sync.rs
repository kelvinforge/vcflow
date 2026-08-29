use git2::{build::CheckoutBuilder, Repository};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("ref not found: {0}")]
    RefNotFound(String),
    #[error("local '{branch}' and origin/{branch} have diverged ({ahead} local, {behind} remote) -- reconcile it in the working directory first")]
    Diverged {
        branch: String,
        ahead: usize,
        behind: usize,
    },
    #[error(transparent)]
    Git(#[from] git2::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastForward {
    /// Local already at or ahead of origin -- nothing pulled.
    AlreadyCurrent,
    /// Local ref moved up to origin by `commits` commits.
    Advanced { commits: usize },
}

/// Bring local `branch` up to `origin/<branch>` by fast-forward only.
///
/// Reads the remote-tracking ref only -- run `fetch_origin` first to make it
/// current. Diverged (both sides carry unique commits) -> `Err(Diverged)`,
/// never auto-resolved (Work Safe). If `branch` is the checked-out HEAD its
/// working tree is updated too; callers guard for a clean tree beforehand.
pub fn fast_forward_from_origin(repo: &Repository, branch: &str) -> Result<FastForward, SyncError> {
    let local_ref = format!("refs/heads/{branch}");
    let remote_ref = format!("refs/remotes/origin/{branch}");

    let local = repo
        .revparse_single(&local_ref)
        .map_err(|_| SyncError::RefNotFound(local_ref.clone()))?
        .peel_to_commit()?
        .id();
    let remote = repo
        .revparse_single(&remote_ref)
        .map_err(|_| SyncError::RefNotFound(remote_ref))?
        .peel_to_commit()?
        .id();

    if local == remote {
        return Ok(FastForward::AlreadyCurrent);
    }

    let (ahead, behind) = repo.graph_ahead_behind(local, remote)?;
    if ahead > 0 && behind > 0 {
        return Err(SyncError::Diverged {
            branch: branch.to_string(),
            ahead,
            behind,
        });
    }
    if behind == 0 {
        // Local strictly ahead -- nothing to fast-forward.
        return Ok(FastForward::AlreadyCurrent);
    }

    repo.find_reference(&local_ref)?
        .set_target(remote, "fast-forward from origin")?;

    let on_this_branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(str::to_string))
        .as_deref()
        == Some(branch);
    if on_this_branch {
        // Callers guarantee a clean tree (Work Safe guard), so a forced
        // checkout only ever replays the fast-forwarded commits.
        repo.checkout_head(Some(CheckoutBuilder::new().force()))?;
    }

    Ok(FastForward::Advanced { commits: behind })
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

    fn init_repo() -> TempDir {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "master"]);
        run(dir.path(), &["config", "user.email", "t@e.com"]);
        run(dir.path(), &["config", "user.name", "T"]);
        commit(dir.path(), "a.txt", "A");
        dir
    }

    #[test]
    fn behind_origin_fast_forwards() {
        let dir = init_repo();
        commit(dir.path(), "b.txt", "B");
        run(dir.path(), &["update-ref", "refs/remotes/origin/master", "HEAD"]);
        run(dir.path(), &["reset", "--hard", "HEAD~1"]);

        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(
            fast_forward_from_origin(&repo, "master").unwrap(),
            FastForward::Advanced { commits: 1 }
        );
        // Working tree now carries the pulled commit.
        assert!(dir.path().join("b.txt").exists());
    }

    #[test]
    fn diverged_is_rejected() {
        let dir = init_repo();
        run(dir.path(), &["update-ref", "refs/remotes/origin/master", "master"]);
        commit(dir.path(), "local.txt", "local");
        run(dir.path(), &["checkout", "-b", "tmp", "refs/remotes/origin/master"]);
        commit(dir.path(), "remote.txt", "remote");
        run(dir.path(), &["update-ref", "refs/remotes/origin/master", "tmp"]);
        run(dir.path(), &["checkout", "master"]);

        let repo = Repository::open(dir.path()).unwrap();
        assert!(matches!(
            fast_forward_from_origin(&repo, "master"),
            Err(SyncError::Diverged { .. })
        ));
    }

    #[test]
    fn in_sync_is_noop() {
        let dir = init_repo();
        run(dir.path(), &["update-ref", "refs/remotes/origin/master", "master"]);
        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(
            fast_forward_from_origin(&repo, "master").unwrap(),
            FastForward::AlreadyCurrent
        );
    }
}
