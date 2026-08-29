use git2::Repository;
use thiserror::Error;

use crate::branch::{create_work_branch, BranchError, BranchKind};
use crate::commit_push::{commit_all, CommitPushError};
use crate::version::{read_version_file, write_version_file, VersionError};

#[derive(Debug, Error)]
pub enum HotfixError {
    #[error(transparent)]
    Branch(#[from] BranchError),
    #[error(transparent)]
    Version(#[from] VersionError),
    #[error(transparent)]
    Commit(#[from] CommitPushError),
    #[error(transparent)]
    Git(#[from] git2::Error),
    #[error("repository has no working directory")]
    NoWorkdir,
}

/// Creates `hotfix/<slug>` off local `master` and auto-bumps + commits the
/// VERSION patch in one step -- no user confirm, unlike Release's bump.
///
/// Assumes a clean working tree with `master` already fast-forwarded to
/// origin: the Tauri command layer runs the Work Safe guard and
/// `fast_forward_from_origin` before calling this. No worktree isolation --
/// Work Safe is the only safety layer.
pub fn create_hotfix_branch(repo: &Repository, slug: &str) -> Result<String, HotfixError> {
    let branch_name = create_work_branch(repo, BranchKind::Hotfix, slug, "master")?;
    bump_and_commit(repo)?;
    Ok(branch_name)
}

fn bump_and_commit(repo: &Repository) -> Result<(), HotfixError> {
    let workdir = repo.workdir().ok_or(HotfixError::NoWorkdir)?;
    let current = read_version_file(workdir)?;
    let bumped = current.bump_patch();
    write_version_file(workdir, &bumped)?;
    commit_all(repo, &format!("chore: bump version to {bumped}"))?;
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
    fn creates_hotfix_branch_off_master_and_bumps_patch() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "master"]);
        run(dir.path(), &["config", "user.email", "test@example.com"]);
        run(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("VERSION"), "1.2.3\n").unwrap();
        run(dir.path(), &["add", "VERSION"]);
        run(dir.path(), &["commit", "-m", "init"]);

        let repo = Repository::open(dir.path()).unwrap();
        let branch_name = create_hotfix_branch(&repo, "urgent-fix").unwrap();
        assert_eq!(branch_name, "hotfix/urgent-fix");

        let head = repo.head().unwrap();
        assert_eq!(head.shorthand(), Some("hotfix/urgent-fix"));

        let version = std::fs::read_to_string(dir.path().join("VERSION")).unwrap();
        assert_eq!(version.trim(), "1.2.4");

        let commit = head.peel_to_commit().unwrap();
        assert_eq!(commit.message(), Some("chore: bump version to 1.2.4"));
    }
}
