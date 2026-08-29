//! Strict application-eligibility gate. Read-only: nothing here mutates the
//! repo, fetches, stashes, commits, or creates branches. Its single job is to
//! decide whether the app is allowed to operate on a repository at all.
//!
//! Seven mandatory checks; any single failure blocks the app. A missing
//! `develop` branch and a dirty working tree are deliberately NOT failures --
//! those are workflow states handled by Initial Workflow Setup
//! (`commands::initialize_workflow`), not eligibility problems.
//!
//! `assemble_preflight` takes already-gathered inputs (git version string,
//! opened repo, provider classification, remote-connection result) so the whole
//! check list is unit-testable without a Tauri/network layer.

use git2::Repository;

use crate::repo::head_branch;
use crate::ssh::SshError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warning,
    Fail,
}

impl CheckStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckStatus::Pass => "pass",
            CheckStatus::Warning => "warning",
            CheckStatus::Fail => "fail",
        }
    }
}

/// One eligibility check. `message` explains, on failure, what is missing and
/// what the user must do. No UI colors/presentation -- the frontend maps
/// `status` itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub id: &'static str,
    pub status: CheckStatus,
    pub title: String,
    pub message: String,
    /// Every preflight check is blocking. Kept explicit so the frontend
    /// contract is self-describing.
    pub blocking: bool,
}

impl Check {
    fn pass(id: &'static str, title: &str, message: &str) -> Self {
        Self { id, status: CheckStatus::Pass, title: title.into(), message: message.into(), blocking: true }
    }
    fn fail(id: &'static str, title: &str, message: &str) -> Self {
        Self { id, status: CheckStatus::Fail, title: title.into(), message: message.into(), blocking: true }
    }
    /// A check that can't be evaluated because an earlier one failed -- still
    /// emitted (as a blocking Fail) so the frontend always renders all 7 rows.
    fn blocked(id: &'static str, title: &str) -> Self {
        Self {
            id,
            status: CheckStatus::Fail,
            title: title.into(),
            message: "Could not check this yet -- resolve the earlier failed check first.".into(),
            blocking: true,
        }
    }
}

/// Provider classification the caller passes in. Keeps `git_core` free of a
/// `provider_core` dependency; the command layer maps `provider_core::Provider`
/// (plus its self-hosted live probe) onto this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreflightProvider {
    GitHub,
    GitLab,
    /// A reachable host that is neither GitHub nor GitLab, or one that could
    /// not be classified.
    Unsupported,
}

#[derive(Debug, Clone)]
pub struct Preflight {
    pub checks: Vec<Check>,
    /// True only when every check passed.
    pub eligible: bool,

    // --- raw facts, for the frontend and for Initial Workflow Setup ---
    pub git_version: Option<String>,
    pub is_repo: bool,
    pub has_commits: bool,
    /// Git repo initialized but with zero commits (unborn HEAD).
    pub unborn: bool,
    pub current_branch: Option<String>,
    pub remote_url: Option<String>,
    pub provider: Option<PreflightProvider>,
}

