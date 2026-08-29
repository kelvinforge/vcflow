use git2::{Repository, Signature, StashFlags};
use thiserror::Error;

use crate::status::{read_working_tree_status, StatusError};

#[derive(Debug, Error)]
pub enum SaveWorkError {
    #[error("no saved work matches stash {0}")]
    NotFound(String),
    #[error("working tree is dirty -- commit or save the current changes before restoring")]
    WouldClobber,
    /// Re-applying the saved changes collided with the current branch content.
    /// The saved entry is left intact -- Work Safe never resets, never
    /// discards. The listed paths carry conflict markers for the user to
    /// resolve in the working directory.
    #[error("restoring saved work hit a merge conflict in {} file(s)", .files.len())]
    Conflict { files: Vec<String> },
    #[error(transparent)]
    Git(#[from] git2::Error),
}

/// A single stash entry Work Safe created so a workflow step could run on a
/// clean tree. `stash_oid` is the stash commit id -- stable even if the user
/// stashes other things by hand, so the exact entry can be found and dropped
/// later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedWork {
    pub stash_oid: String,
    pub message: String,
}

/// Stashes the whole working tree -- tracked changes AND untracked files --
/// leaving a clean tree behind. Returns `Ok(None)` when the tree is already
/// clean (nothing to save, not an error).
///
/// Internal plumbing: callers surface this to the user as "Saved Work", never
/// as "stash".
pub fn save_work(repo: &mut Repository, message: &str) -> Result<Option<SavedWork>, SaveWorkError> {
    if !dirty(repo)? {
        return Ok(None);
    }
    let sig = repo
        .signature()
        .or_else(|_| Signature::now("VC Flow Work Safe", "vc-flow@localhost"))?;
    let oid = repo.stash_save2(&sig, Some(message), Some(StashFlags::INCLUDE_UNTRACKED))?;
    Ok(Some(SavedWork {
        stash_oid: oid.to_string(),
        message: message.to_string(),
    }))
}

/// Re-applies a saved entry and removes it from the stash list. Refuses when
/// the tree is currently dirty -- applying onto uncommitted changes risks a
/// merge the user never asked for (Work Safe: never auto-resolve).
pub fn restore_work(repo: &mut Repository, stash_oid: &str) -> Result<(), SaveWorkError> {
    if dirty(repo)? {
        return Err(SaveWorkError::WouldClobber);
    }
    let index = stash_index_of(repo, stash_oid)?;
    // Apply first, then drop only on a clean apply. libgit2 can report a
    // content conflict either as an error or as an Ok apply that leaves
    // conflict entries in the index -- handle both, and in neither case drop
    // the entry: it stays resumable/discardable and the workdir holds the
    // markers (Work Safe -- never reset, never auto-resolve).
    match repo.stash_apply(index, None) {
        Ok(()) => {}
        Err(e) if e.code() == git2::ErrorCode::Conflict || e.class() == git2::ErrorClass::Merge => {
            return Err(SaveWorkError::Conflict { files: conflicting_paths(repo) });
        }
        Err(e) => return Err(e.into()),
    }
    if repo.index().map(|i| i.has_conflicts()).unwrap_or(false) {
        return Err(SaveWorkError::Conflict { files: conflicting_paths(repo) });
    }
    let index = stash_index_of(repo, stash_oid)?;
    repo.stash_drop(index)?;
    Ok(())
}

fn conflicting_paths(repo: &Repository) -> Vec<String> {
    let Ok(index) = repo.index() else { return Vec::new() };
    let Ok(conflicts) = index.conflicts() else { return Vec::new() };
    conflicts
        .filter_map(Result::ok)
        .filter_map(|c| c.our.or(c.their).or(c.ancestor))
        .filter_map(|e| String::from_utf8(e.path).ok())
        .collect()
}

/// Drops a saved entry without applying it. Explicit user action only --
/// Work Safe never discards on its own.
pub fn discard_work(repo: &mut Repository, stash_oid: &str) -> Result<(), SaveWorkError> {
    let index = stash_index_of(repo, stash_oid)?;
    repo.stash_drop(index)?;
    Ok(())
}

fn dirty(repo: &Repository) -> Result<bool, SaveWorkError> {
    read_working_tree_status(repo)
        .map(|s| s.is_dirty())
        .map_err(|StatusError::Git(g)| g.into())
}

