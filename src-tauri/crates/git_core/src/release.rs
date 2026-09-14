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

/// Resolve the commit that shipped release `version`, for the "has this release
/// reached develop?" question. Tries, in order:
///   1. the `v<version>` tag -- vcflow publishes it during `sync_develop_after_release`
///   2. the `chore: release <version>` prep commit reachable from `production_ref`
///      -- vcflow's own commit subject (see `create_release_candidate`)
///
/// `None` when neither can be found: the release has not actually shipped, so
/// "reached develop" is not a meaningful question yet.
fn resolve_release_commit(
    repo: &Repository,
    version: &str,
    production_ref: &str,
) -> Option<git2::Oid> {
    if let Ok(obj) = repo.revparse_single(&format!("refs/tags/v{version}")) {
        if let Ok(commit) = obj.peel_to_commit() {
            return Some(commit.id());
        }
    }

    let subject = format!("chore: release {version}");
    let tip = repo.revparse_single(production_ref).ok()?.peel_to_commit().ok()?.id();
    let mut walk = repo.revwalk().ok()?;
    walk.push(tip).ok()?;
    for oid in walk.flatten() {
        if let Ok(commit) = repo.find_commit(oid) {
            if commit.summary() == Some(subject.as_str()) {
                return Some(oid);
            }
        }
    }
    None
}

/// Whether the commit that shipped release `version` is an ancestor of (or
/// identical to) `develop_ref`. This is the git-truth answer to "is the
/// develop-sync for this release done?" -- a merge request is only the
/// mechanism that gets the commit there, never the proof that it arrived.
///
/// `Ok(None)` means the release commit could not be resolved at all (not
/// shipped yet); callers treat that as "not reached".
pub fn release_reached_branch(
    repo: &Repository,
    version: &str,
    production_ref: &str,
    develop_ref: &str,
) -> Result<Option<bool>, ReleaseError> {
    let Some(release_commit) = resolve_release_commit(repo, version, production_ref) else {
        return Ok(None);
    };
    let develop = repo
        .revparse_single(develop_ref)
        .map_err(|_| ReleaseError::BaseNotFound(develop_ref.to_string()))?
        .peel_to_commit()?
        .id();
    if develop == release_commit {
        return Ok(Some(true));
    }
    Ok(Some(repo.graph_descendant_of(develop, release_commit)?))
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

    fn commit(dir: &std::path::Path, file: &str, msg: &str) {
        std::fs::write(dir.join(file), msg).unwrap();
        run(dir, &["add", file]);
        run(dir, &["commit", "-m", msg]);
    }

    #[test]
    fn reached_branch_via_tag_when_release_commit_is_ancestor_of_develop() {
        let dir = seed();
        commit(dir.path(), "r.txt", "chore: release 1.4.0");
        run(dir.path(), &["tag", "v1.4.0"]);
        run(dir.path(), &["branch", "master"]);
        // develop moves on past the release commit.
        commit(dir.path(), "next.txt", "feat: more");

        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(
            release_reached_branch(&repo, "1.4.0", "master", "develop").unwrap(),
            Some(true)
        );
    }

    #[test]
    fn reached_branch_via_prep_commit_subject_when_tag_is_missing() {
        let dir = seed();
        commit(dir.path(), "r.txt", "chore: release 1.4.0");
        run(dir.path(), &["branch", "master"]);
        commit(dir.path(), "next.txt", "feat: more");

        let repo = Repository::open(dir.path()).unwrap();
        // no v1.4.0 tag -> falls back to the `chore: release 1.4.0` commit.
        assert_eq!(
            release_reached_branch(&repo, "1.4.0", "master", "develop").unwrap(),
            Some(true)
        );
    }

    #[test]
    fn not_reached_when_release_commit_absent_from_develop() {
        let dir = seed();
        run(dir.path(), &["checkout", "-b", "master"]);
        commit(dir.path(), "r.txt", "chore: release 1.4.0");
        run(dir.path(), &["tag", "v1.4.0"]);
        run(dir.path(), &["checkout", "develop"]);

        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(
            release_reached_branch(&repo, "1.4.0", "master", "develop").unwrap(),
            Some(false)
        );
    }

    #[test]
    fn none_when_release_commit_cannot_be_resolved() {
        let dir = seed();
        run(dir.path(), &["branch", "master"]);
        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(
            release_reached_branch(&repo, "9.9.9", "master", "develop").unwrap(),
            None
        );
    }
}
