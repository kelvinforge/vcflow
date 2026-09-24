//! Repository Concurrency v1 (P0 #1 of the frozen concurrency/lifecycle
//! audit). Serializes VC Flow's own in-process access to a repository's
//! `.git` state so a mutation (checkout/stash/commit/push/...) and a
//! concurrent read never observe or produce a torn state, and two mutations
//! on the same repository never race each other.
//!
//! Scope, exactly as frozen in the design decision -- deliberately NOT solved
//! here: an external git client (VS Code, a terminal, the app's own
//! `open_in_external_tool` mergetool subprocess) and a second VC Flow process
//! on the same machine. Both are invisible to this in-process lock; see
//! `commands::open_in_external_tool`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use git2::Repository;
use tokio::sync::Mutex as AsyncMutex;

/// One `Arc<tokio::sync::Mutex<()>>` per canonical repository root, behind a
/// `std::sync::Mutex` that protects only the `HashMap` lookup/insert -- that
/// outer lock is never held across an `.await`.
///
/// Register via `tauri::Builder::manage(RepoLockRegistry::default())`.
///
/// v1 is deliberately append-only: one entry per distinct repository root
/// ever opened this session. Not cleaned up. This is an accepted tradeoff,
/// not an oversight -- the map's size is bounded by "how many different
/// repositories this session touched", which for a desktop tool is a handful
/// at most.
#[derive(Default)]
pub struct RepoLockRegistry {
    locks: Mutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>,
}

impl RepoLockRegistry {
    fn lock_for(&self, key: &Path) -> Arc<AsyncMutex<()>> {
        let mut map = self.locks.lock().expect("RepoLockRegistry map poisoned");
        map.entry(key.to_path_buf())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }
}

/// Resolves the lock key: the repository's actual canonical root, never the
/// caller's raw `repo_path`. The frontend's directory picker lets the user
/// choose any subdirectory of a repository (`open({ directory: true })`), and
/// `Repository::discover` walks upward to find the real `.git` -- two
/// different subdirectories of the same repository must resolve to the same
/// key, and two different repositories must not collide.
///
/// Errors here are real errors (not a repository, or the root can't be
/// canonicalized) -- never lock contention. Callers that must tolerate "not a
/// repository yet" as a normal state (`repository_preflight`, `build_status`,
/// `build_work_list`) check that themselves before calling into this lock,
/// exactly as they already do without it; this function does not special-case
/// that for them.
fn canonical_repo_key(repo_path: &str) -> Result<PathBuf, String> {
    let repo = Repository::discover(repo_path).map_err(|e| e.to_string())?;
    let root = repo.workdir().unwrap_or_else(|| repo.path());
    std::fs::canonicalize(root).map_err(|e| e.to_string())
}

/// Blocking acquire -- for a user-triggered repository mutation. Resolves the
/// canonical key outside any lock, waits for that repository's lock, runs `f`
/// while holding the guard, then releases it (RAII: a panic or an `Err`
/// return from `f` both release it on unwind/return).
///
/// `f` is synchronous by design: nothing that needs `.await` (a provider REST
/// call) may run while the lock is held. `git_core`'s own push/fetch are
/// blocking calls, not `async fn`, so they run inside `f` like any other git2
/// operation. A command that must interleave a network `.await` between two
/// repository touches calls `with_repo_lock` twice, sequentially -- there is
/// no special "split" API; see `commands.rs`'s `finish_work_item` /
/// `finish_hotfix` / `finish_release` for the pattern.
pub async fn with_repo_lock<T>(
    registry: &RepoLockRegistry,
    repo_path: &str,
    f: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let key = canonical_repo_key(repo_path)?;
    let repo_lock = registry.lock_for(&key);
    let _guard = repo_lock.lock().await;
    f()
}

