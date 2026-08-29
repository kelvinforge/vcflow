use git2::{BranchType, Repository};
use thiserror::Error;

/// Feature/Bug/Chore share one branch-create flow; only the prefix
/// differs. Hotfix gets its own dedicated flow (different base branch,
/// extra version-bump commit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchKind {
    Feature,
    Bug,
    Chore,
    /// Branches from `master`, not `develop` -- callers must pass `master`
    /// as `base` themselves, this fn doesn't special-case it.
    Hotfix,
}

impl BranchKind {
    fn prefix(self) -> &'static str {
        match self {
            // Official prefix is `bug/*`, never `bugfix/*` (branch strategy).
            BranchKind::Feature => "feature",
            BranchKind::Bug => "bug",
            BranchKind::Chore => "chore",
            BranchKind::Hotfix => "hotfix",
        }
    }
}

#[derive(Debug, Error)]
pub enum BranchError {
    #[error("slug must be lowercase letters, digits, and hyphens only, got: {0}")]
    InvalidSlug(String),
    #[error("base branch '{0}' not found locally")]
    BaseNotFound(String),
    #[error("branch '{0}' already exists")]
    AlreadyExists(String),
    #[error("branch '{0}' not found locally")]
    NotFound(String),
    #[error(transparent)]
    Git(#[from] git2::Error),
}

/// Switches HEAD to an existing local branch and updates the working tree to
/// match it. Plain checkout only -- never creates a branch, never resets, and
/// (because it uses a safe checkout) refuses rather than clobbering
/// uncommitted changes. Callers gate the dirty tree via `guard_working_tree`
/// first; this is the last line of defence.
pub fn checkout_branch(repo: &Repository, name: &str) -> Result<(), BranchError> {
    repo.find_branch(name, BranchType::Local)
        .map_err(|_| BranchError::NotFound(name.to_string()))?;

    let obj = repo.revparse_single(&format!("refs/heads/{name}"))?;
    // Default (safe) checkout: libgit2 aborts if it would overwrite a
    // modified file, so this cannot silently discard the user's work.
    repo.checkout_tree(&obj, None)?;
    repo.set_head(&format!("refs/heads/{name}"))?;
    Ok(())
}

/// Creates `<kind>/<slug>` off `base` (e.g. `develop`) and checks it out.
pub fn create_work_branch(
    repo: &Repository,
    kind: BranchKind,
    slug: &str,
    base: &str,
) -> Result<String, BranchError> {
    let valid_slug = !slug.is_empty()
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid_slug {
        return Err(BranchError::InvalidSlug(slug.to_string()));
    }

    let branch_name = format!("{}/{slug}", kind.prefix());
    if repo.find_branch(&branch_name, BranchType::Local).is_ok() {
        return Err(BranchError::AlreadyExists(branch_name));
    }

    let base_branch = repo
        .find_branch(base, BranchType::Local)
        .map_err(|_| BranchError::BaseNotFound(base.to_string()))?;
    let base_commit = base_branch.get().peel_to_commit()?;

    repo.branch(&branch_name, &base_commit, false)?;

    let obj = repo.revparse_single(&format!("refs/heads/{branch_name}"))?;
    repo.checkout_tree(&obj, None)?;
    repo.set_head(&format!("refs/heads/{branch_name}"))?;

    Ok(branch_name)
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

    fn init_repo_with_develop() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "develop"]);
        run(dir.path(), &["config", "user.email", "test@example.com"]);
        run(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("readme.md"), "x").unwrap();
        run(dir.path(), &["add", "readme.md"]);
        run(dir.path(), &["commit", "-m", "init"]);
        dir
    }

    #[test]
    fn creates_and_checks_out_feature_branch() {
        let dir = init_repo_with_develop();
        let repo = Repository::open(dir.path()).unwrap();

        let name = create_work_branch(&repo, BranchKind::Feature, "my-thing", "develop").unwrap();
        assert_eq!(name, "feature/my-thing");

        let head = repo.head().unwrap();
        assert_eq!(head.shorthand(), Some("feature/my-thing"));
    }

    #[test]
    fn bug_uses_bug_prefix_not_bugfix() {
        let dir = init_repo_with_develop();
        let repo = Repository::open(dir.path()).unwrap();

        let name = create_work_branch(&repo, BranchKind::Bug, "off-by-one", "develop").unwrap();
        assert_eq!(name, "bug/off-by-one");
    }

    #[test]
    fn rejects_uppercase_slug() {
        let dir = init_repo_with_develop();
        let repo = Repository::open(dir.path()).unwrap();

        let err = create_work_branch(&repo, BranchKind::Feature, "MyThing", "develop").unwrap_err();
        assert!(matches!(err, BranchError::InvalidSlug(_)));
    }

    #[test]
    fn rejects_duplicate_branch() {
        let dir = init_repo_with_develop();
        let repo = Repository::open(dir.path()).unwrap();

        create_work_branch(&repo, BranchKind::Feature, "dup", "develop").unwrap();
        // Reset back to develop so we're not branching from the new branch itself.
        run(dir.path(), &["checkout", "develop"]);
        let err = create_work_branch(&repo, BranchKind::Feature, "dup", "develop").unwrap_err();
        assert!(matches!(err, BranchError::AlreadyExists(_)));
    }

    #[test]
    fn checkout_branch_switches_head() {
        let dir = init_repo_with_develop();
        let repo = Repository::open(dir.path()).unwrap();
        create_work_branch(&repo, BranchKind::Feature, "a", "develop").unwrap();
        run(dir.path(), &["checkout", "develop"]);

        checkout_branch(&repo, "feature/a").unwrap();
        assert_eq!(repo.head().unwrap().shorthand(), Some("feature/a"));
    }

    #[test]
    fn checkout_branch_rejects_unknown() {
        let dir = init_repo_with_develop();
        let repo = Repository::open(dir.path()).unwrap();
        let err = checkout_branch(&repo, "feature/nope").unwrap_err();
        assert!(matches!(err, BranchError::NotFound(_)));
    }

    #[test]
    fn checkout_branch_does_not_clobber_dirty_file() {
        let dir = init_repo_with_develop();
        let repo = Repository::open(dir.path()).unwrap();
        // readme.md differs between develop and feature/a
        create_work_branch(&repo, BranchKind::Feature, "a", "develop").unwrap();
        std::fs::write(dir.path().join("readme.md"), "feature-content").unwrap();
        run(dir.path(), &["commit", "-am", "feature edit"]);
        run(dir.path(), &["checkout", "develop"]);

        std::fs::write(dir.path().join("readme.md"), "uncommitted local edit").unwrap();
        assert!(checkout_branch(&repo, "feature/a").is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("readme.md")).unwrap(),
            "uncommitted local edit"
        );
    }

    #[test]
    fn rejects_missing_base() {
        let dir = init_repo_with_develop();
        let repo = Repository::open(dir.path()).unwrap();

        let err = create_work_branch(&repo, BranchKind::Feature, "x", "no-such-base").unwrap_err();
        assert!(matches!(err, BranchError::BaseNotFound(_)));
    }
}
