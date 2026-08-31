use std::cell::RefCell;

use git2::{Direction, PushOptions, RemoteCallbacks, Repository, Signature};
use thiserror::Error;

use crate::ssh::make_credentials_callback;

#[derive(Debug, Error)]
pub enum TagError {
    #[error("tag target '{0}' not found")]
    TargetNotFound(String),
    #[error("could not reach origin: {0}")]
    RemoteUnreachable(String),
    #[error("could not push tag: {0}")]
    PushFailed(String),
    #[error(transparent)]
    Git(#[from] git2::Error),
}

/// Whether an `ensure_release_tag` call had to create the tag or found it
/// already published on `origin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagOutcome {
    Created,
    AlreadyPresent,
}

/// True if `origin` already publishes `refs/tags/<tag>`. Used to make tagging
/// idempotent: an Owner who retries the release sync must not fail and must not
/// move an existing tag.
pub fn remote_tag_exists(repo: &Repository, tag: &str) -> Result<bool, TagError> {
    let mut remote = repo.find_remote("origin")?;
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(make_credentials_callback());
    remote
        .connect_auth(Direction::Fetch, Some(callbacks), None)
        .map_err(|e| TagError::RemoteUnreachable(e.message().to_string()))?;
    let want = format!("refs/tags/{tag}");
    let found = remote
        .list()
        .map_err(|e| TagError::RemoteUnreachable(e.message().to_string()))?
        .iter()
        .any(|h| h.name() == want);
    let _ = remote.disconnect();
    Ok(found)
}

/// Creates an annotated tag `tag` pointing at `target_ref` (e.g.
/// `refs/remotes/origin/master`). `force` is always false -- an existing tag is
/// an error here, never silently moved.
pub fn create_annotated_tag(
    repo: &Repository,
    tag: &str,
    target_ref: &str,
    message: &str,
) -> Result<git2::Oid, TagError> {
    let target = repo
        .revparse_single(target_ref)
        .map_err(|_| TagError::TargetNotFound(target_ref.to_string()))?;
    let sig = repo
        .signature()
        .or_else(|_| Signature::now("git-workflow-engine", "noreply@localhost"))?;
    let oid = repo.tag(tag, &target, &sig, message, false)?;
    Ok(oid)
}

/// Pushes `refs/tags/<tag>` to `origin`. Mirrors `commit_push::push`: a
/// server-side rejection comes back via the callback, not as an `Err`.
pub fn push_tag(repo: &Repository, tag: &str) -> Result<(), TagError> {
    let mut remote = repo.find_remote("origin")?;
    let refspec = format!("refs/tags/{tag}:refs/tags/{tag}");

    let rejection = RefCell::new(None);
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(make_credentials_callback());
    callbacks.push_update_reference(|_refname, status| {
        if let Some(msg) = status {
            *rejection.borrow_mut() = Some(msg.to_string());
        }
        Ok(())
    });

    let mut opts = PushOptions::new();
    opts.remote_callbacks(callbacks);

    let push_result = remote
        .push(&[&refspec], Some(&mut opts))
        .map_err(|e| TagError::PushFailed(e.message().to_string()));
    drop(opts);
    push_result?;

    if let Some(msg) = rejection.into_inner() {
        return Err(TagError::PushFailed(msg));
    }
    Ok(())
}

