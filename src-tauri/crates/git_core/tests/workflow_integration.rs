//! End-to-end backend workflow flows, chaining `git_core`'s public API
//! against real temp repos (git CLI for setup, git2 for the assertions the
//! library itself uses). Per-module unit tests already cover each op in
//! isolation; these prove the ops compose the way the Tauri command layer
//! drives them.

use std::path::Path;
use std::process::Command;

use git2::Repository;
use git_core::{
    commit_all, commit_merge, compare_refs, create_hotfix_branch, create_work_branch,
    fast_forward_from_origin, merge_target_into_head, push, read_repository_state, restore_work,
    save_work, verify_resolved, BranchKind, SyncError,
};
use tempfile::{tempdir, TempDir};

fn run(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git missing")
        .success();
    assert!(ok, "git {args:?} failed");
}

fn commit_file(dir: &Path, file: &str, content: &str, msg: &str) {
    std::fs::write(dir.join(file), content).unwrap();
    run(dir, &["add", file]);
    run(dir, &["commit", "-m", msg]);
}

/// A repo with `default_branch` checked out and one seed commit.
fn seed_repo(default_branch: &str) -> TempDir {
    let dir = tempdir().unwrap();
    run(dir.path(), &["init", "-b", default_branch]);
    run(dir.path(), &["config", "user.email", "t@e.com"]);
    run(dir.path(), &["config", "user.name", "T"]);
    commit_file(dir.path(), "README", "seed\n", "init");
    dir
}

/// Bare `origin` on `develop` + a working clone of it.
fn origin_and_clone() -> (TempDir, TempDir) {
    let seed = seed_repo("develop");
    let origin = tempdir().unwrap();
    run(origin.path(), &["init", "--bare", "-b", "develop"]);
    run(seed.path(), &["remote", "add", "origin", origin.path().to_str().unwrap()]);
    run(seed.path(), &["push", "origin", "develop"]);

    let clone = tempdir().unwrap();
    run(clone.path(), &["clone", origin.path().to_str().unwrap(), clone.path().to_str().unwrap()]);
    run(clone.path(), &["config", "user.email", "t@e.com"]);
    run(clone.path(), &["config", "user.name", "T"]);
    (origin, clone)
}

#[test]
fn feature_flow_branch_commit_push() {
    let (origin, clone) = origin_and_clone();
    let repo = Repository::open(clone.path()).unwrap();

    let branch = create_work_branch(&repo, BranchKind::Feature, "add-widget", "develop").unwrap();
    assert_eq!(branch, "feature/add-widget");

    std::fs::write(clone.path().join("widget.txt"), "hi\n").unwrap();
    commit_all(&repo, "feat: widget").unwrap();
    push(&repo, &branch).unwrap();

    // origin now carries the pushed branch.
    let origin_repo = Repository::open(origin.path()).unwrap();
    assert!(origin_repo.find_branch(&branch, git2::BranchType::Local).is_ok());
}

#[test]
fn bug_branch_uses_bug_prefix() {
    let dir = seed_repo("develop");
    let repo = Repository::open(dir.path()).unwrap();
    let branch = create_work_branch(&repo, BranchKind::Bug, "off-by-one", "develop").unwrap();
    assert_eq!(branch, "bug/off-by-one");
}

#[test]
fn hotfix_flow_bumps_version_off_master() {
    let dir = seed_repo("master");
    commit_file(dir.path(), "VERSION", "1.4.2\n", "chore: version");
    let repo = Repository::open(dir.path()).unwrap();

    let branch = create_hotfix_branch(&repo, "urgent-fix").unwrap();
    assert_eq!(branch, "hotfix/urgent-fix");
    assert_eq!(read_repository_state(&repo).unwrap().current_branch, "hotfix/urgent-fix");

    let version = std::fs::read_to_string(dir.path().join("VERSION")).unwrap();
    assert_eq!(version.trim(), "1.4.3", "hotfix must bump patch");

    // The bump is its own commit on top of the seed history.
    let count = Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(count.stdout).unwrap().trim(), "3");
}

#[test]
fn conflict_resolution_roundtrip() {
    let dir = seed_repo("develop");
    commit_file(dir.path(), "shared.txt", "base\n", "init shared");
    run(dir.path(), &["checkout", "-b", "feature/x"]);
    commit_file(dir.path(), "shared.txt", "feature side\n", "feat: edit");
    run(dir.path(), &["checkout", "develop"]);
    commit_file(dir.path(), "shared.txt", "develop side\n", "chore: edit");
    run(dir.path(), &["checkout", "feature/x"]);

    let repo = Repository::open(dir.path()).unwrap();
    let merge = merge_target_into_head(&repo, "develop").unwrap();
    assert_eq!(merge.conflicting_files, vec!["shared.txt".to_string()]);

    // Unresolved markers -> verify fails.
    assert!(verify_resolved(&repo, dir.path()).is_err());

    // Owner resolves in the working directory, then verify passes.
    std::fs::write(dir.path().join("shared.txt"), "resolved\n").unwrap();
    verify_resolved(&repo, dir.path()).unwrap();

    // Recorded as a real two-parent merge commit; merge state cleared.
    let oid = commit_merge(&repo, merge.target_commit, "merge develop into feature/x").unwrap();
    assert_eq!(repo.find_commit(oid).unwrap().parent_count(), 2);
    assert!(repo.find_reference("MERGE_HEAD").is_err());
}

#[test]
fn work_safe_save_and_restore_roundtrip() {
    let dir = seed_repo("develop");
    let mut repo = Repository::open(dir.path()).unwrap();

    std::fs::write(dir.path().join("README"), "seed\nedit\n").unwrap();
    std::fs::write(dir.path().join("scratch.txt"), "untracked\n").unwrap();

    let saved = save_work(&mut repo, "wip").unwrap().expect("dirty tree -> Some");
    assert!(!read_repository_state(&repo).unwrap().working_tree.is_dirty(), "clean after save");
    assert!(!dir.path().join("scratch.txt").exists(), "untracked stashed too");

    restore_work(&mut repo, &saved.stash_oid).unwrap();
    assert_eq!(std::fs::read_to_string(dir.path().join("README")).unwrap(), "seed\nedit\n");
    assert!(dir.path().join("scratch.txt").exists(), "untracked restored");
}

#[test]
fn divergence_blocks_fast_forward() {
    let (origin, clone) = origin_and_clone();

    // origin/develop moves forward via a second clone...
    let other = tempdir().unwrap();
    run(other.path(), &["clone", origin.path().to_str().unwrap(), other.path().to_str().unwrap()]);
    run(other.path(), &["config", "user.email", "t@e.com"]);
    run(other.path(), &["config", "user.name", "T"]);
    commit_file(other.path(), "remote.txt", "remote\n", "remote commit");
    run(other.path(), &["push", "origin", "develop"]);

    // ...while the first clone commits locally, then fetches.
    commit_file(clone.path(), "local.txt", "local\n", "local commit");
    run(clone.path(), &["fetch", "origin"]);

    let repo = Repository::open(clone.path()).unwrap();
    let d = compare_refs(&repo, "refs/heads/develop", "refs/remotes/origin/develop").unwrap();
    assert!(d.is_diverged());

    match fast_forward_from_origin(&repo, "develop") {
        Err(SyncError::Diverged { .. }) => {}
        other => panic!("expected Diverged, got {other:?}"),
    }
}