/// Non-blocking acquire -- for background/polling reads (the frontend's 3s/15s
/// timers). `Ok(None)` means the lock is currently held elsewhere; callers
/// treat that as "skip this tick", not an error -- polling is not required to
/// queue behind a mutation. A repository discovery/canonicalization failure
/// is still `Err`: that is a real error, not lock contention, and must not be
/// confused with a busy lock.
pub async fn try_with_repo_lock<T>(
    registry: &RepoLockRegistry,
    repo_path: &str,
    f: impl FnOnce() -> Result<T, String>,
) -> Result<Option<T>, String> {
    let key = canonical_repo_key(repo_path)?;
    let repo_lock = registry.lock_for(&key);
    let result = match repo_lock.try_lock() {
        Ok(_guard) => f().map(Some),
        Err(_) => Ok(None),
    };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn init_repo(dir: &std::path::Path) {
        let run = |args: &[&str]| {
            let status = Command::new("git").args(args).current_dir(dir).status().unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "t@e.com"]);
        run(&["config", "user.name", "T"]);
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-m", "init"]);
    }

    #[tokio::test]
    async fn same_canonical_path_maps_to_the_same_lock() {
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        let path = dir.path().to_str().unwrap();

        let registry = RepoLockRegistry::default();
        let k1 = canonical_repo_key(path).unwrap();
        let k2 = canonical_repo_key(path).unwrap();
        assert_eq!(k1, k2);
        assert!(Arc::ptr_eq(&registry.lock_for(&k1), &registry.lock_for(&k2)));
    }

    #[tokio::test]
    async fn different_repositories_do_not_share_a_lock() {
        let dir_a = tempdir().unwrap();
        let dir_b = tempdir().unwrap();
        init_repo(dir_a.path());
        init_repo(dir_b.path());

        let registry = RepoLockRegistry::default();
        let ka = canonical_repo_key(dir_a.path().to_str().unwrap()).unwrap();
        let kb = canonical_repo_key(dir_b.path().to_str().unwrap()).unwrap();
        assert_ne!(ka, kb);
        assert!(!Arc::ptr_eq(&registry.lock_for(&ka), &registry.lock_for(&kb)));
    }

    #[tokio::test]
    async fn a_subdirectory_resolves_to_the_repo_roots_lock() {
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();

        let root_key = canonical_repo_key(dir.path().to_str().unwrap()).unwrap();
        let sub_key = canonical_repo_key(sub.to_str().unwrap()).unwrap();
        assert_eq!(root_key, sub_key, "a subdirectory must resolve to the repo root, not its own path");
    }

    #[tokio::test]
    async fn try_lock_returns_none_when_already_held() {
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        let path = dir.path().to_str().unwrap();
        let registry = RepoLockRegistry::default();

        let key = canonical_repo_key(path).unwrap();
        let held = registry.lock_for(&key);
        let _guard = held.lock().await; // simulate an in-flight mutation holding the lock

        let result = try_with_repo_lock(&registry, path, || Ok::<_, String>(42)).await;
        assert_eq!(result.unwrap(), None);
    }

    #[tokio::test]
    async fn try_lock_reports_discovery_error_distinctly_from_contention() {
        let dir = tempdir().unwrap(); // never git-inited
        let registry = RepoLockRegistry::default();
        let result =
            try_with_repo_lock(&registry, dir.path().to_str().unwrap(), || Ok::<_, String>(1)).await;
        assert!(result.is_err(), "a non-repository path must be Err, not Ok(None)");
    }

    #[tokio::test]
    async fn guard_releases_the_lock_after_the_closure_completes_or_errors() {
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        let path = dir.path().to_str().unwrap();
        let registry = RepoLockRegistry::default();

        // An `Err` return from the closure must still release the lock.
        let _ = with_repo_lock(&registry, path, || Err::<(), String>("boom".into())).await;
        let after_err = try_with_repo_lock(&registry, path, || Ok::<_, String>(1)).await;
        assert_eq!(after_err.unwrap(), Some(1));

        // A normal completion also releases it.
        with_repo_lock(&registry, path, || Ok::<_, String>(())).await.unwrap();
        let after_ok = try_with_repo_lock(&registry, path, || Ok::<_, String>(2)).await;
        assert_eq!(after_ok.unwrap(), Some(2));
    }
}
