use std::fs;
use std::path::Path;

use git2::Repository;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RepoError {
    #[error("not a git repository: {0}")]
    NotARepo(#[source] git2::Error),
    #[error("HEAD is not a branch (detached HEAD)")]
    DetachedHead,
    #[error("no remote named 'origin'")]
    NoOrigin,
    #[error(transparent)]
    Git(#[from] git2::Error),
    #[error("failed to read VERSION file: {0}")]
    VersionRead(#[source] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub current_branch: String,
    pub remote_url: Option<String>,
    pub version: Option<String>,
}

/// True if the working tree has any modified/staged/untracked change --
/// used to decide whether Hotfix creation must isolate into its own
/// worktree instead of checking out in place.
pub fn is_dirty(repo: &Repository) -> Result<bool, RepoError> {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let statuses = repo.statuses(Some(&mut opts))?;
    Ok(!statuses.is_empty())
}

/// Current branch name from HEAD. `Ok(None)` means detached HEAD.
///
/// On an unborn branch -- a freshly `git init`'d repo with no commits --
/// `repo.head()` fails with `UnbornBranch` because `refs/heads/<name>`
/// doesn't exist yet. That is not an error condition for us: HEAD is still a
/// symbolic ref naming the branch the first commit will create, so return
/// that name.
pub fn head_branch(repo: &Repository) -> Result<Option<String>, git2::Error> {
    match repo.head() {
        Ok(head) => Ok(head.shorthand().filter(|_| head.is_branch()).map(str::to_string)),
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => Ok(Some(unborn_branch_name(repo))),
        Err(e) => Err(e),
    }
}

/// The production branch to base `develop` on: the checked-out branch when it
/// is `main`/`master` (works for an unborn HEAD too, since `head_branch`
/// returns the intended name), otherwise whichever of `main`/`master` exists
/// locally. `None` when neither is present and HEAD is something else.
pub fn production_branch(repo: &Repository) -> Option<String> {
    if let Ok(Some(b)) = head_branch(repo) {
        if b == "main" || b == "master" {
            return Some(b);
        }
    }
    for cand in ["main", "master"] {
        if repo.find_branch(cand, git2::BranchType::Local).is_ok() {
            return Some(cand.to_string());
        }
    }
    None
}

/// Reads `path` (repo-relative, e.g. `"VERSION"`) as it exists at `ref_name`
/// (e.g. `"refs/remotes/origin/master"`) without touching the working tree.
/// `Err` when the ref or the file is absent at that ref.
pub fn read_file_at_ref(
    repo: &Repository,
    ref_name: &str,
    path: &str,
) -> Result<String, RepoError> {
    let tree = repo.revparse_single(ref_name)?.peel_to_tree()?;
    let entry = tree.get_path(Path::new(path))?;
    let blob = repo.find_blob(entry.id())?;
    Ok(String::from_utf8_lossy(blob.content()).into_owned())
}

/// True when `name` resolves to a local branch head or to
/// `refs/remotes/origin/<name>`. Used to pick a free `release/x.y.z[-N]` name.
pub fn ref_exists(repo: &Repository, name: &str) -> bool {
    repo.find_branch(name, git2::BranchType::Local).is_ok()
        || repo
            .revparse_single(&format!("refs/remotes/origin/{name}"))
            .is_ok()
}

fn unborn_branch_name(repo: &Repository) -> String {
    repo.find_reference("HEAD")
        .ok()
        .and_then(|h| h.symbolic_target().and_then(|t| t.strip_prefix("refs/heads/")).map(str::to_string))
        .unwrap_or_else(|| "main".to_string())
}

/// Reads current branch, `origin` remote URL, and repo-root `VERSION` file
/// for the git repository at `path`.
pub fn read_repo_info(path: impl AsRef<Path>) -> Result<RepoInfo, RepoError> {
    let repo = Repository::discover(path).map_err(RepoError::NotARepo)?;

    let current_branch = head_branch(&repo)?.ok_or(RepoError::DetachedHead)?;

    let remote_url = match repo.find_remote("origin") {
        Ok(remote) => remote.url().map(str::to_string),
        Err(_) => None,
    };

    let workdir = repo.workdir().ok_or(RepoError::NoOrigin)?;
    let version_path = workdir.join("VERSION");
    let version = if version_path.exists() {
        Some(
            fs::read_to_string(&version_path)
                .map_err(RepoError::VersionRead)?
                .trim()
                .to_string(),
        )
    } else {
        None
    };

    Ok(RepoInfo {
        current_branch,
        remote_url,
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("failed to run git");
        assert!(status.success(), "git {:?} failed", args);
    }

    #[test]
    fn reads_branch_remote_and_version() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "develop"]);
        run(dir.path(), &["config", "user.email", "test@example.com"]);
        run(dir.path(), &["config", "user.name", "Test"]);
        run(
            dir.path(),
            &["remote", "add", "origin", "git@example.com:group/repo.git"],
        );
        fs::write(dir.path().join("VERSION"), "1.2.3\n").unwrap();
        run(dir.path(), &["add", "VERSION"]);
        run(dir.path(), &["commit", "-m", "init"]);

        let info = read_repo_info(dir.path()).unwrap();
        assert_eq!(info.current_branch, "develop");
        assert_eq!(
            info.remote_url.as_deref(),
            Some("git@example.com:group/repo.git")
        );
        assert_eq!(info.version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn dirty_worktree_detected() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "develop"]);
        run(dir.path(), &["config", "user.email", "test@example.com"]);
        run(dir.path(), &["config", "user.name", "Test"]);
        fs::write(dir.path().join("readme.md"), "x").unwrap();
        run(dir.path(), &["add", "readme.md"]);
        run(dir.path(), &["commit", "-m", "init"]);

        let repo = Repository::open(dir.path()).unwrap();
        assert!(!is_dirty(&repo).unwrap());

        fs::write(dir.path().join("readme.md"), "changed").unwrap();
        assert!(is_dirty(&repo).unwrap());
    }

    #[test]
    fn production_branch_prefers_head_then_main_then_master() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "main"]);
        run(dir.path(), &["config", "user.email", "t@e.com"]);
        run(dir.path(), &["config", "user.name", "T"]);
        std::fs::write(dir.path().join("a"), "a").unwrap();
        run(dir.path(), &["add", "a"]);
        run(dir.path(), &["commit", "-m", "init"]);

        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(production_branch(&repo).as_deref(), Some("main"));

        run(dir.path(), &["checkout", "-b", "feature/x"]);
        assert_eq!(production_branch(&repo).as_deref(), Some("main"));

        run(dir.path(), &["branch", "-m", "main", "master"]);
        assert_eq!(production_branch(&repo).as_deref(), Some("master"));
    }

    #[test]
    fn production_branch_none_when_no_main_or_master() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "trunk"]);
        run(dir.path(), &["config", "user.email", "t@e.com"]);
        run(dir.path(), &["config", "user.name", "T"]);
        std::fs::write(dir.path().join("a"), "a").unwrap();
        run(dir.path(), &["add", "a"]);
        run(dir.path(), &["commit", "-m", "init"]);

        let repo = Repository::open(dir.path()).unwrap();
        assert_eq!(production_branch(&repo), None);
    }

    #[test]
    fn missing_version_file_is_none() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "main"]);
        run(dir.path(), &["config", "user.email", "test@example.com"]);
        run(dir.path(), &["config", "user.name", "Test"]);
        fs::write(dir.path().join("readme.md"), "x").unwrap();
        run(dir.path(), &["add", "readme.md"]);
        run(dir.path(), &["commit", "-m", "init"]);

        let info = read_repo_info(dir.path()).unwrap();
        assert_eq!(info.version, None);
        assert_eq!(info.remote_url, None);
    }
}