/// Builds the seven-check eligibility report.
///
/// - `git_version`: `Some` = `git --version` succeeded, `None` = Git not found.
/// - `repo`: `Some` = a repository was opened, `None` = the folder is not a git repo.
/// - `provider`: classification of `origin`'s URL, or `None` when there is no origin.
/// - `remote_conn`: result of a single `connect_auth` against `origin`, or `None`
///   when it was not attempted (no repo / no origin).
pub fn assemble_preflight(
    git_version: Option<String>,
    repo: Option<&Repository>,
    provider: Option<PreflightProvider>,
    remote_conn: Option<Result<(), SshError>>,
) -> Preflight {
    let is_repo = repo.is_some();

    let remote_url = repo.and_then(|r| {
        r.find_remote("origin").ok().and_then(|rm| rm.url().map(str::to_string))
    });
    let remote_ok = remote_url.is_some();

    let (current_branch, head_ok) = match repo {
        Some(r) => match head_branch(r) {
            Ok(Some(b)) => (Some(b), true),
            _ => (None, false),
        },
        None => (None, false),
    };

    let has_commits = repo
        .map(|r| r.head().and_then(|h| h.peel_to_commit()).is_ok())
        .unwrap_or(false);
    let unborn = is_repo && !has_commits;

    let usable = match repo {
        Some(r) => !r.is_bare() && r.workdir().is_some(),
        None => false,
    };

    let git_ok = git_version.is_some();

    let (reach, auth, conn_reason) = match &remote_conn {
        Some(r) => classify_remote_conn(r),
        None => (CheckStatus::Fail, CheckStatus::Fail, String::new()),
    };

    let mut checks = Vec::with_capacity(7);

    // 1 -- Git installed
    checks.push(match &git_version {
        Some(v) => Check::pass("git_installed", "Git installed", &format!("Git is available ({v}).")),
        None => Check::fail(
            "git_installed",
            "Git installed",
            "Git was not found on your system. Install Git and restart the app.",
        ),
    });

    // 2 -- Remote configured
    checks.push(if !git_ok {
        Check::blocked("remote_configured", "Remote configured")
    } else if remote_ok {
        Check::pass("remote_configured", "Remote configured", "An 'origin' remote is configured.")
    } else if !is_repo {
        Check::fail(
            "remote_configured",
            "Remote configured",
            "This folder is not a Git repository, so it has no remote. Run `git init` yourself, then add a GitHub or GitLab 'origin' remote.",
        )
    } else {
        Check::fail(
            "remote_configured",
            "Remote configured",
            "No Git remote named \"origin\" was found. Add a GitHub or GitLab remote before continuing (git remote add origin <url>).",
        )
    });

    // 3 -- GitHub / GitLab detected
    checks.push(if !git_ok || !remote_ok {
        Check::blocked("provider_detected", "GitHub or GitLab detected")
    } else {
        match provider {
            Some(PreflightProvider::GitHub) => {
                Check::pass("provider_detected", "GitHub or GitLab detected", "The 'origin' remote is a GitHub repository.")
            }
            Some(PreflightProvider::GitLab) => {
                Check::pass("provider_detected", "GitHub or GitLab detected", "The 'origin' remote is a GitLab repository.")
            }
            _ => Check::fail(
                "provider_detected",
                "GitHub or GitLab detected",
                "The 'origin' remote is not a GitHub or GitLab repository. This app supports GitHub and GitLab only -- point 'origin' at a GitHub or GitLab repository.",
            ),
        }
    });

    // 4 -- Remote reachable
    checks.push(if !git_ok || !remote_ok {
        Check::blocked("remote_reachable", "Remote reachable")
    } else if reach == CheckStatus::Pass {
        Check::pass("remote_reachable", "Remote reachable", "The 'origin' remote responded.")
    } else {
        Check::fail(
            "remote_reachable",
            "Remote reachable",
            &format!(
                "Could not reach the 'origin' remote: {}. Check the remote URL and your network connection.",
                if conn_reason.is_empty() { "no connection" } else { &conn_reason }
            ),
        )
    });

    // 5 -- Git authentication available (Git transport auth -- NOT the app's API token)
    checks.push(if !git_ok || !remote_ok || reach != CheckStatus::Pass {
        Check::blocked("git_auth_available", "Git authentication available")
    } else if auth == CheckStatus::Pass {
        Check::pass("git_auth_available", "Git authentication available", "Git can authenticate to the 'origin' remote.")
    } else {
        Check::fail(
            "git_auth_available",
            "Git authentication available",
            "Git could not authenticate to the 'origin' remote. Check your SSH key/agent or Git credential helper. (This is separate from the GitHub/GitLab API token the app uses.)",
        )
    });

    // 6 -- Required repository information available
    checks.push(if !git_ok {
        Check::blocked("repo_info_available", "Repository information available")
    } else if !is_repo {
        Check::fail(
            "repo_info_available",
            "Repository information available",
            "This folder is not a Git repository. Initialize it with `git init` and add a GitHub or GitLab 'origin' remote.",
        )
    } else if head_ok && remote_url.is_some() {
        Check::pass("repo_info_available", "Repository information available", "Branch and remote information is readable.")
    } else {
        Check::fail(
            "repo_info_available",
            "Repository information available",
            "Could not read the repository's branch/remote information (detached HEAD or a damaged repository). Check out a branch before continuing.",
        )
    });

    // 7 -- Current repository is usable
    checks.push(if !git_ok {
        Check::blocked("repo_usable", "Repository is usable")
    } else if !is_repo {
        Check::fail(
            "repo_usable",
            "Repository is usable",
            "This folder is not a Git repository. The app will not initialize it for you -- run `git init` yourself, then retry.",
        )
    } else if usable {
        Check::pass("repo_usable", "Repository is usable", "The repository has a working tree and is ready.")
    } else {
        Check::fail(
            "repo_usable",
            "Repository is usable",
            "This repository has no working tree (bare repository). Open a normal working clone instead.",
        )
    });

    let eligible = checks.iter().all(|c| c.status == CheckStatus::Pass);

    Preflight {
        checks,
        eligible,
        git_version,
        is_repo,
        has_commits,
        unborn,
        current_branch,
        remote_url,
        provider,
    }
}

