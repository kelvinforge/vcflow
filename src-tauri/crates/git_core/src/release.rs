use git2::{BranchType, Repository};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReleaseError {
    #[error("branch '{0}' already exists")]
    AlreadyExists(String),
    #[error("base branch '{0}' not found locally")]
    BaseNotFound(String),
    #[error(transparent)]
    Git(#[from] git2::Error),
}

/// Creates `branch_name` off `base` and checks it out. The caller supplies the
/// fully-resolved name (including any `-N` supersede suffix) -- collision and
/// naming logic lives in the command layer, which needs remote refs and
/// `wip_items` to decide. Unlike `create_work_branch` this takes a full branch
/// name and runs no slug validator, so `release/1.4.0-2` is accepted.
///
/// Does NOT bump `VERSION`, write `CHANGELOG.md`, or commit -- those are the
/// release-preparation steps the command layer performs with its own inputs
/// (typed `Version`, edited changelog body).
pub fn create_release_branch(
    repo: &Repository,
    branch_name: &str,
    base: &str,
) -> Result<(), ReleaseError> {
    if repo.find_branch(branch_name, BranchType::Local).is_ok() {
        return Err(ReleaseError::AlreadyExists(branch_name.to_string()));
    }
    let base_commit = repo
        .find_branch(base, BranchType::Local)
        .map_err(|_| ReleaseError::BaseNotFound(base.to_string()))?
        .get()
        .peel_to_commit()?;

    repo.branch(branch_name, &base_commit, false)?;
    let obj = repo.revparse_single(&format!("refs/heads/{branch_name}"))?;
    repo.checkout_tree(&obj, None)?;
    repo.set_head(&format!("refs/heads/{branch_name}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn run(dir: &std::path::Path, args: &[&str]) {
        let ok = Command::new("git").args(args).current_dir(dir).status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    fn seed() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "develop"]);
        run(dir.path(), &["config", "user.email", "t@e.com"]);
        run(dir.path(), &["config", "user.name", "T"]);
        std::fs::write(dir.path().join("VERSION"), "1.3.0\n").unwrap();
        run(dir.path(), &["add", "."]);
        run(dir.path(), &["commit", "-m", "init"]);
        dir
    }

    #[test]
    fn creates_and_checks_out_release_branch_with_dot_and_dash_n() {
        let dir = seed();
        let repo = Repository::open(dir.path()).unwrap();

        create_release_branch(&repo, "release/1.4.0-2", "develop").unwrap();
        assert_eq!(repo.head().unwrap().shorthand(), Some("release/1.4.0-2"));
        // develop tip unchanged.
        let dev = repo.find_branch("develop", BranchType::Local).unwrap();
        assert_eq!(
            dev.get().peel_to_commit().unwrap().id(),
            repo.head().unwrap().peel_to_commit().unwrap().id()
        );
    }

    #[test]
    fn rejects_existing_branch_and_missing_base() {
        let dir = seed();
        let repo = Repository::open(dir.path()).unwrap();
        create_release_branch(&repo, "release/1.4.0", "develop").unwrap();
        run(dir.path(), &["checkout", "develop"]);

        assert!(matches!(
            create_release_branch(&repo, "release/1.4.0", "develop"),
            Err(ReleaseError::AlreadyExists(_))
        ));
        assert!(matches!(
            create_release_branch(&repo, "release/9.9.9", "no-such"),
            Err(ReleaseError::BaseNotFound(_))
        ));
    }
}
