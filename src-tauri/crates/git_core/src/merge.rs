use git2::build::CheckoutBuilder;
use git2::{Oid, Repository};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MergeError {
    #[error("target '{0}' not found")]
    TargetNotFound(String),
    #[error(transparent)]
    Git(#[from] git2::Error),
}

pub struct ConflictMerge {
    /// Files left carrying conflict markers in the working directory.
    pub conflicting_files: Vec<String>,
    /// The `target` commit merged in -- callers need this to record the
    /// resolution as a real two-parent merge commit, not a single-parent one.
    pub target_commit: Oid,
}

/// Merges `target` into the currently checked-out HEAD **in the repo's own
/// working directory** (Work Safe -- no worktree isolation), leaving conflict
/// markers and `MERGE_HEAD` in place for the Owner to resolve with their own
/// editor/mergetool. Caller guarantees a clean tree first (`guard_working_tree`).
pub fn merge_target_into_head(repo: &Repository, target: &str) -> Result<ConflictMerge, MergeError> {
    let target_commit = repo
        .revparse_single(target)
        .map_err(|_| MergeError::TargetNotFound(target.to_string()))?
        .peel_to_commit()?;
    let annotated = repo.find_annotated_commit(target_commit.id())?;

    let mut checkout = CheckoutBuilder::new();
    // `force()` resets the low strategy bits, so it must come before
    // `allow_conflicts()` -- calling it last silently clears that flag again.
    checkout.force().allow_conflicts(true).conflict_style_merge(true);
    repo.merge(&[&annotated], None, Some(&mut checkout))?;

    let index = repo.index()?;
    let conflicting_files = index
        .conflicts()?
        .filter_map(|c| c.ok())
        .filter_map(|c| c.our.or(c.their).or(c.ancestor))
        .filter_map(|e| String::from_utf8(e.path).ok())
        .collect();

    Ok(ConflictMerge {
        conflicting_files,
        target_commit: target_commit.id(),
    })
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

    fn commit(dir: &std::path::Path, file: &str, content: &str, msg: &str) {
        std::fs::write(dir.join(file), content).unwrap();
        run(dir, &["add", file]);
        run(dir, &["commit", "-m", msg]);
    }

    #[test]
    fn surfaces_markers_in_working_dir() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "develop"]);
        run(dir.path(), &["config", "user.email", "t@e.com"]);
        run(dir.path(), &["config", "user.name", "T"]);
        commit(dir.path(), "shared.txt", "base\n", "init");
        run(dir.path(), &["checkout", "-b", "feature/a"]);
        commit(dir.path(), "shared.txt", "feature change\n", "feature: change");
        run(dir.path(), &["checkout", "develop"]);
        commit(dir.path(), "shared.txt", "develop change\n", "develop: change");
        run(dir.path(), &["checkout", "feature/a"]);

        let repo = Repository::open(dir.path()).unwrap();
        let res = merge_target_into_head(&repo, "develop").unwrap();
        assert_eq!(res.conflicting_files, vec!["shared.txt".to_string()]);

        let content = std::fs::read_to_string(dir.path().join("shared.txt")).unwrap();
        assert!(content.contains("<<<<<<<"));
    }
}