fn stash_index_of(repo: &mut Repository, stash_oid: &str) -> Result<usize, SaveWorkError> {
    let mut found = None;
    repo.stash_foreach(|index, _msg, oid| {
        if oid.to_string() == stash_oid {
            found = Some(index);
            false
        } else {
            true
        }
    })?;
    found.ok_or_else(|| SaveWorkError::NotFound(stash_oid.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use tempfile::{tempdir, TempDir};

    fn run(dir: &Path, args: &[&str]) {
        assert!(
            Command::new("git").args(args).current_dir(dir).status().unwrap().success(),
            "git {args:?}"
        );
    }

    fn init_repo() -> TempDir {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "develop"]);
        run(dir.path(), &["config", "user.email", "t@e.com"]);
        run(dir.path(), &["config", "user.name", "T"]);
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        run(dir.path(), &["add", "a.txt"]);
        run(dir.path(), &["commit", "-m", "init"]);
        dir
    }

    #[test]
    fn clean_tree_saves_nothing() {
        let dir = init_repo();
        let mut repo = Repository::open(dir.path()).unwrap();
        assert!(save_work(&mut repo, "x").unwrap().is_none());
    }

    #[test]
    fn saves_and_restores_tracked_and_untracked() {
        let dir = init_repo();
        std::fs::write(dir.path().join("a.txt"), "changed\n").unwrap();
        std::fs::write(dir.path().join("new.txt"), "new\n").unwrap();
        let mut repo = Repository::open(dir.path()).unwrap();

        let saved = save_work(&mut repo, "wip").unwrap().expect("something saved");
        assert!(!read_working_tree_status(&repo).unwrap().is_dirty());
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "a\n");
        assert!(!dir.path().join("new.txt").exists());

        restore_work(&mut repo, &saved.stash_oid).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "changed\n");
        assert_eq!(std::fs::read_to_string(dir.path().join("new.txt")).unwrap(), "new\n");
    }

    #[test]
    fn restore_refuses_on_dirty_tree() {
        let dir = init_repo();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        let mut repo = Repository::open(dir.path()).unwrap();
        let saved = save_work(&mut repo, "wip").unwrap().unwrap();
        std::fs::write(dir.path().join("a.txt"), "conflict\n").unwrap();
        assert!(matches!(
            restore_work(&mut repo, &saved.stash_oid),
            Err(SaveWorkError::WouldClobber)
        ));
    }

    #[test]
    fn restore_conflict_preserves_the_saved_entry() {
        let dir = init_repo();
        std::fs::write(dir.path().join("a.txt"), "from-save\n").unwrap();
        let mut repo = Repository::open(dir.path()).unwrap();
        let saved = save_work(&mut repo, "wip").unwrap().unwrap();

        // Move the branch content so re-applying the stash collides.
        std::fs::write(dir.path().join("a.txt"), "from-branch\n").unwrap();
        run(dir.path(), &["commit", "-am", "diverge"]);

        match restore_work(&mut repo, &saved.stash_oid) {
            Err(SaveWorkError::Conflict { files }) => assert!(files.contains(&"a.txt".to_string())),
            other => panic!("expected Conflict, got {other:?}"),
        }
        // Entry is still there -- not dropped, not reset.
        assert!(stash_index_of(&mut repo, &saved.stash_oid).is_ok());
    }

    #[test]
    fn discard_drops_without_applying() {
        let dir = init_repo();
        std::fs::write(dir.path().join("a.txt"), "gone\n").unwrap();
        let mut repo = Repository::open(dir.path()).unwrap();
        let saved = save_work(&mut repo, "wip").unwrap().unwrap();
        discard_work(&mut repo, &saved.stash_oid).unwrap();
        assert!(matches!(
            restore_work(&mut repo, &saved.stash_oid),
            Err(SaveWorkError::NotFound(_))
        ));
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "a\n");
    }

    #[test]
    fn unknown_oid_is_not_found() {
        let dir = init_repo();
        let mut repo = Repository::open(dir.path()).unwrap();
        assert!(matches!(
            discard_work(&mut repo, "0000000000000000000000000000000000000000"),
            Err(SaveWorkError::NotFound(_))
        ));
    }
}