/// Splits a single `connect_auth` result into the (reachable, auth) verdicts.
///
/// `validate_remote_connection` makes one connection; its `Unreachable`
/// message text (produced by `ssh::human_reason`) is what tells "we reached the
/// host but auth failed" apart from "we never reached the host".
fn classify_remote_conn(conn: &Result<(), SshError>) -> (CheckStatus, CheckStatus, String) {
    match conn {
        Ok(()) => (CheckStatus::Pass, CheckStatus::Pass, String::new()),
        Err(SshError::NoOrigin) => {
            (CheckStatus::Fail, CheckStatus::Fail, "no 'origin' remote".to_string())
        }
        Err(SshError::Unreachable(msg)) => {
            if msg.contains("authentication") {
                // host answered, credentials rejected
                (CheckStatus::Pass, CheckStatus::Fail, msg.clone())
            } else {
                (CheckStatus::Fail, CheckStatus::Fail, msg.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use tempfile::{tempdir, TempDir};

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git").args(args).current_dir(dir).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn repo_on_main() -> TempDir {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "main"]);
        run(dir.path(), &["config", "user.email", "t@e.com"]);
        run(dir.path(), &["config", "user.name", "T"]);
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        run(dir.path(), &["add", "a.txt"]);
        run(dir.path(), &["commit", "-m", "init"]);
        dir
    }

    fn find(pf: &Preflight, id: &str) -> Check {
        pf.checks.iter().find(|c| c.id == id).cloned().unwrap_or_else(|| panic!("no check {id}"))
    }

    #[test]
    fn not_a_git_repo_is_blocked_and_never_runs_git_init() {
        let dir = tempdir().unwrap();
        let pf = assemble_preflight(Some("git version 2.40".into()), None, None, None);

        assert!(!pf.eligible);
        assert!(!pf.is_repo);
        assert_eq!(find(&pf, "repo_usable").status, CheckStatus::Fail);
        assert!(find(&pf, "repo_usable").blocking);
        // preflight is read-only -- it must not have created a repository
        assert!(!dir.path().join(".git").exists());
    }

    #[test]
    fn git_missing_blocks_everything() {
        let pf = assemble_preflight(None, None, None, None);
        assert_eq!(find(&pf, "git_installed").status, CheckStatus::Fail);
        assert!(pf.checks.iter().all(|c| c.status == CheckStatus::Fail));
    }

    #[test]
    fn repo_without_origin_fails_remote_configured() {
        let dir = repo_on_main();
        let repo = Repository::open(dir.path()).unwrap();
        let pf = assemble_preflight(Some("git version 2.40".into()), Some(&repo), None, None);

        assert_eq!(find(&pf, "remote_configured").status, CheckStatus::Fail);
        assert!(!pf.eligible);
    }

    #[test]
    fn unsupported_provider_fails() {
        let dir = repo_on_main();
        run(dir.path(), &["remote", "add", "origin", "https://bitbucket.org/x/y.git"]);
        let repo = Repository::open(dir.path()).unwrap();
        let pf = assemble_preflight(
            Some("git version 2.40".into()),
            Some(&repo),
            Some(PreflightProvider::Unsupported),
            Some(Ok(())),
        );

        assert_eq!(find(&pf, "provider_detected").status, CheckStatus::Fail);
        assert!(!pf.eligible);
    }

    #[test]
    fn github_and_gitlab_pass_provider_check() {
        for (url, prov) in [
            ("git@github.com:o/r.git", PreflightProvider::GitHub),
            ("https://gitlab.com/g/r.git", PreflightProvider::GitLab),
        ] {
            let dir = repo_on_main();
            run(dir.path(), &["remote", "add", "origin", url]);
            let repo = Repository::open(dir.path()).unwrap();
            let pf = assemble_preflight(
                Some("git version 2.40".into()),
                Some(&repo),
                Some(prov),
                Some(Ok(())),
            );
            assert_eq!(find(&pf, "provider_detected").status, CheckStatus::Pass, "url {url}");
        }
    }

    #[test]
    fn unreachable_remote_fails_reachable_and_blocks_auth() {
        let dir = repo_on_main();
        run(dir.path(), &["remote", "add", "origin", "https://github.com/o/r.git"]);
        let repo = Repository::open(dir.path()).unwrap();
        let conn = Err(SshError::Unreachable("host not found -- check remote URL/DNS".into()));
        let pf = assemble_preflight(
            Some("git version 2.40".into()),
            Some(&repo),
            Some(PreflightProvider::GitHub),
            Some(conn),
        );

        assert_eq!(find(&pf, "remote_reachable").status, CheckStatus::Fail);
        assert_eq!(find(&pf, "git_auth_available").status, CheckStatus::Fail);
    }

    #[test]
    fn classify_remote_conn_splits_reachable_from_auth() {
        let (r, a, _) = classify_remote_conn(&Ok(()));
        assert_eq!((r, a), (CheckStatus::Pass, CheckStatus::Pass));

        let (r, a, _) = classify_remote_conn(&Err(SshError::Unreachable(
            "authentication failed -- check SSH key/agent".into(),
        )));
        assert_eq!((r, a), (CheckStatus::Pass, CheckStatus::Fail));

        let (r, a, _) = classify_remote_conn(&Err(SshError::Unreachable(
            "host not found -- check remote URL/DNS".into(),
        )));
        assert_eq!((r, a), (CheckStatus::Fail, CheckStatus::Fail));
    }

    #[test]
    fn everything_good_is_eligible() {
        let dir = repo_on_main();
        run(dir.path(), &["remote", "add", "origin", "git@github.com:o/r.git"]);
        let repo = Repository::open(dir.path()).unwrap();
        let pf = assemble_preflight(
            Some("git version 2.40".into()),
            Some(&repo),
            Some(PreflightProvider::GitHub),
            Some(Ok(())),
        );

        assert!(pf.eligible, "checks: {:#?}", pf.checks);
        assert_eq!(pf.checks.len(), 7);
        assert!(pf.checks.iter().all(|c| c.status == CheckStatus::Pass));
        assert!(pf.has_commits);
        assert!(!pf.unborn);
        assert_eq!(pf.current_branch.as_deref(), Some("main"));
    }

    #[test]
    fn dirty_tree_and_missing_develop_do_not_break_eligibility() {
        let dir = repo_on_main();
        run(dir.path(), &["remote", "add", "origin", "git@github.com:o/r.git"]);
        std::fs::write(dir.path().join("a.txt"), "changed\n").unwrap();
        std::fs::write(dir.path().join("untracked.txt"), "x\n").unwrap();
        let repo = Repository::open(dir.path()).unwrap();
        let pf = assemble_preflight(
            Some("git version 2.40".into()),
            Some(&repo),
            Some(PreflightProvider::GitHub),
            Some(Ok(())),
        );
        assert!(pf.eligible);
    }

    #[test]
    fn unborn_repo_with_good_remote_is_eligible() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init", "-b", "main"]);
        run(dir.path(), &["remote", "add", "origin", "git@github.com:o/r.git"]);
        let repo = Repository::open(dir.path()).unwrap();
        let pf = assemble_preflight(
            Some("git version 2.40".into()),
            Some(&repo),
            Some(PreflightProvider::GitHub),
            Some(Ok(())),
        );
        assert!(pf.eligible, "checks: {:#?}", pf.checks);
        assert!(pf.unborn);
        assert!(!pf.has_commits);
    }
}
