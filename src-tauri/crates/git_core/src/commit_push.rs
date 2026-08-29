use std::cell::RefCell;

use git2::{PushOptions, RemoteCallbacks, Repository, Signature};
use thiserror::Error;

use crate::ssh::make_credentials_callback;

#[derive(Debug, Error)]
pub enum CommitPushError {
    #[error("nothing to commit -- working tree is clean")]
    NothingToCommit,
    #[error("could not push: {0}")]
    PushFailed(String),
    #[error(transparent)]
    Git(#[from] git2::Error),
}

/// Stages all changes (tracked + untracked, respecting `.gitignore`) and
/// commits them under the repo's configured user identity.
pub fn commit_all(repo: &Repository, message: &str) -> Result<git2::Oid, CommitPushError> {
    let mut index = repo.index()?;
    index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
    index.write()?;

    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    let head = repo.head()?;
    let parent = head.peel_to_commit()?;

    if tree_id == parent.tree_id() {
        return Err(CommitPushError::NothingToCommit);
    }

    let sig = repo
        .signature()
        .or_else(|_| Signature::now("git-workflow-engine", "noreply@localhost"))?;

    let oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
    Ok(oid)
}

/// Commits a resolved merge as a proper two-parent merge commit (current
/// HEAD + `other_parent`), then clears the repository's merging state.
/// Conflict resolution must never collapse this into a single-parent commit
/// -- that would silently erase the merge ancestry from history.
pub fn commit_merge(
    repo: &Repository,
    other_parent: git2::Oid,
    message: &str,
) -> Result<git2::Oid, CommitPushError> {
    let mut index = repo.index()?;
    index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
    index.write()?;

    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    let head = repo.head()?;
    let head_commit = head.peel_to_commit()?;
    let other_commit = repo.find_commit(other_parent)?;

    let sig = repo
        .signature()
        .or_else(|_| Signature::now("git-workflow-engine", "noreply@localhost"))?;

    let oid = repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        message,
        &tree,
        &[&head_commit, &other_commit],
    )?;
    repo.cleanup_state()?;
    Ok(oid)
}

/// Pushes `branch` to `origin`, creating the matching remote branch if it
/// doesn't exist yet. Never touches `master`/`develop` directly -- callers
/// only ever push a `feature/*`/`bug/*`/`chore/*`/`hotfix/*` branch here.
pub fn push(repo: &Repository, branch: &str) -> Result<(), CommitPushError> {
    let mut remote = repo.find_remote("origin")?;
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");

    // `Remote::push` reports transport-level failures as `Err`, but a
    // server-side rejection (e.g. protected branch, non-fast-forward) comes
    // back as `Ok` with the rejection reason only visible via this callback.
    let rejection = RefCell::new(None);
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(make_credentials_callback());
    callbacks.push_update_reference(|_refname, status| {
        if let Some(msg) = status {
            *rejection.borrow_mut() = Some(msg.to_string());
        }
        Ok(())
    });

    let mut opts = PushOptions::new();
    opts.remote_callbacks(callbacks);

    let push_result = remote
        .push(&[&refspec], Some(&mut opts))
        .map_err(|e| CommitPushError::PushFailed(e.message().to_string()));
    drop(opts);
    push_result?;

    if let Some(msg) = rejection.into_inner() {
        return Err(CommitPushError::PushFailed(msg));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn run(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("failed to run git");
        assert!(status.success(), "git {:?} failed", args);
    }

    #[test]
    fn commits_staged_changes() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "main"]);
        run(dir.path(), &["config", "user.email", "test@example.com"]);
        run(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("a.txt"), "1").unwrap();
        run(dir.path(), &["add", "a.txt"]);
        run(dir.path(), &["commit", "-m", "init"]);

        std::fs::write(dir.path().join("a.txt"), "2").unwrap();
        let repo = Repository::open(dir.path()).unwrap();
        let oid = commit_all(&repo, "update a").unwrap();

        let commit = repo.find_commit(oid).unwrap();
        assert_eq!(commit.message(), Some("update a"));
    }

    #[test]
    fn commit_merge_records_two_parents() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "main"]);
        run(dir.path(), &["config", "user.email", "test@example.com"]);
        run(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("a.txt"), "1").unwrap();
        run(dir.path(), &["add", "a.txt"]);
        run(dir.path(), &["commit", "-m", "init"]);
        run(dir.path(), &["checkout", "-b", "other"]);
        std::fs::write(dir.path().join("b.txt"), "1").unwrap();
        run(dir.path(), &["add", "b.txt"]);
        run(dir.path(), &["commit", "-m", "other change"]);
        let other_oid = git2::Repository::open(dir.path())
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap();
        run(dir.path(), &["checkout", "main"]);
        std::fs::write(dir.path().join("a.txt"), "2").unwrap();

        let repo = Repository::open(dir.path()).unwrap();
        let oid = commit_merge(&repo, other_oid, "merge: resolve").unwrap();

        let commit = repo.find_commit(oid).unwrap();
        assert_eq!(commit.parent_count(), 2);
    }

    #[test]
    fn nothing_to_commit_is_reported() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "main"]);
        run(dir.path(), &["config", "user.email", "test@example.com"]);
        run(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("a.txt"), "1").unwrap();
        run(dir.path(), &["add", "a.txt"]);
        run(dir.path(), &["commit", "-m", "init"]);

        let repo = Repository::open(dir.path()).unwrap();
        let err = commit_all(&repo, "no-op").unwrap_err();
        assert!(matches!(err, CommitPushError::NothingToCommit));
    }
}
