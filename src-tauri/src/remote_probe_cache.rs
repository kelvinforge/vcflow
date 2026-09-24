//! Last-known remote-connectivity/provider-probe cache. Fixes the bug where
//! the 3s local-only poll (`get_repo_status`, meant to be network-free) was
//! calling the same unconditional live SSH+provider probe as the 15s network
//! refresh, and closes baseline audit issue #7 (the Setup Card's own live
//! probe and this one disagreeing, since neither shared a result with the
//! other).
//!
//! Keyed by canonical repository root -- same key function as
//! `repo_lock::canonical_repo_key`, reused rather than re-derived, so a probe
//! and a repo-mutation lock always agree on "which repository is this."
//!
//! This is a plain `std::sync::Mutex<HashMap<...>>`, structurally identical
//! to `RepoLockRegistry` -- deliberately not a new concurrency primitive for
//! the codebase to learn. It is never nested inside `with_repo_lock`/
//! `try_with_repo_lock`; a cache read/write is a brief, independent
//! operation, not a repository touch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

/// The probe outcome fields, grouped so `set`/`set_transport` stay under
/// clippy's argument-count limit instead of taking each field separately.
#[derive(Debug, Clone)]
pub struct ProbedFields {
    pub ssh_ok: bool,
    pub ssh_error: Option<String>,
    pub gitlab_ok: bool,
    pub gitlab_error: Option<String>,
    pub role: Option<String>,
}

/// One repository's last real remote probe. `remote_url` is the URL the probe
/// was actually run against -- a caller whose *current* remote_url no longer
/// matches this must treat the entry as stale (the origin changed under it),
/// never serve it as current.
#[derive(Debug, Clone)]
pub struct CachedProbe {
    pub remote_url: Option<String>,
    // ponytail: recorded but not yet read anywhere -- the design keeps this
    // for a future staleness display on the DTO; add that read when needed.
    #[allow(dead_code)]
    pub checked_at: Instant,
    pub generation: u64,
    pub ssh_ok: bool,
    pub ssh_error: Option<String>,
    pub gitlab_ok: bool,
    pub gitlab_error: Option<String>,
    pub role: Option<String>,
}

#[derive(Default)]
pub struct RemoteProbeCache {
    entries: Mutex<HashMap<PathBuf, CachedProbe>>,
}

impl RemoteProbeCache {
    /// The cached probe for `canonical_root`, only when its `remote_url`
    /// still matches `current_remote_url` -- a URL mismatch (repo's origin
    /// changed since the probe ran) returns `None`, the same as "never
    /// probed", rather than serving a result computed against a URL that is
    /// no longer `origin`.
    pub fn get(&self, canonical_root: &Path, current_remote_url: &Option<String>) -> Option<CachedProbe> {
        let map = self.entries.lock().expect("RemoteProbeCache map poisoned");
        let entry = map.get(canonical_root)?;
        if entry.remote_url != *current_remote_url {
            return None;
        }
        Some(entry.clone())
    }

    /// Records a fresh probe result, bumping `generation`. Called only from
    /// the network-probing path (`fresh_remote_probe`) and, best-effort, from
    /// `repository_preflight` when it learns the same fact live.
    pub fn set(&self, canonical_root: PathBuf, remote_url: Option<String>, probed: ProbedFields) {
        let mut map = self.entries.lock().expect("RemoteProbeCache map poisoned");
        let generation = map.get(&canonical_root).map(|e| e.generation + 1).unwrap_or(0);
        map.insert(
            canonical_root,
            CachedProbe {
                remote_url,
                checked_at: Instant::now(),
                generation,
                ssh_ok: probed.ssh_ok,
                ssh_error: probed.ssh_error,
                gitlab_ok: probed.gitlab_ok,
                gitlab_error: probed.gitlab_error,
                role: probed.role,
            },
        );
    }

    /// Records a fresh *transport-only* result -- what `repository_preflight`
    /// learns (git-transport reachable + authenticated), which never
    /// includes a provider API probe. Preserves whatever `gitlab_ok`/
    /// `gitlab_error`/`role` the cache already had (or leaves them at the
    /// "never probed" defaults if there was no entry yet) rather than
    /// clobbering a possibly-fresher provider-API result from
    /// `refresh_repo_status` with preflight's own unknown-for-that-field
    /// values. This is what lets both call sites share one cache without
    /// one's partial probe overwriting the other's.
    pub fn set_transport(
        &self,
        canonical_root: PathBuf,
        remote_url: Option<String>,
        ssh_ok: bool,
        ssh_error: Option<String>,
    ) {
        let mut map = self.entries.lock().expect("RemoteProbeCache map poisoned");
        let (generation, gitlab_ok, gitlab_error, role) = match map.get(&canonical_root) {
            // Same remote_url: this repo's existing provider-probe fields are
            // still about the right target, keep them.
            Some(e) if e.remote_url == remote_url => {
                (e.generation + 1, e.gitlab_ok, e.gitlab_error.clone(), e.role.clone())
            }
            // No entry, or the remote changed: the old provider fields (if
            // any) are about a different target, don't carry them forward.
            Some(e) => (e.generation + 1, false, None, None),
            None => (0, false, None, None),
        };
        map.insert(
            canonical_root,
            CachedProbe {
                remote_url,
                checked_at: Instant::now(),
                generation,
                ssh_ok,
                ssh_error,
                gitlab_ok,
                gitlab_error,
                role,
            },
        );
    }

