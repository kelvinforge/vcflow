use std::cell::RefCell;

use git2::{FetchOptions, RemoteCallbacks, Repository};
use thiserror::Error;

use crate::ssh::make_credentials_callback;

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("no remote named 'origin'")]
    NoOrigin,
    #[error("could not fetch from origin: {0}")]
    Fetch(String),
    #[error(transparent)]
    Git(#[from] git2::Error),
}

/// One remote ref that moved as a result of the fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatedRef {
    pub name: String,
    pub old: Option<String>,
    pub new: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchReport {
    pub updated: Vec<UpdatedRef>,
}

impl FetchReport {
    pub fn anything_changed(&self) -> bool {
        !self.updated.is_empty()
    }
}

/// `git fetch origin` -- updates remote-tracking refs only. Never touches the
/// working tree, index, or any local branch, so it is safe to call on a
/// dirty repo and from monitoring/Refresh without a Working Tree Guard.
///
/// Uses the same SSH/HTTPS credential resolution as push/connect. No prune
/// (matches `git fetch` CLI default).
pub fn fetch_origin(repo: &Repository) -> Result<FetchReport, FetchError> {
    let mut remote = repo.find_remote("origin").map_err(|_| FetchError::NoOrigin)?;

    let updated: RefCell<Vec<UpdatedRef>> = RefCell::new(Vec::new());
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(make_credentials_callback());
    callbacks.update_tips(|name, old, new| {
        updated.borrow_mut().push(UpdatedRef {
            name: name.to_string(),
            old: (!old.is_zero()).then(|| old.to_string()),
            new: new.to_string(),
        });
        true
    });

    let mut opts = FetchOptions::new();
    opts.remote_callbacks(callbacks);

    let refspecs: Vec<String> = remote
        .fetch_refspecs()?
        .iter()
        .flatten()
        .map(str::to_string)
        .collect();

    let result = remote
        .fetch(&refspecs, Some(&mut opts), None)
        .map_err(|e| FetchError::Fetch(e.message().to_string()));
    drop(opts);
    result?;

    Ok(FetchReport {
        updated: updated.into_inner(),
    })
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

    fn run_out(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("failed to run git");
        assert!(out.status.success(), "git {:?} failed", args);
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// Bare `origin` with one commit on `develop`, plus a local clone of it.
    fn origin_and_clone() -> (TempDir, TempDir) {
        let origin = tempdir().unwrap();
        run(origin.path(), &["init", "--bare", "-b", "develop"]);

        let seed = tempdir().unwrap();
        run(seed.path(), &["init", "-b", "develop"]);
        run(seed.path(), &["config", "user.email", "t@e.com"]);
        run(seed.path(), &["config", "user.name", "T"]);
        std::fs::write(seed.path().join("a.txt"), "one\n").unwrap();
        run(seed.path(), &["add", "a.txt"]);
        run(seed.path(), &["commit", "-m", "init"]);
        run(seed.path(), &["remote", "add", "origin", origin.path().to_str().unwrap()]);
        run(seed.path(), &["push", "origin", "develop"]);

        let clone = tempdir().unwrap();
        run(
            clone.path(),
            &["clone", origin.path().to_str().unwrap(), clone.path().to_str().unwrap()],
        );
        run(clone.path(), &["config", "user.email", "t@e.com"]);
        run(clone.path(), &["config", "user.name", "T"]);

        (origin, clone)
    }

    #[test]
    fn fetch_with_no_new_commits_reports_nothing_changed() {
        let (_origin, clone) = origin_and_clone();
        let repo = Repository::open(clone.path()).unwrap();
        let report = fetch_origin(&repo).unwrap();
        assert!(!report.anything_changed(), "got: {report:?}");
    }

    #[test]
    fn fetch_picks_up_a_new_remote_commit_without_touching_working_tree() {
        let (origin, clone) = origin_and_clone();

        // Dirty the clone's working tree -- fetch must leave it alone.
        std::fs::write(clone.path().join("a.txt"), "one\nlocal edit\n").unwrap();
        std::fs::write(clone.path().join("untracked.txt"), "x\n").unwrap();

        // Push a new commit to origin from a second working clone.
        let other = tempdir().unwrap();
        run(
            other.path(),
            &["clone", origin.path().to_str().unwrap(), other.path().to_str().unwrap()],
        );
        run(other.path(), &["config", "user.email", "t@e.com"]);
        run(other.path(), &["config", "user.name", "T"]);
        std::fs::write(other.path().join("b.txt"), "b\n").unwrap();
        run(other.path(), &["add", "b.txt"]);
        run(other.path(), &["commit", "-m", "second"]);
        run(other.path(), &["push", "origin", "develop"]);

        let repo = Repository::open(clone.path()).unwrap();
        let before_head = run_out(clone.path(), &["rev-parse", "HEAD"]);

        let report = fetch_origin(&repo).unwrap();

        assert!(report.anything_changed());
        assert!(report
            .updated
            .iter()
            .any(|u| u.name.contains("origin/develop")));

        // Local HEAD unmoved, working-tree edits + untracked file intact.
        assert_eq!(run_out(clone.path(), &["rev-parse", "HEAD"]), before_head);
        assert_eq!(
            std::fs::read_to_string(clone.path().join("a.txt")).unwrap(),
            "one\nlocal edit\n"
        );
        assert!(clone.path().join("untracked.txt").exists());

        // But origin/develop now points at the new commit.
        let remote_tip = run_out(clone.path(), &["rev-parse", "origin/develop"]);
        assert_ne!(remote_tip, before_head);
    }

    #[test]
    fn missing_origin_is_reported() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "develop"]);
        let repo = Repository::open(dir.path()).unwrap();
        assert!(matches!(fetch_origin(&repo), Err(FetchError::NoOrigin)));
    }
}