/// Idempotently publish the release tag `tag` at `target_ref`:
///
/// * already on `origin`            -> [`TagOutcome::AlreadyPresent`], no-op
/// * missing                        -> create (reusing a lingering local tag
///   from a half-done prior attempt rather than moving it) and push
///
/// The existing tag is never overwritten or moved.
pub fn ensure_release_tag(
    repo: &Repository,
    tag: &str,
    target_ref: &str,
    message: &str,
) -> Result<TagOutcome, TagError> {
    if remote_tag_exists(repo, tag)? {
        return Ok(TagOutcome::AlreadyPresent);
    }
    if repo.revparse_single(&format!("refs/tags/{tag}")).is_err() {
        create_annotated_tag(repo, tag, target_ref, message)?;
    }
    push_tag(repo, tag)?;
    Ok(TagOutcome::Created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use tempfile::{tempdir, TempDir};

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").args(args).current_dir(dir).status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    fn git_out(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").args(args).current_dir(dir).output().unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// Work repo on `master` with a bare `origin`; the work repo's
    /// `origin/master` tracks one pushed commit -- the "production tip".
    fn repo_with_origin() -> (TempDir, TempDir) {
        let origin = tempdir().unwrap();
        git(origin.path(), &["init", "--bare", "-b", "master"]);
        let work = tempdir().unwrap();
        git(work.path(), &["init", "-b", "master"]);
        git(work.path(), &["config", "user.email", "t@e.com"]);
        git(work.path(), &["config", "user.name", "T"]);
        std::fs::write(work.path().join("VERSION"), "0.2.3\n").unwrap();
        git(work.path(), &["add", "."]);
        git(work.path(), &["commit", "-m", "release 0.2.3"]);
        git(work.path(), &["remote", "add", "origin", origin.path().to_str().unwrap()]);
        git(work.path(), &["push", "origin", "master"]);
        (origin, work)
    }

    #[test]
    fn ensure_release_tag_points_at_the_production_commit() {
        let (_origin, work) = repo_with_origin();
        let repo = Repository::open(work.path()).unwrap();
        let prod_tip = git_out(work.path(), &["rev-parse", "refs/remotes/origin/master"]);

        let outcome = ensure_release_tag(
            &repo,
            "v0.2.3",
            "refs/remotes/origin/master",
            "Release 0.2.3",
        )
        .unwrap();

        assert_eq!(outcome, TagOutcome::Created);
        // annotated tag -> dereference with ^{} to reach the commit it wraps.
        assert_eq!(git_out(work.path(), &["rev-parse", "v0.2.3^{commit}"]), prod_tip);
        assert_eq!(git_out(work.path(), &["cat-file", "-t", "v0.2.3"]), "tag");
        // and it made it to origin.
        assert!(remote_tag_exists(&repo, "v0.2.3").unwrap());
    }

    #[test]
    fn ensure_release_tag_skips_an_existing_tag_without_moving_it() {
        let (_origin, work) = repo_with_origin();
        let repo = Repository::open(work.path()).unwrap();

        ensure_release_tag(&repo, "v0.2.3", "refs/remotes/origin/master", "Release 0.2.3").unwrap();
        let first = git_out(work.path(), &["rev-parse", "v0.2.3"]);

        // Move the production tip on; a retry must NOT re-point the tag.
        std::fs::write(work.path().join("VERSION"), "0.2.4\n").unwrap();
        git(work.path(), &["commit", "-am", "more"]);
        git(work.path(), &["push", "origin", "master"]);

        let outcome =
            ensure_release_tag(&repo, "v0.2.3", "refs/remotes/origin/master", "Release 0.2.3")
                .unwrap();

        assert_eq!(outcome, TagOutcome::AlreadyPresent);
        assert_eq!(git_out(work.path(), &["rev-parse", "v0.2.3"]), first);
    }

    #[test]
    fn create_annotated_tag_reports_a_missing_target() {
        let (_origin, work) = repo_with_origin();
        let repo = Repository::open(work.path()).unwrap();
        let err = create_annotated_tag(&repo, "v9.9.9", "refs/remotes/origin/nope", "x").unwrap_err();
        assert!(matches!(err, TagError::TargetNotFound(_)));
    }

    #[test]
    fn push_tag_surfaces_a_transport_failure() {
        let (origin, work) = repo_with_origin();
        let repo = Repository::open(work.path()).unwrap();
        create_annotated_tag(&repo, "v0.2.3", "refs/remotes/origin/master", "Release 0.2.3").unwrap();

        // origin goes away -> the push must fail loudly, not silently succeed.
        drop(origin);
        let err = push_tag(&repo, "v0.2.3").unwrap_err();
        assert!(matches!(err, TagError::PushFailed(_) | TagError::Git(_)));
    }
}