    /// Drops this repository's cached probe outright -- used when a fact the
    /// probe result depends on but that isn't `remote_url` changes (a token
    /// save/delete): the next read, local or fresh, must not serve a result
    /// computed against the old token.
    pub fn invalidate(&self, canonical_root: &Path) {
        let mut map = self.entries.lock().expect("RemoteProbeCache map poisoned");
        map.remove(canonical_root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probed(ssh_ok: bool, gitlab_ok: bool, role: Option<&str>) -> ProbedFields {
        ProbedFields {
            ssh_ok,
            ssh_error: None,
            gitlab_ok,
            gitlab_error: None,
            role: role.map(str::to_string),
        }
    }

    #[test]
    fn set_then_get_round_trips_when_url_matches() {
        let cache = RemoteProbeCache::default();
        let root = PathBuf::from("/repo/a");
        let url = Some("git@example.com:org/repo.git".to_string());
        cache.set(root.clone(), url.clone(), probed(true, true, Some("Owner")));

        let got = cache.get(&root, &url).unwrap();
        assert!(got.ssh_ok);
        assert!(got.gitlab_ok);
        assert_eq!(got.role, Some("Owner".into()));
        assert_eq!(got.generation, 0);
    }

    #[test]
    fn get_is_none_when_remote_url_no_longer_matches() {
        let cache = RemoteProbeCache::default();
        let root = PathBuf::from("/repo/a");
        cache.set(
            root.clone(),
            Some("git@old.example.com:org/repo.git".to_string()),
            probed(true, true, None),
        );

        let current = Some("git@new.example.com:org/repo.git".to_string());
        assert!(cache.get(&root, &current).is_none());
    }

    #[test]
    fn get_is_none_when_never_probed() {
        let cache = RemoteProbeCache::default();
        let root = PathBuf::from("/repo/a");
        assert!(cache.get(&root, &None).is_none());
    }

    #[test]
    fn different_roots_do_not_share_an_entry() {
        let cache = RemoteProbeCache::default();
        let a = PathBuf::from("/repo/a");
        let b = PathBuf::from("/repo/b");
        cache.set(a.clone(), None, probed(true, true, None));

        assert!(cache.get(&a, &None).is_some());
        assert!(cache.get(&b, &None).is_none());
    }

    #[test]
    fn a_second_set_bumps_generation() {
        let cache = RemoteProbeCache::default();
        let root = PathBuf::from("/repo/a");
        cache.set(root.clone(), None, probed(true, true, None));
        cache.set(root.clone(), None, probed(false, false, None));

        let got = cache.get(&root, &None).unwrap();
        assert_eq!(got.generation, 1);
        assert!(!got.ssh_ok);
    }

    #[test]
    fn set_transport_preserves_existing_provider_fields_for_the_same_url() {
        let cache = RemoteProbeCache::default();
        let root = PathBuf::from("/repo/a");
        let url = Some("git@example.com:org/repo.git".to_string());
        cache.set(root.clone(), url.clone(), probed(true, true, Some("Owner")));

        // preflight learns only transport reachability, nothing about the
        // provider API -- it must not blank out the role a fresher
        // provider-API probe already established.
        cache.set_transport(root.clone(), url.clone(), true, None);

        let got = cache.get(&root, &url).unwrap();
        assert!(got.gitlab_ok);
        assert_eq!(got.role, Some("Owner".into()));
        assert_eq!(got.generation, 1);
    }

    #[test]
    fn set_transport_drops_stale_provider_fields_on_a_different_url() {
        let cache = RemoteProbeCache::default();
        let root = PathBuf::from("/repo/a");
        cache.set(
            root.clone(),
            Some("git@old.example.com:org/repo.git".to_string()),
            probed(true, true, Some("Owner")),
        );

        let new_url = Some("git@new.example.com:org/repo.git".to_string());
        cache.set_transport(root.clone(), new_url.clone(), true, None);

        let got = cache.get(&root, &new_url).unwrap();
        assert!(!got.gitlab_ok);
        assert_eq!(got.role, None);
    }

    #[test]
    fn invalidate_drops_the_entry() {
        let cache = RemoteProbeCache::default();
        let root = PathBuf::from("/repo/a");
        cache.set(root.clone(), None, probed(true, true, None));
        cache.invalidate(&root);

        assert!(cache.get(&root, &None).is_none());
    }
}
