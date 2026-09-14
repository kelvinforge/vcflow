//! Snapshot assembly + provider-client resolution shared by every VCFlow
//! consumer (Tauri commands today, a standalone binary and future adapters
//! later). Tauri-free: callers own presentation, this crate owns "what is
//! the workflow state and what should happen next".

use git2::Repository;
use serde::Serialize;

use auth_core::{
    AuditEntry, AuditLog, Capability, CommandLog, Config, ConflictLog, OverrideRole, RoleOverride,
    SavedWorkLog, SavedWorkRecord, WipItemLog, WorkItemLog,
};
use provider_core::github::{github_api_base, GitHubClient};
use provider_core::gitlab::GitLabClient;
use provider_core::{
    detect_provider, MergeRequest, Mergeability, MergeStatus, Provider, ProviderError,
    Role as ProviderRole,
};
use workflow_engine::{
    next_action as compute_next_action, AllowedAction, BranchClass, MrSnapshot, PrimaryAction,
    Role as WorkflowRole, WorkflowSnapshot, WorkItemState,
};

/// The repo's production branch: `main` (GitHub default) or `master`
/// (GitLab default), whichever this repo actually has. `main` when neither
/// exists yet.
pub fn resolve_production(repo: &Repository) -> Result<String, String> {
    git_core::production_branch(repo)
        .ok_or_else(|| "this repository has no 'main' or 'master' branch".to_string())
}

pub fn current_branch(repo: &Repository) -> Result<String, String> {
    git_core::head_branch(repo)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "HEAD is not a branch (detached HEAD)".to_string())
}

pub fn classify_branch(name: &str) -> BranchClass {
    match name {
        "develop" => BranchClass::Develop,
        "master" | "main" => BranchClass::Master,
        _ if name.starts_with("hotfix/") => BranchClass::Hotfix,
        _ if name.starts_with("release/") => BranchClass::Release,
        _ if name.starts_with("feature/")
            || name.starts_with("bug/")
            || name.starts_with("chore/") =>
        {
            BranchClass::WorkItem
        }
        _ => BranchClass::Other,
    }
}

pub fn work_item_log() -> Option<WorkItemLog> {
    AuditLog::default_path().ok().and_then(|p| WorkItemLog::open(p).ok())
}

pub fn wip_item_log() -> Option<WipItemLog> {
    AuditLog::default_path().ok().and_then(|p| WipItemLog::open(p).ok())
}

/// Maps the best-effort display role to the pure-logic role
/// `workflow_engine::transition` gates on. Anything short of a confirmed
/// Owner is treated as Member -- the only actions gated to Member are
/// lower-privilege than Owner ones, so defaulting an unresolved role to
/// Member is the safe direction.
pub async fn resolve_workflow_role(repo_path: &str) -> WorkflowRole {
    let info = match git_core::read_repo_info(repo_path) {
        Ok(info) => info,
        Err(_) => return WorkflowRole::Member,
    };
    match resolve_provider_role(&info.remote_url).await {
        Some(OverrideRole::Owner) => WorkflowRole::Owner,
        _ => WorkflowRole::Member,
    }
}

/// Provider API client resolved for a repo's `origin` remote. Every workflow
/// API call goes through this enum so a GitLab repo never runs GitHub code
/// and vice versa -- the variant is decided once, here, from the remote URL.
pub enum ApiClient {
    GitLab(GitLabClient),
    GitHub(GitHubClient),
}

impl ApiClient {
    pub fn provider(&self) -> Provider {
        match self {
            ApiClient::GitLab(_) => Provider::GitLab,
            ApiClient::GitHub(_) => Provider::GitHub,
        }
    }

    pub async fn get_current_user_role(&self) -> Result<ProviderRole, ProviderError> {
        match self {
            ApiClient::GitLab(c) => c.get_current_user_role().await,
            ApiClient::GitHub(c) => c.get_current_user_role().await,
        }
    }

    pub async fn get_merge_request(&self, id: &str) -> Result<MergeRequest, ProviderError> {
        match self {
            ApiClient::GitLab(c) => c.get_merge_request(id).await,
            ApiClient::GitHub(c) => c.get_merge_request(id).await,
        }
    }

    pub async fn check_mergeability(&self, id: &str) -> Result<Mergeability, ProviderError> {
        match self {
            ApiClient::GitLab(c) => c.check_mergeability(id).await,
            ApiClient::GitHub(c) => c.check_mergeability(id).await,
        }
    }

    pub async fn create_merge_request(
        &self,
        source_branch: &str,
        target_branch: &str,
        title: &str,
    ) -> Result<MergeRequest, ProviderError> {
        match self {
            ApiClient::GitLab(c) => c.create_merge_request(source_branch, target_branch, title).await,
            ApiClient::GitHub(c) => c.create_merge_request(source_branch, target_branch, title).await,
        }
    }

    pub async fn detect_capabilities(&self) -> provider_core::CapabilityReport {
        match self {
            ApiClient::GitLab(c) => provider_core::gitlab::detect_capabilities(c).await,
            ApiClient::GitHub(c) => provider_core::github::detect_capabilities(c).await,
        }
    }
}

/// Resolves the provider API client for a repo's `origin` URL, reading the
/// matching keychain credential (`github|<host>|default` or
/// `gitlab|<host>|default`). `None` when the remote can't be parsed, no token
/// is stored for it, or the API host can't be reached.
///
/// A host the URL doesn't positively classify (self-hosted) is probed live --
/// GitLab first (the historical case), then GitHub Enterprise.
pub async fn provider_client_for(remote_url: &Option<String>) -> Option<ApiClient> {
    let url = remote_url.as_deref()?;
    let host = extract_https_host(url)?;
    let path = extract_project_path(url)?;

    let kind = match detect_provider(url) {
        Provider::GitHub => Provider::GitHub,
        Provider::GitLab => Provider::GitLab,
        Provider::Unknown => probe_unknown_host(&host).await?,
    };

    match kind {
        Provider::GitHub => {
            let token = auth_core::CredentialStore::get("github", &host, "default").ok()?;
            let (owner, repo) = path.split_once('/')?;
            Some(ApiClient::GitHub(GitHubClient::new(
                github_api_base(&host),
                owner,
                repo,
                token,
            )))
        }
        _ => {
            let token = auth_core::CredentialStore::get("gitlab", &host, "default").ok()?;
            let base_url = detect_base_url(&host).await?;
            Some(ApiClient::GitLab(GitLabClient::new(base_url, path, token)))
        }
    }
}

/// Live provider probe for a host the remote URL didn't classify. GitLab is
/// tried first so an existing self-hosted GitLab setup pays no extra latency;
/// a GitHub Enterprise host falls through to the `/api/v3` check.
pub async fn probe_unknown_host(host: &str) -> Option<Provider> {
    if detect_base_url(host).await.is_some() {
        return Some(Provider::GitLab);
    }
    let is_ghes = reqwest::Client::new()
        .get(format!("https://{host}/api/v3"))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    is_ghes.then_some(Provider::GitHub)
}

/// Not every self-hosted GitLab terminates TLS (e.g. an internal instance
/// behind a VPN may only serve plain HTTP) -- probe https first, then fall
/// back to http, rather than assuming a scheme.
pub async fn detect_base_url(host: &str) -> Option<String> {
    let https_url = format!("https://{host}");
    if reqwest::Client::new()
        .get(format!("{https_url}/api/v4/version"))
        .send()
        .await
        .is_ok()
    {
        return Some(https_url);
    }

    let http_url = format!("http://{host}");
    if reqwest::Client::new()
        .get(format!("{http_url}/api/v4/version"))
        .send()
        .await
        .is_ok()
    {
        return Some(http_url);
    }

    None
}

pub fn extract_https_host(remote_url: &str) -> Option<String> {
    let rest = remote_url
        .strip_prefix("git@")
        .and_then(|r| r.split(':').next())
        .or_else(|| {
            remote_url
                .strip_prefix("https://")
                .or_else(|| remote_url.strip_prefix("http://"))
                .and_then(|r| r.split('/').next())
        })?;
    Some(rest.to_string())
}

pub fn extract_project_path(remote_url: &str) -> Option<String> {
    let path = if let Some(rest) = remote_url.strip_prefix("git@") {
        rest.split_once(':').map(|(_, p)| p)?
    } else {
        let rest = remote_url
            .strip_prefix("https://")
            .or_else(|| remote_url.strip_prefix("http://"))?;
        rest.split_once('/').map(|(_, p)| p)?
    };
    Some(path.trim_end_matches(".git").to_string())
}

/// Maps a provider client's live "current user role" to the local
/// `OverrideRole`. `None` on any API error -- callers treat an unresolved
/// role as Member (the safe direction).
pub async fn role_from_client(client: &ApiClient) -> Option<OverrideRole> {
    match client.get_current_user_role().await {
        Ok(ProviderRole::Owner) => Some(OverrideRole::Owner),
        Ok(ProviderRole::Member) => Some(OverrideRole::Member),
        Err(_) => None,
    }
}

pub async fn resolve_provider_role(remote_url: &Option<String>) -> Option<OverrideRole> {
    role_from_client(&provider_client_for(remote_url).await?).await
}

pub async fn mr_snapshot(client: &ApiClient, mr_iid: &str) -> Option<MrSnapshot> {
    let mr = client.get_merge_request(mr_iid).await.ok()?;
    Some(MrSnapshot {
        merged: mr.status == MergeStatus::Merged,
        conflicted: matches!(client.check_mergeability(mr_iid).await, Ok(Mergeability::Conflicted)),
    })
}

/// The single recommended next step for the current branch. Serialized shape
/// of `workflow_engine::NextAction`: `primary` is a snake_case action id, or
/// `null` when there is nothing for this user to do now.
#[derive(Debug, Clone, Serialize)]
pub struct NextActionDto {
    pub title: String,
    pub description: String,
    pub primary: Option<String>,
    pub helper: Option<String>,
}

/// Read-only: computes the focus-card next step via
/// `workflow_engine::next_action` from live repo + provider state.
/// Best-effort throughout -- an unreadable repo errors, but an unreachable
/// provider or missing token just yields a safe-looking snapshot.
///
/// This is the single snapshot-assembly path: Tauri's `get_next_action`
/// command and the standalone `vcflow` binary both call this function
/// directly, so there is exactly one workflow implementation.
pub async fn get_next_action(repo_path: &str) -> Result<NextActionDto, String> {
    let repo = Repository::discover(repo_path).map_err(|e| e.to_string())?;
    let branch = current_branch(&repo)?;
    let class = classify_branch(&branch);

    let (dirty, in_progress_op, ahead, behind, diverged) =
        match git_core::read_repository_state(&repo).ok() {
            Some(s) => {
                let up = s.upstream.unwrap_or_default();
                (
                    s.working_tree.is_dirty(),
                    s.in_progress_op.map(|o| o.label().to_string()),
                    up.ahead,
                    up.behind,
                    up.is_diverged(),
                )
            }
            None => (false, None, 0, 0, false),
        };

    let role = resolve_workflow_role(repo_path).await;

    // A work item's MR targets develop; a hotfix's primary MR targets the
    // production branch (main/master).
    let primary_target = if class == BranchClass::Hotfix || class == BranchClass::Release {
        resolve_production(&repo)?
    } else {
        "develop".to_string()
    };
    let tracked_mr = work_item_log()
        .and_then(|log| log.mrs_for_branch(repo_path, &branch).ok())
        .unwrap_or_default()
        .into_iter()
        .find(|m| m.target_branch == primary_target);

    let mr = match &tracked_mr {
        Some(m) => {
            let remote_url = repo
                .find_remote("origin")
                .ok()
                .and_then(|r| r.url().map(str::to_string));
            match provider_client_for(&remote_url).await {
                Some(client) => mr_snapshot(&client, &m.mr_iid).await,
                None => None,
            }
        }
        None => None,
    };

    // The MR-merged fact is learned here (the poll), so retire the WIP item
    // here too -- best-effort, a no-op if the branch was never tracked.
    if mr.map(|m| m.merged).unwrap_or(false) {
        if let Some(log) = wip_item_log() {
            log.set_status(repo_path, &branch, "completed").ok();
        }
    }

    let work_item = match class {
        BranchClass::WorkItem | BranchClass::Hotfix => match &mr {
            Some(m) if m.conflicted => WorkItemState::Conflicted,
            _ if tracked_mr.is_some() => WorkItemState::PushedForReview,
            _ => WorkItemState::Developing,
        },
        _ => WorkItemState::NotStarted,
    };

    let action = compute_next_action(&WorkflowSnapshot {
        role,
        branch: class,
        work_item,
        in_progress_op,
        dirty,
        ahead,
        behind,
        diverged,
        mr,
    });

    Ok(NextActionDto {
        title: action.title,
        description: action.description,
        primary: action.primary.map(|p| {
            match p {
                PrimaryAction::ResolveInWorkingDir => "resolve_in_working_dir",
                PrimaryAction::ResolveMrConflict => "resolve_mr_conflict",
                PrimaryAction::Commit => "commit",
                PrimaryAction::Push => "push",
                PrimaryAction::Finish => "finish",
                PrimaryAction::FinishHotfix => "finish_hotfix",
                PrimaryAction::FinishRelease => "finish_release",
                PrimaryAction::SyncDevelop => "sync_develop",
                PrimaryAction::ReturnToDevelop => "return_to_develop",
                PrimaryAction::UpdateBranch => "update_branch",
                PrimaryAction::StartWorkItem => "start_work_item",
                PrimaryAction::MoveToNewBranch => "move_to_new_branch",
            }
            .to_string()
        }),
        helper: action.helper,
    })
}
#[derive(Debug, Clone, Serialize)]
pub struct RepoStatus {
    pub branch: String,
    pub version: Option<String>,
    pub remote_url: Option<String>,
    pub provider: String,
    pub ssh_ok: bool,
    pub ssh_error: Option<String>,
    /// GitLab API reachable *and* the saved token authenticated (the app
    /// could resolve the current user's role). `false` with a reason in
    /// `gitlab_error` means: no token saved, token/host invalid, GitHub
    /// remote, or the API was unreachable. The frontend must not guess token
    /// validity or MR-create ability -- it reads this.
    pub gitlab_ok: bool,
    pub gitlab_error: Option<String>,
    pub role: String,

    // --- Work Safe read-only state (git_core::RepositoryState projection) ---
    /// Working tree has uncommitted changes (tracked or untracked).
    pub dirty: bool,
    /// Number of changed paths across all buckets.
    pub dirty_count: usize,
    /// `"merge"`/`"rebase"`/`"cherry-pick"`/... when a git operation is
    /// half-finished; `None` when the repo is clean. Any value here is a
    /// hard STOP for the workflow.
    pub in_progress_op: Option<String>,
    /// Current branch commits ahead of / behind `origin/<branch>`. Both 0
    /// when in sync or when no remote-tracking ref exists.
    pub ahead: usize,
    pub behind: usize,
    /// Local and remote both carry unique commits -- never auto-resolved.
    pub diverged: bool,

    /// The repo's production branch: `main` (GitHub default) or `master`
    /// (GitLab default), whichever this repo actually has. `main` when neither
    /// exists yet. The frontend uses this for branch-inspection targets instead
    /// of assuming `master`.
    pub production_branch: String,

    /// Workflow Guard severity for the current branch, derived from
    /// `classify_branch` (never a separate list): `"block"` for
    /// `main`/`master`, `"warn"` for `develop`, `None` for a normal working
    /// branch. UI hint only (badge + guard-card intensity); the actual
    /// commit/push blocking is enforced in the command layer.
    pub branch_guard: Option<String>,
}

/// A `RepoStatus` plus the repo path it describes -- returned by the Hotfix
/// commands so the frontend knows which path is active without re-deriving it.
#[derive(Debug, Clone, Serialize)]
pub struct RepoStatusWithPath {
    pub status: RepoStatus,
    pub repo_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MrStatus {
    pub id: String,
    pub web_url: String,
    pub status: String,
    pub mergeability: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HotfixStatus {
    pub master: Option<MrStatus>,
    pub develop: Option<MrStatus>,
}

/// Walking-skeleton round-trip: repo detection -> provider detection ->
/// SSH validation -> VERSION read -> role resolution -> one event emit.
/// Every step degrades to a best-effort placeholder rather than failing the
/// whole command, since a fresh machine won't have a token stored yet
/// (that flow lands in Phase 5) -- this command's job is to prove the
/// wiring, not to be the final production status check.
pub async fn get_repo_status(repo_path: String) -> Result<RepoStatus, String> {
    build_status(&repo_path).await
}

/// Refresh = `git fetch origin` (remote-tracking refs only, never the working
/// tree) then re-evaluate state. Fetch failure is non-fatal: the status is
/// still rebuilt from whatever refs are already local. Never pulls, never
/// merges -- catching the local branch up is a separate, guarded action.
///
/// Idempotent and safe to call repeatedly: the frontend timer calls this on an
/// interval to keep remote-tracking refs current -- the backend runs no
/// background watcher. Does NOT emit `workflow:state:changed`; a poll is not a
/// mutation, so the frontend re-reads the workflow snapshot on its own
/// schedule. `get_repo_status` is the no-fetch sibling.
pub async fn refresh_repo_status(repo_path: String) -> Result<RepoStatus, String> {
    if let Ok(repo) = Repository::discover(&repo_path) {
        let _ = git_core::fetch_origin(&repo);
    }
    build_status(&repo_path).await
}

// --- Repository preflight (eligibility gate) + Initial Workflow Setup --------

#[derive(Debug, Clone, Serialize)]
pub struct CheckDto {
    pub id: String,
    /// `"pass"` | `"warning"` | `"fail"`. The frontend maps this to its own
    /// colors -- none are encoded here.
    pub status: String,
    pub title: String,
    pub message: String,
    pub blocking: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreflightDto {
    pub checks: Vec<CheckDto>,
    pub eligible: bool,
    pub git_version: Option<String>,
    pub is_repo: bool,
    pub has_commits: bool,
    pub unborn: bool,
    pub current_branch: Option<String>,
    pub remote_url: Option<String>,
    /// `"github"` | `"gitlab"` | `"unsupported"`, or `null` when no origin.
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowInitDto {
    /// The branch HEAD is on when setup finishes -- `develop` (clean repo) or
    /// `feature/initial` (dirty repo, saved work carried onto it).
    pub final_branch: String,
    pub develop_created: bool,
    pub develop_pushed: bool,
    /// The Saved Work label when a dirty tree was stashed, else `null`. Present
    /// even when `restored` is true, so the frontend can name it on a conflict.
    pub saved_work_label: Option<String>,
    /// `true` only when the saved work re-applied cleanly.
    pub restored: bool,
    /// Non-empty only when restoring the saved work hit a merge conflict; the
    /// entry is kept and the working directory holds the markers.
    pub conflicts: Vec<String>,
    pub notes: Vec<String>,
}

/// Label `guard_working_tree` writes for the stash taken at the start of
/// Initial Workflow (`"auto-saved before " + the action string below`). Used
/// to recognise an interrupted-init stash in `get_setup_state`.
const INIT_WORKFLOW_ACTION: &str = "workflow initialization";
const INIT_WORKFLOW_SAVE_LABEL: &str = "auto-saved before workflow initialization";

/// `git --version` first line, or `None` when Git is not on `PATH`.
pub fn detect_git_version() -> Option<String> {
    let out = std::process::Command::new("git").arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Maps `origin`'s URL onto the preflight provider classification, using the
/// same URL-pattern detection + self-hosted live probe the rest of the app uses.
pub async fn classify_preflight_provider(url: &str) -> git_core::PreflightProvider {
    use git_core::PreflightProvider as P;
    match detect_provider(url) {
        Provider::GitHub => P::GitHub,
        Provider::GitLab => P::GitLab,
        Provider::Unknown => match extract_https_host(url) {
            Some(host) => match probe_unknown_host(&host).await {
                Some(Provider::GitHub) => P::GitHub,
                Some(Provider::GitLab) => P::GitLab,
                _ => P::Unsupported,
            },
            None => P::Unsupported,
        },
    }
}

pub fn preflight_dto(pf: git_core::Preflight) -> PreflightDto {
    use git_core::PreflightProvider as P;
    PreflightDto {
        checks: pf
            .checks
            .into_iter()
            .map(|c| CheckDto {
                id: c.id.to_string(),
                status: c.status.as_str().to_string(),
                title: c.title,
                message: c.message,
                blocking: c.blocking,
            })
            .collect(),
        eligible: pf.eligible,
        git_version: pf.git_version,
        is_repo: pf.is_repo,
        has_commits: pf.has_commits,
        unborn: pf.unborn,
        current_branch: pf.current_branch,
        remote_url: pf.remote_url,
        provider: pf.provider.map(|p| {
            match p {
                P::GitHub => "github",
                P::GitLab => "gitlab",
                P::Unsupported => "unsupported",
            }
            .to_string()
        }),
    }
}

/// Read-only strict eligibility gate. Runs the seven mandatory checks and
/// never mutates the repository. A missing `develop` and a dirty tree are NOT
/// failures here -- those are handled by `initialize_workflow`.
pub async fn repository_preflight(repo_path: String) -> Result<PreflightDto, String> {
    let git_version = detect_git_version();
    let repo = Repository::discover(&repo_path).ok();

    let remote_url = repo.as_ref().and_then(|r| {
        r.find_remote("origin").ok().and_then(|rm| rm.url().map(str::to_string))
    });

    let provider = match remote_url.as_deref() {
        Some(url) => Some(classify_preflight_provider(url).await),
        None => None,
    };

    let remote_conn = match (repo.as_ref(), remote_url.is_some()) {
        (Some(r), true) => Some(git_core::validate_remote_connection(r)),
        _ => None,
    };

    let pf = git_core::assemble_preflight(git_version, repo.as_ref(), provider, remote_conn);
    Ok(preflight_dto(pf))
}

/// `develop` reachable from this repo -- local branch or `origin/develop`.
pub fn develop_exists(repo: &Repository) -> bool {
    repo.find_branch("develop", git2::BranchType::Local).is_ok()
        || repo.revparse_single("refs/remotes/origin/develop").is_ok()
}

pub fn init_safe_note(label: &Option<String>) -> String {
    match label {
        Some(l) => format!(
            "\n\nYour changes are safe -- Saved Work \"{l}\". Restore them from the \
             Saved work panel."
        ),
        None => String::new(),
    }
}

/// Creates the local `develop` ref (never a commit on it). Preconditions: it
/// does not already exist locally. Returns `(created, pushed)`.
pub fn ensure_develop(
    repo: &Repository,
    repo_path: &str,
    production: &str,
    saved_label: &Option<String>,
    emit: &impl Fn(&str),
) -> Result<(bool, bool), String> {
    if let Ok(origin_dev) = repo.revparse_single("refs/remotes/origin/develop") {
        let commit = origin_dev.peel_to_commit().map_err(|e| e.to_string())?;
        repo.branch("develop", &commit, false).map_err(|e| e.to_string())?;
        return Ok((true, false));
    }
    let prod_commit = repo
        .find_branch(production, git2::BranchType::Local)
        .map_err(|e| e.to_string())?
        .get()
        .peel_to_commit()
        .map_err(|e| e.to_string())?;
    repo.branch("develop", &prod_commit, false).map_err(|e| e.to_string())?;

    emit("Publishing develop…");
    match run_git(repo_path, "push", "develop", || git_core::push(repo, "develop")) {
        Ok(()) => Ok((true, true)),
        Err(e) => Err(format!(
            "Workflow initialization stopped.\n\nReason:\nCould not publish 'develop': {e}{}",
            init_safe_note(saved_label)
        )),
    }
}

/// The setup gate's mutation, composed from existing `git_core` primitives and
/// the existing Save Work path (`guard_working_tree` -> `SavedWorkLog`). Runs
/// only after preflight passes and only on explicit user confirmation. Never
/// commits, never pushes to `main`/`master`, never runs `git init`. Emits
/// `workflow:init:step` at each step so the Setup Card shows live text.
pub async fn initialize_workflow(
    repo_path: String,
    emit: &impl Fn(&str),
) -> Result<WorkflowInitDto, String> {
    let pf = repository_preflight(repo_path.clone()).await?;
    if !pf.eligible {
        let reason = pf
            .checks
            .iter()
            .find(|c| c.status == "fail")
            .map(|c| c.message.clone())
            .unwrap_or_else(|| "a preflight check failed".to_string());
        return Err(format!(
            "Workflow initialization stopped.\n\nReason:\n{reason}\n\nAction required:\n\
             Fix the issue above, then run the preflight check again."
        ));
    }

    let out = initialize_workflow_inner(&repo_path, emit)?;

    audit_best_effort(&repo_path, "initialize_workflow", &out.final_branch, None);
    Ok(out)
}

/// The Initial Workflow orchestration, sync and `AppHandle`-free so it is
/// directly testable. Composed only from existing `git_core` primitives + the
/// existing Save Work path. `emit` receives the live step text.
pub fn initialize_workflow_inner(
    repo_path: &str,
    emit: &impl Fn(&str),
) -> Result<WorkflowInitDto, String> {
    emit("Checking repository…");
    let mut repo = Repository::discover(repo_path).map_err(|e| e.to_string())?;

    if repo.head().and_then(|h| h.peel_to_commit()).is_err() {
        // §4.5 -- the Setup Card blocks this upstream; defensive only.
        return Err(
            "This repository has no commits yet. Make your first commit, then reopen.".to_string(),
        );
    }

    let production = git_core::production_branch(&repo).ok_or(
        "Workflow initialization stopped.\n\nReason:\nNo 'main' or 'master' branch to base \
         'develop' on.",
    )?;

    if develop_exists(&repo) {
        // Idempotent -- the gate normally routes to Ready/Recover, not here.
        return Ok(WorkflowInitDto {
            final_branch: current_branch(&repo)?,
            develop_created: false,
            develop_pushed: false,
            saved_work_label: None,
            restored: false,
            conflicts: vec![],
            notes: vec!["'develop' already exists -- the workflow is already initialized.".to_string()],
        });
    }

    let dirty = git_core::read_repository_state(&repo)
        .map_err(|e| e.to_string())?
        .working_tree
        .is_dirty();

    // Save Work (records to SavedWorkLog) while still on production, before any
    // switch -- so the row's `branch` is production, not `feature/initial`.
    // Interrupted-init recovery finds it by INIT_WORKFLOW_SAVE_LABEL (§4.3).
    let mut saved_label: Option<String> = None;
    if dirty {
        emit("Saving your work…");
    }
    let saved_id = guard_working_tree(repo_path, &mut repo, INIT_WORKFLOW_ACTION)?;
    if saved_id.is_some() {
        saved_label = Some(INIT_WORKFLOW_SAVE_LABEL.to_string());
    }

    emit("Creating the develop branch…");
    let (develop_created, develop_pushed) =
        ensure_develop(&repo, repo_path, &production, &saved_label, emit)?;

    let mut final_branch = "develop".to_string();
    let mut restored = false;
    let mut conflicts: Vec<String> = vec![];

    if let Some(sid) = saved_id {
        emit("Creating feature/initial…");
        run_git(repo_path, "checkout", "develop", || {
            git_core::checkout_branch(&repo, "develop")
        })
        .map_err(|e| {
            format!(
                "Workflow initialization stopped.\n\nReason:\nCould not switch to 'develop': {e}{}",
                init_safe_note(&saved_label)
            )
        })?;
        run_git(repo_path, "create_branch", "feature/initial", || {
            git_core::create_work_branch(&repo, git_core::BranchKind::Feature, "initial", "develop")
        })
        .map_err(|e| {
            format!(
                "Workflow initialization stopped.\n\nReason:\nCould not create 'feature/initial': {e}{}",
                init_safe_note(&saved_label)
            )
        })?;
        final_branch = "feature/initial".to_string();

        emit("Restoring your work…");
        let stash_oid = saved_work_log()
            .and_then(|l| l.get(sid).ok().flatten())
            .map(|r| r.stash_oid)
            .ok_or("the Saved Work record for this setup could not be read")?;
        match run_git(repo_path, "restore_work", "feature/initial", || {
            git_core::restore_work(&mut repo, &stash_oid)
        }) {
            Ok(()) => {
                if let Some(l) = saved_work_log() {
                    l.set_status(sid, "restored").ok();
                }
                restored = true;
            }
            Err(git_core::SaveWorkError::Conflict { files }) => {
                if let Some(l) = saved_work_log() {
                    l.set_status(sid, "conflict").ok();
                }
                conflicts = files;
            }
            Err(e) => {
                return Err(format!(
                    "Workflow initialization stopped.\n\nReason:\nCould not restore your saved \
                     work: {e}{}",
                    init_safe_note(&saved_label)
                ));
            }
        }
    } else {
        // Clean repo -- land on develop, no feature/initial (§4.6).
        run_git(repo_path, "checkout", "develop", || {
            git_core::checkout_branch(&repo, "develop")
        })
        .map_err(|e| e.to_string())?;
    }

    emit("Done");
    Ok(WorkflowInitDto {
        final_branch,
        develop_created,
        develop_pushed,
        saved_work_label: saved_label,
        restored,
        conflicts,
        notes: vec![],
    })
}

// --- Derived setup phase (the setup gate in front of the normal workflow) ----

#[derive(Debug, Clone, Serialize)]
pub struct SetupStateDto {
    /// `not_a_repo` | `needs_first_commit` | `preflight_failed` |
    /// `needs_initial_workflow` | `recover` | `ready`.
    pub phase: String,
    /// The 7 preflight rows -- always present so the card can render them.
    pub checks: Vec<CheckDto>,
    /// `origin`'s current URL, so the frontend can prefill a "change remote"
    /// form. `null` when there is no origin yet.
    pub remote_url: Option<String>,
    pub needs_git_init: bool,
    /// Working tree dirty (only meaningful for `needs_initial_workflow`).
    pub dirty: bool,
    /// Interrupted-init Saved Work still awaiting restore (`recover` phase).
    pub recover_entries: Vec<SavedWorkDto>,
    pub notes: Vec<String>,
}

/// Read-only. Derives the setup phase each call from real git / provider /
/// SavedWorkLog state -- no persisted phase. Not on the 3s/15s poll timers;
/// the frontend calls it on directory change, mount, after `initialize_workflow`,
/// and on `workflow:state:changed`.
pub async fn get_setup_state(repo_path: String) -> Result<SetupStateDto, String> {
    let pf = repository_preflight(repo_path.clone()).await?;

    let mut dto = SetupStateDto {
        phase: "ready".to_string(),
        checks: pf.checks.clone(),
        remote_url: pf.remote_url.clone(),
        needs_git_init: !pf.is_repo,
        dirty: false,
        recover_entries: vec![],
        notes: vec![],
    };

    if !pf.is_repo {
        dto.phase = "not_a_repo".to_string();
        return Ok(dto);
    }
    if !pf.has_commits {
        dto.phase = "needs_first_commit".to_string();
        return Ok(dto);
    }
    if !pf.eligible {
        dto.phase = "preflight_failed".to_string();
        return Ok(dto);
    }

    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;

    if develop_exists(&repo) {
        let init_related: Vec<SavedWorkDto> = saved_work_log()
            .and_then(|l| l.actionable_entries(&repo_path).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|r| {
                r.label == INIT_WORKFLOW_SAVE_LABEL
                    && (r.status == "saved" || r.status == "conflict")
            })
            .map(saved_work_dto)
            .collect();
        if init_related.is_empty() {
            dto.phase = "ready".to_string();
        } else {
            dto.phase = "recover".to_string();
            dto.recover_entries = init_related;
        }
        return Ok(dto);
    }

    // Preflight ok, >=1 commit, no develop -> Initial Workflow needed.
    dto.phase = "needs_initial_workflow".to_string();
    dto.dirty = git_core::read_repository_state(&repo)
        .map(|s| s.working_tree.is_dirty())
        .unwrap_or(false);
    Ok(dto)
}

/// Saves a provider Personal Access Token into the OS keychain, under the
/// service the repo's `origin` resolves to: `github|<host>|default` for a
/// GitHub remote, `gitlab|<host>|default` otherwise. The token only ever
/// lives in the keychain -- never localStorage, SQLite, config, or logs.
pub async fn save_token(repo_path: String, host: String, token: String) -> Result<(), String> {
    let remote = git_core::read_repo_info(&repo_path)
        .ok()
        .and_then(|i| i.remote_url);
    let url_provider = remote.as_deref().map(detect_provider);
    let service = credential_service_for(url_provider, &host).await;

    auth_core::CredentialStore::set(service, &host, "default", &token)
        .map_err(|e| e.to_string())?;

    // Migration: a GitHub token saved before provider dispatch existed landed
    // under `gitlab|<host>|default`. Now that the correctly-labelled
    // `github|<host>|default` entry is written, drop the stale one -- but
    // ONLY when the URL positively classifies the host as GitHub, so a real
    // GitLab credential on any other host is never touched.
    if service == "github" && url_provider == Some(Provider::GitHub) {
        let _ = auth_core::CredentialStore::delete("gitlab", &host, "default");
    }
    Ok(())
}

/// Rewrites the repo's `origin` remote URL -- e.g. switching an SSH remote
/// (`git@host:owner/repo.git`) to HTTPS (`https://host/owner/repo.git`) so a
/// saved access token can authenticate instead of an SSH key/agent.
pub async fn set_remote_url(repo_path: String, url: String) -> Result<(), String> {
    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    git_core::set_remote_url(&repo, &url).map_err(|e| e.to_string())
}

/// `"github"` or `"gitlab"` for a token save/delete. Uses the URL-pattern
/// provider when it's decisive; for a self-hosted host the URL can't
/// classify, probes the host live (GitLab first, then GitHub Enterprise) and
/// defaults to `"gitlab"` if neither answers -- the historical behavior.
pub async fn credential_service(repo_path: &str, host: &str) -> &'static str {
    let remote = git_core::read_repo_info(repo_path)
        .ok()
        .and_then(|i| i.remote_url);
    credential_service_for(remote.as_deref().map(detect_provider), host).await
}

pub async fn credential_service_for(url_provider: Option<Provider>, host: &str) -> &'static str {
    match url_provider {
        Some(Provider::GitHub) => "github",
        Some(Provider::GitLab) => "gitlab",
        _ => match probe_unknown_host(host).await {
            Some(Provider::GitHub) => "github",
            _ => "gitlab",
        },
    }
}

/// Member-only: creates `<kind>/<slug>` off `develop` and checks it out.
/// `kind` is one of "feature" / "bug" / "chore".
pub async fn create_work_item(
    repo_path: String,
    kind: String,
    slug: String,
) -> Result<RepoStatus, String> {
    let role = resolve_workflow_role(&repo_path).await;
    workflow_engine::transition(WorkItemState::NotStarted, AllowedAction::StartDevelopment, role)
        .map_err(|e| e.to_string())?;

    let branch_kind = parse_branch_kind(&kind)?;
    let mut repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    guard_working_tree(&repo_path, &mut repo, &format!("creating {kind}/{slug}"))?;

    // Bring local `develop` current before branching off it. Fetch is
    // best-effort; a real divergence is a hard STOP (Work Safe -- never
    // reconciled for the user).
    let _ = git_core::fetch_origin(&repo);
    run_git(&repo_path, "fast_forward", "develop", || {
        git_core::fast_forward_from_origin(&repo, "develop")
    })
    .map_err(|e| e.to_string())?;

    let branch_name = run_git(&repo_path, "create_branch", &format!("{kind}/{slug} off develop"), || {
        git_core::create_work_branch(&repo, branch_kind, &slug, "develop")
    })
    .map_err(|e| e.to_string())?;

    if let Some(log) = wip_item_log() {
        log.start(&repo_path, &branch_name, &kind).ok();
    }
    audit_best_effort(&repo_path, "create_work_item", &branch_name, None);
    build_status(&repo_path).await
}

/// Outcome of `move_changes_to_new_branch`: the fresh status on the new branch
/// plus what happened to the changes carried over.
#[derive(Debug, Clone, Serialize)]
pub struct MoveChangesOutcome {
    pub status: RepoStatus,
    pub new_branch: String,
    /// `"restored"` (re-applied cleanly), `"conflict"` (re-applied with
    /// collisions -- markers in the working dir on the new branch, Saved Work
    /// entry kept), or `"error"` (could not auto-apply -- entry untouched, use
    /// Resume in the Saved work panel). Never `"none"`: the guard only runs on
    /// a dirty tree.
    pub restore_outcome: String,
    pub conflicting_files: Vec<String>,
}

/// Re-apply the Saved Work row `id` onto the current (clean) branch. Mirrors
/// the auto-restore in `continue_work`; never resets or discards. Returns
/// `("restored" | "conflict" | "error", files)`.
pub fn restore_saved_by_id(repo_path: &str, repo: &mut Repository, id: i64) -> (String, Vec<String>) {
    let Some(log) = saved_work_log() else {
        return ("error".to_string(), vec![]);
    };
    let Ok(Some(rec)) = log.get(id) else {
        return ("error".to_string(), vec![]);
    };
    match run_git(repo_path, "restore_work", &rec.branch, || {
        git_core::restore_work(repo, &rec.stash_oid)
    }) {
        Ok(()) => {
            log.set_status(id, "restored").ok();
            audit_best_effort(repo_path, "resume_work", &rec.branch, None);
            ("restored".to_string(), vec![])
        }
        Err(git_core::SaveWorkError::Conflict { files }) => {
            log.set_status(id, "conflict").ok();
            audit_best_effort(repo_path, "resume_work_conflict", &rec.branch, None);
            ("conflict".to_string(), files)
        }
        Err(_) => ("error".to_string(), vec![]),
    }
}

/// Workflow Guard recovery: on a protected branch (`main`/`master`/`develop`)
/// with a dirty tree, stash the changes (existing Save Work path), create
/// `<kind>/<slug>` off `develop`, check it out, and re-apply the changes there.
/// Reuses `guard_working_tree`, `create_work_branch` and the Saved Work restore
/// -- no new stash/restore logic. Nothing is ever discarded.
pub async fn move_changes_to_new_branch(
    repo_path: String,
    kind: String,
    slug: String,
) -> Result<MoveChangesOutcome, String> {
    let (new_branch, restore_outcome, conflicting_files) =
        move_changes_to_new_branch_inner(&repo_path, &kind, &slug)?;
    audit_best_effort(&repo_path, "move_changes_to_new_branch", &new_branch, None);
    let status = build_status(&repo_path).await?;
    Ok(MoveChangesOutcome { status, new_branch, restore_outcome, conflicting_files })
}

/// `AppHandle`-free core so it is directly testable. Returns
/// `(new_branch, restore_outcome, conflicting_files)`.
pub fn move_changes_to_new_branch_inner(
    repo_path: &str,
    kind: &str,
    slug: &str,
) -> Result<(String, String, Vec<String>), String> {
    let branch_kind = parse_branch_kind(kind)?;
    let mut repo = Repository::discover(repo_path).map_err(|e| e.to_string())?;

    let current = current_branch(&repo)?;
    if !branch_is_protected(&current) {
        return Err(format!(
            "Move Changes to New Branch only applies on a protected branch (main/master/develop), \
             not '{current}'."
        ));
    }
    if !git_core::read_repository_state(&repo)
        .map_err(|e| e.to_string())?
        .working_tree
        .is_dirty()
    {
        return Err("Nothing to move -- the working tree is clean.".to_string());
    }
    // New work always bases on `develop`; require it before we stash anything.
    repo.find_branch("develop", git2::BranchType::Local)
        .map_err(|_| "No local 'develop' branch to base the new work on.".to_string())?;

    // 1. Stash the dirty tree into Saved Work (recorded against `current`).
    let saved_id = guard_working_tree(repo_path, &mut repo, &format!("moving changes off {current}"))?
        .ok_or("working tree reported dirty but nothing was saved")?;

    // 2. Bring `develop` current, then branch off it. Fetch + fast-forward are
    //    best-effort and skipped when `develop` was never pushed; a real
    //    divergence is still a hard STOP (Work Safe). The Saved Work entry
    //    stays resumable if this fails.
    let _ = git_core::fetch_origin(&repo);
    if repo.revparse_single("refs/remotes/origin/develop").is_ok() {
        run_git(repo_path, "fast_forward", "develop", || {
            git_core::fast_forward_from_origin(&repo, "develop")
        })
        .map_err(|e| e.to_string())?;
    }

    let new_branch = run_git(
        repo_path,
        "create_branch",
        &format!("{kind}/{slug} off develop"),
        || git_core::create_work_branch(&repo, branch_kind, slug, "develop"),
    )
    .map_err(|e| e.to_string())?;

    if let Some(log) = wip_item_log() {
        log.start(repo_path, &new_branch, kind).ok();
    }

    // 3. Re-apply the saved changes onto the new branch.
    let (restore_outcome, conflicting_files) = restore_saved_by_id(repo_path, &mut repo, saved_id);

    Ok((new_branch, restore_outcome, conflicting_files))
}

/// Stages and commits everything currently in the working tree.
pub async fn commit_work_item(
    repo_path: String,
    message: String,
) -> Result<RepoStatus, String> {
    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    reject_protected_branch(&repo, "commit")?;
    run_git(&repo_path, "commit", &message, || git_core::commit_all(&repo, &message))
        .map_err(|e| e.to_string())?;

    audit_best_effort(&repo_path, "commit_work_item", "", None);
    build_status(&repo_path).await
}

/// Pushes the current branch to `origin`. Never pushes anything but the
/// current `feature/*`/`bug/*`/`chore/*` branch.
pub async fn push_work_item(repo_path: String) -> Result<RepoStatus, String> {
    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    reject_protected_branch(&repo, "push")?;
    let branch = current_branch(&repo)?;
    run_git(&repo_path, "push", &branch, || git_core::push(&repo, &branch)).map_err(|e| e.to_string())?;

    audit_best_effort(&repo_path, "push_work_item", &branch, None);

    // This runs only for follow-up commits onto an already-open MR (the Push
    // next-action). The MR now has everything -- park the user back on develop.
    // Best-effort -- see finish_work_item.
    let _ = run_git(&repo_path, "checkout", "develop", || {
        git_core::checkout_branch(&repo, "develop")
    });

    build_status(&repo_path).await
}

/// Member-only: pushes the current branch (in case of uncommitted pushes)
/// and opens the MR into `develop`. This is the only step that talks to the
/// provider's write API for Feature/Bug/Chore.
pub async fn finish_work_item(
    repo_path: String,
    title: String,
) -> Result<RepoStatus, String> {
    let role = resolve_workflow_role(&repo_path).await;
    workflow_engine::transition(WorkItemState::Developing, AllowedAction::Finish, role)
        .map_err(|e| e.to_string())?;

    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let branch = current_branch(&repo)?;
    run_git(&repo_path, "push", &branch, || git_core::push(&repo, &branch)).map_err(|e| e.to_string())?;

    let remote_url = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().map(str::to_string));
    let client = provider_client_for(&remote_url)
        .await
        .ok_or("could not reach the provider API for this remote (check token/host)")?;

    let mr = client
        .create_merge_request(&branch, "develop", &title)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(log) = work_item_log() {
        log.add_mr(&repo_path, &branch, "develop", &mr.id).ok();
    }
    if let Some(log) = wip_item_log() {
        log.set_status(&repo_path, &branch, "waiting").ok();
    }
    audit_best_effort(&repo_path, "finish_work_item", &branch, Some(&mr.id));

    // MR is open; nothing more to do on the work branch. Park the user back on
    // develop. Best-effort: the tree is clean here (Finish is only offered when
    // committed) and the checkout is safe, but a missing local develop must not
    // fail a finish whose MR already succeeded.
    let _ = run_git(&repo_path, "checkout", "develop", || {
        git_core::checkout_branch(&repo, "develop")
    });

    build_status(&repo_path).await
}

/// Read-only: polls the provider for the MR opened on the current branch,
/// if any. No role gate -- viewing status is never Owner-only.
pub async fn get_mr_status(repo_path: String) -> Result<Option<MrStatus>, String> {
    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let branch = current_branch(&repo)?;

    let Some(mr_ref) = work_item_log()
        .and_then(|log| log.mrs_for_branch(&repo_path, &branch).ok())
        .unwrap_or_default()
        .into_iter()
        .find(|m| m.target_branch == "develop")
    else {
        return Ok(None);
    };

    let remote_url = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().map(str::to_string));
    let Some(client) = provider_client_for(&remote_url).await else {
        return Ok(None);
    };

    fetch_mr_status(&client, &mr_ref.mr_iid).await.map(Some).map_err(|e| e.to_string())
}

/// Hotfix branches are not role-gated to create/finish -- only Conflict
/// Resolution and Release triggering are Owner-only per spec.
pub async fn create_hotfix(
    repo_path: String,
    slug: String,
) -> Result<RepoStatusWithPath, String> {
    let mut repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let production = resolve_production(&repo)?;
    guard_working_tree(&repo_path, &mut repo, &format!("creating hotfix/{slug}"))?;

    // Bring the local production branch current before branching off it. Fetch
    // is best-effort; a real divergence is a hard STOP (Work Safe -- never
    // reconciled for the user).
    let _ = git_core::fetch_origin(&repo);
    run_git(&repo_path, "fast_forward", &production, || {
        git_core::fast_forward_from_origin(&repo, &production)
    })
    .map_err(|e| e.to_string())?;

    let branch_name = run_git(&repo_path, "create_hotfix_branch", &slug, || {
        git_core::create_hotfix_branch(&repo, &slug, &production)
    })
    .map_err(|e| e.to_string())?;

    if let Some(log) = wip_item_log() {
        log.start(&repo_path, &branch_name, "hotfix").ok();
    }
    audit_best_effort(&repo_path, "create_hotfix", &branch_name, None);
    let status = build_status(&repo_path).await?;
    Ok(RepoStatusWithPath { status, repo_path })
}

/// Opens the `hotfix/* -> <production>` MR, then a separate
/// `<production> -> develop` sync MR so the hotfix lands back on develop.
/// `<production>` is the repo's `main`/`master`. Each MR is tracked and
/// merged independently -- no automatic merge, no worktree.
pub async fn finish_hotfix(
    repo_path: String,
    title: String,
) -> Result<RepoStatusWithPath, String> {
    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let production = resolve_production(&repo)?;
    let branch = current_branch(&repo)?;
    run_git(&repo_path, "push", &branch, || git_core::push(&repo, &branch)).map_err(|e| e.to_string())?;

    let remote_url = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().map(str::to_string));
    let client = provider_client_for(&remote_url)
        .await
        .ok_or("could not reach the provider API for this remote (check token/host)")?;

    let prod_mr = client
        .create_merge_request(&branch, &production, &title)
        .await
        .map_err(|e| e.to_string())?;
    let sync_mr = client
        .create_merge_request(&production, "develop", &format!("sync: {title}"))
        .await
        .map_err(|e| e.to_string())?;

    if let Some(log) = work_item_log() {
        log.add_mr(&repo_path, &branch, &production, &prod_mr.id).ok();
        log.add_mr(&repo_path, &branch, "develop", &sync_mr.id).ok();
    }
    if let Some(log) = wip_item_log() {
        log.set_status(&repo_path, &branch, "waiting").ok();
    }
    audit_best_effort(&repo_path, "finish_hotfix", &branch, Some(&prod_mr.id));
    audit_best_effort(&repo_path, "finish_hotfix", &branch, Some(&sync_mr.id));

    // Both MRs are open; park the user back on develop. Best-effort -- see
    // finish_work_item.
    let _ = run_git(&repo_path, "checkout", "develop", || {
        git_core::checkout_branch(&repo, "develop")
    });

    let status = build_status(&repo_path).await?;
    Ok(RepoStatusWithPath { status, repo_path })
}

/// Read-only: polls both MRs of the current hotfix branch, if any.
pub async fn get_hotfix_status(repo_path: String) -> Result<Option<HotfixStatus>, String> {
    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let branch = current_branch(&repo)?;

    let mrs = work_item_log()
        .and_then(|log| log.mrs_for_branch(&repo_path, &branch).ok())
        .unwrap_or_default();
    if mrs.is_empty() {
        return Ok(None);
    }

    let remote_url = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().map(str::to_string));
    let Some(client) = provider_client_for(&remote_url).await else {
        return Ok(None);
    };

    let production = resolve_production(&repo)?;
    let mut master = None;
    let mut develop = None;
    for mr_ref in mrs {
        let status = fetch_mr_status(&client, &mr_ref.mr_iid).await.map_err(|e| e.to_string())?;
        if mr_ref.target_branch == "develop" {
            develop = Some(status);
        } else if mr_ref.target_branch == production {
            master = Some(status);
        }
    }

    Ok(Some(HotfixStatus { master, develop }))
}

pub async fn fetch_mr_status(client: &ApiClient, mr_iid: &str) -> Result<MrStatus, provider_core::ProviderError> {
    let mr = client.get_merge_request(mr_iid).await?;
    let mergeability = client.check_mergeability(mr_iid).await.unwrap_or(Mergeability::Unknown);
    Ok(MrStatus {
        id: mr.id,
        web_url: mr.web_url,
        status: format!("{:?}", mr.status),
        mergeability: format!("{mergeability:?}"),
    })
}

/// Workflow Guard severity for `name`, from `classify_branch` (never a
/// separate list): `"block"` for production, `"warn"` for develop, `None`
/// otherwise.
pub fn branch_guard(name: &str) -> Option<&'static str> {
    match classify_branch(name) {
        BranchClass::Master => Some("block"),
        BranchClass::Develop => Some("warn"),
        _ => None,
    }
}

/// A protected development branch the Workflow Guard covers -- no direct
/// commits or pushes. Reuses `classify_branch`; never a separate branch list.
pub fn branch_is_protected(name: &str) -> bool {
    branch_guard(name).is_some()
}

/// Workflow Guard enforcement: refuse a mutating git op on a protected branch.
/// `op` is a verb like `"commit"` / `"push"`. Returns `Ok(())` on any normal
/// working branch. This is the real block -- the Next Action card only explains.
pub fn reject_protected_branch(repo: &Repository, op: &str) -> Result<(), String> {
    let branch = current_branch(repo)?;
    if branch_is_protected(&branch) {
        return Err(format!(
            "Workflow Guard: refusing to {op} on the protected branch '{branch}'. Direct \
             development on protected branches is not allowed. Use \"Move Changes to New Branch\" \
             in the Next Action card to move your changes onto a feature branch, then {op} there."
        ));
    }
    Ok(())
}

/// Build `RepoStatus` without broadcasting. Use this on read-only paths
/// (`get_repo_status`, `refresh_repo_status`) so a periodic poll never emits
/// `workflow:state:changed` -- that event means "a mutation happened, re-read
/// the workflow snapshot", and a poll is not a mutation.
pub async fn build_status(repo_path: &str) -> Result<RepoStatus, String> {
    let info = git_core::read_repo_info(repo_path).map_err(|e| e.to_string())?;

    let mut provider = info
        .remote_url
        .as_deref()
        .map(detect_provider)
        .unwrap_or(Provider::Unknown);

    let (ssh_ok, ssh_error) = match Repository::discover(repo_path) {
        Ok(repo) => match git_core::validate_remote_connection(&repo) {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e.to_string())),
        },
        Err(e) => (false, Some(e.to_string())),
    };

    let user = whoami_user();
    let repository_key = info.remote_url.clone().unwrap_or_else(|| repo_path.to_string());

    // Resolve the provider API client once and learn the current user's role
    // from it. A hostname-Unknown remote (self-hosted) is probed live inside
    // `provider_client_for` -- per provider_core::detect's contract, only an
    // API call can confirm a self-hosted host's provider, a hostname guess
    // can't. The resolved variant then becomes the displayed `provider`.
    let provider_role = if ssh_ok {
        match provider_client_for(&info.remote_url).await {
            Some(client) => {
                let role = role_from_client(&client).await;
                if role.is_some() {
                    provider = client.provider();
                }
                role
            }
            None => None,
        }
    } else {
        None
    };

    let config = auth_core::default_config_path()
        .ok()
        .and_then(|p| auth_core::load_config(p).ok())
        .unwrap_or_default();

    // `gitlab_ok` / `gitlab_error` are the provider-neutral "the API works"
    // signal the frontend reads (field names kept for compatibility). Set the
    // same way for GitLab and GitHub.
    let (gitlab_ok, gitlab_error) = if !ssh_ok {
        (false, ssh_error.clone())
    } else if provider_role.is_some() {
        (true, None)
    } else {
        (
            false,
            Some(
                "could not authenticate to the provider API (no token saved, or the token/host is \
                 invalid, or the API was unreachable)"
                    .to_string(),
            ),
        )
    };

    let role = resolve_role_best_effort(&user, &repository_key, provider_role, &config);

    // Work Safe read-only state -- best-effort; a repo we can't inspect
    // (detached HEAD, unreadable) just reports the safe-looking default.
    let repo_handle = Repository::discover(repo_path).ok();
    let production_branch = repo_handle
        .as_ref()
        .and_then(git_core::production_branch)
        .unwrap_or_else(|| "main".to_string());
    let ws = repo_handle
        .as_ref()
        .and_then(|repo| git_core::read_repository_state(repo).ok());
    let (dirty, dirty_count, in_progress_op, ahead, behind, diverged) = match ws {
        Some(s) => {
            let up = s.upstream.unwrap_or_default();
            (
                s.working_tree.is_dirty(),
                s.working_tree.total_count(),
                s.in_progress_op.map(|o| o.label().to_string()),
                up.ahead,
                up.behind,
                up.is_diverged(),
            )
        }
        None => (false, 0, None, 0, 0, false),
    };

    let branch_guard = branch_guard(&info.current_branch).map(str::to_string);
    let status = RepoStatus {
        branch: info.current_branch,
        version: info.version,
        remote_url: info.remote_url,
        provider: format!("{provider:?}"),
        ssh_ok,
        ssh_error,
        gitlab_ok,
        gitlab_error,
        role,
        dirty,
        dirty_count,
        in_progress_op,
        ahead,
        behind,
        diverged,
        production_branch,
        branch_guard,
    };

    Ok(status)
}


/// Current branch HEAD commit OID as a hex string, or `""` if HEAD can't be
/// read (unborn branch, detached, unreadable) -- callers store it as a
/// best-effort base marker, never a required field.
pub fn head_commit_oid(repo: &Repository) -> String {
    repo.head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.id().to_string())
        .unwrap_or_default()
}

pub fn parse_branch_kind(kind: &str) -> Result<git_core::BranchKind, String> {
    match kind {
        "feature" => Ok(git_core::BranchKind::Feature),
        "bug" => Ok(git_core::BranchKind::Bug),
        "chore" => Ok(git_core::BranchKind::Chore),
        other => Err(format!("unknown work item kind: {other}")),
    }
}

/// Wraps a mutating git op so its real execution -- duration, success/failure,
/// masked error -- lands in `command_log` (shares `audit.sqlite3`).
/// Best-effort: if the log can't be opened the op still runs.
pub fn run_git<T, E: std::fmt::Display>(
    repo: &str,
    op: &str,
    args: &str,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    match AuditLog::default_path().ok().and_then(|p| CommandLog::open(p).ok()) {
        Some(log) => auth_core::record_op(&log, repo, op, args, f),
        None => f(),
    }
}

pub fn saved_work_log() -> Option<SavedWorkLog> {
    AuditLog::default_path().ok().and_then(|p| SavedWorkLog::open(p).ok())
}

pub fn conflict_log() -> Option<ConflictLog> {
    AuditLog::default_path().ok().and_then(|p| ConflictLog::open(p).ok())
}

/// Work Safe gate. Every mutating workflow step calls this before touching
/// the working tree:
/// - a git operation half-finished (merge/rebase/...) -> hard STOP (error);
///   Work Safe never auto-resolves.
/// - tree dirty -> Save Work (stash incl. untracked). Success -> the step
///   continues on a clean tree; failure -> hard STOP.
/// - tree clean -> nothing to do.
///
/// Returns the `saved_work` row id when it stashed, so the caller can tell
/// the user what to resume afterwards.
pub fn guard_working_tree(
    repo_path: &str,
    repo: &mut Repository,
    action: &str,
) -> Result<Option<i64>, String> {
    let state = git_core::read_repository_state(repo).map_err(|e| e.to_string())?;
    if let Some(op) = state.in_progress_op {
        return Err(format!(
            "a git {} is in progress -- finish or abort it in the working directory, then retry",
            op.label()
        ));
    }
    if !state.working_tree.is_dirty() {
        return Ok(None);
    }

    let branch = state.current_branch;
    let original_commit = head_commit_oid(repo);
    let label = format!("auto-saved before {action}");
    let saved = run_git(repo_path, "save_work", &branch, || git_core::save_work(repo, &label))
        .map_err(|e| e.to_string())?
        .ok_or("working tree reported dirty but nothing was saved")?;

    let id = saved_work_log()
        .and_then(|log| {
            log.record(repo_path, &branch, &saved.stash_oid, &label, &original_commit).ok()
        })
        .unwrap_or(0);
    audit_best_effort(repo_path, "save_work", &branch, None);
    Ok(Some(id))
}

#[derive(Debug, Clone, Serialize)]
pub struct SavedWorkDto {
    pub id: i64,
    pub repo: String,
    /// The branch the work was saved from.
    pub original_branch: String,
    /// That branch's HEAD OID at save time (`""` if it could not be read).
    pub original_commit: String,
    pub label: String,
    pub created_at: String,
    /// `"saved"` | `"conflict"` | `"restored"` | `"discarded"`.
    pub status: String,
}

pub fn saved_work_dto(r: SavedWorkRecord) -> SavedWorkDto {
    SavedWorkDto {
        id: r.id,
        repo: r.repository,
        original_branch: r.branch,
        original_commit: r.original_commit,
        label: r.label,
        created_at: r.timestamp,
        status: r.status,
    }
}

/// Outcome of `resume_work`. `outcome` is `"restored"` (re-applied and
/// dropped) or `"conflict"` (re-applying collided; the entry is **kept**,
/// `conflicting_files` carry markers in the working directory, and nothing
/// was reset or discarded).
#[derive(Debug, Clone, Serialize)]
pub struct ResumeOutcome {
    pub outcome: String,
    pub conflicting_files: Vec<String>,
    pub status: RepoStatus,
}

/// Work Safe: manually stash the current working tree (tracked + untracked)
/// as a resumable Saved Work entry. Errors if the tree is already clean.
pub async fn save_work(repo_path: String) -> Result<RepoStatus, String> {
    let mut repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let branch = current_branch(&repo)?;
    let original_commit = head_commit_oid(&repo);
    let label = format!("manual save on {branch}");
    let saved = run_git(&repo_path, "save_work", &branch, || git_core::save_work(&mut repo, &label))
        .map_err(|e| e.to_string())?
        .ok_or("nothing to save -- working tree is clean")?;
    if let Some(log) = saved_work_log() {
        log.record(&repo_path, &branch, &saved.stash_oid, &label, &original_commit).ok();
    }
    audit_best_effort(&repo_path, "save_work", &branch, None);
    build_status(&repo_path).await
}

/// Work Safe: Saved Work entries the frontend should surface for this repo,
/// newest first -- `saved` (resumable) and `conflict` (a resume collided and
/// the entry was preserved).
pub fn list_saved_work(repo_path: String) -> Result<Vec<SavedWorkDto>, String> {
    let Some(log) = saved_work_log() else {
        return Ok(vec![]);
    };
    Ok(log
        .actionable_entries(&repo_path)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(saved_work_dto)
        .collect())
}

/// Work Safe: re-apply a Saved Work entry. Refuses when the tree is currently
/// dirty. On a clean apply the entry is dropped and `outcome` is `"restored"`.
/// On a collision the entry is **kept** (status -> `conflict`), the working
/// directory holds the conflict markers, and `outcome` is `"conflict"` -- no
/// reset, no discard, no automatic resolution.
pub async fn resume_work(repo_path: String, id: i64) -> Result<ResumeOutcome, String> {
    let log = saved_work_log().ok_or("saved-work log unavailable")?;
    let rec = log.get(id).map_err(|e| e.to_string())?.ok_or("no such saved work")?;
    if rec.status != "saved" {
        return Err(format!("saved work {id} is {} -- only a 'saved' entry can be resumed", rec.status));
    }
    let mut repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;

    match run_git(&repo_path, "restore_work", &rec.branch, || {
        git_core::restore_work(&mut repo, &rec.stash_oid)
    }) {
        Ok(()) => {
            log.set_status(id, "restored").ok();
            audit_best_effort(&repo_path, "resume_work", &rec.branch, None);
            let status = build_status(&repo_path).await?;
            Ok(ResumeOutcome { outcome: "restored".into(), conflicting_files: vec![], status })
        }
        Err(git_core::SaveWorkError::Conflict { files }) => {
            log.set_status(id, "conflict").ok();
            audit_best_effort(&repo_path, "resume_work_conflict", &rec.branch, None);
            let status = build_status(&repo_path).await?;
            Ok(ResumeOutcome { outcome: "conflict".into(), conflicting_files: files, status })
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Work Safe: permanently drop a Saved Work entry without applying it.
/// Explicit user action -- Work Safe never discards automatically. Allowed
/// for a `saved` entry or a `conflict` one (whose stash git kept after a
/// failed resume).
pub async fn discard_work(repo_path: String, id: i64) -> Result<RepoStatus, String> {
    let log = saved_work_log().ok_or("saved-work log unavailable")?;
    let rec = log.get(id).map_err(|e| e.to_string())?.ok_or("no such saved work")?;
    if rec.status != "saved" && rec.status != "conflict" {
        return Err(format!("saved work {id} is already {}", rec.status));
    }
    let mut repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    run_git(&repo_path, "discard_work", &rec.branch, || {
        git_core::discard_work(&mut repo, &rec.stash_oid)
    })
    .map_err(|e| e.to_string())?;
    log.set_status(id, "discarded").ok();
    audit_best_effort(&repo_path, "discard_work", &rec.branch, None);
    build_status(&repo_path).await
}

// --- Work-in-progress items (branch continuation) ----------------------

#[derive(Debug, Clone, Serialize)]
pub struct WipItemDto {
    pub id: i64,
    pub branch: String,
    /// `feature` | `bug` | `chore` | `hotfix`.
    pub work_type: String,
    /// `active` | `waiting`.
    pub status: String,
    pub created_at: String,
    /// True when this is the branch currently checked out.
    pub is_current: bool,
    /// True when a resumable Saved Work entry exists for this branch.
    pub has_saved_work: bool,
}

/// The whole WIP picture the frontend renders -- it does no partitioning of
/// its own. `develop`/`master` never appear here.
#[derive(Debug, Clone, Serialize)]
pub struct WorkList {
    /// The tracked item for the branch currently checked out, if any.
    pub current: Option<WipItemDto>,
    /// `active` items on other branches -- unfinished work to come back to.
    pub other: Vec<WipItemDto>,
    /// `waiting` items -- handed off (MR open), nothing to do now.
    pub waiting: Vec<WipItemDto>,
}

/// Outcome of `continue_work`: the fresh status after checkout plus the Saved
/// Work entry (if any) the user can now restore. The saved-work entry is
/// **not** restored automatically -- that stays an explicit user action.
#[derive(Debug, Clone, Serialize)]
pub struct ContinueOutcome {
    pub status: RepoStatus,
    /// The branch's Saved Work entry as it was *before* the auto-restore
    /// (so the frontend can name it). `None` when the branch had none.
    pub saved_work: Option<SavedWorkDto>,
    /// What happened to that entry: `"restored"` (applied + dropped),
    /// `"conflict"` (applied with collisions -- markers left in the tree, entry
    /// kept), `"none"` (nothing to restore), or `"error"` (apply failed -- the
    /// entry is untouched, use Resume in the Saved work panel).
    pub restore_outcome: String,
    pub conflicting_files: Vec<String>,
}

/// The `wip_items.work_type` for a branch, or `None` if the branch is not a
/// work item (develop/master/detached/other). For `feature/`,`bug/`,`chore/`
/// this is the prefix itself; for hotfix it is `"hotfix"`.
pub fn branch_work_type(branch: &str) -> Option<String> {
    match classify_branch(branch) {
        BranchClass::WorkItem | BranchClass::Hotfix | BranchClass::Release => {
            branch.split('/').next().map(str::to_string)
        }
        _ => None,
    }
}

pub async fn build_work_list(repo_path: &str) -> WorkList {
    let repo = Repository::discover(repo_path).ok();
    let current_branch = repo.as_ref().and_then(|r| current_branch(r).ok());

    // Self-heal: branches that predate wip_items (never went through
    // `create_work_item` in-app) still surface. `backfill` only inserts when
    // absent, so `dropped` / `completed` rows are never resurrected.
    if let Some(log) = wip_item_log() {
        if let Some(b) = current_branch.as_deref() {
            if let Some(wt) = branch_work_type(b) {
                log.backfill(repo_path, b, &wt).ok();
            }
        }
        if let Some(sw) = saved_work_log() {
            for e in sw.open_entries(repo_path).unwrap_or_default() {
                if let Some(wt) = branch_work_type(&e.branch) {
                    log.backfill(repo_path, &e.branch, &wt).ok();
                }
            }
        }
    }

    // Reconcile handed-off items: an MR merged on the web sends us no signal,
    // so poll each `waiting` item's MR and retire it when merged. Best-effort
    // -- offline or no token just leaves it under "Waiting Work".
    let remote_url = repo.as_ref().and_then(|r| {
        r.find_remote("origin").ok().and_then(|rm| rm.url().map(str::to_string))
    });
    let production = repo
        .as_ref()
        .and_then(git_core::production_branch)
        .unwrap_or_else(|| "master".to_string());
    if let Some(client) = provider_client_for(&remote_url).await {
        let waiting: Vec<(String, String)> = wip_item_log()
            .and_then(|l| l.actionable(repo_path).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|i| i.status == "waiting")
            // Release lifecycle (two MRs, supersede) is owned by
            // `get_release_status`, not this single-MR reconcile.
            .filter(|i| classify_branch(&i.branch) != BranchClass::Release)
            .filter_map(|i| {
                let target = if classify_branch(&i.branch) == BranchClass::Hotfix {
                    production.as_str()
                } else {
                    "develop"
                };
                let iid = work_item_log()?
                    .mrs_for_branch(repo_path, &i.branch)
                    .ok()?
                    .into_iter()
                    .find(|m| m.target_branch == target)?
                    .mr_iid;
                Some((i.branch, iid))
            })
            .collect();
        for (branch, iid) in waiting {
            if mr_snapshot(&client, &iid).await.map(|m| m.merged).unwrap_or(false) {
                if let Some(log) = wip_item_log() {
                    log.set_status(repo_path, &branch, "completed").ok();
                }
            }
        }
    }

    let items = wip_item_log()
        .and_then(|l| l.actionable(repo_path).ok())
        .unwrap_or_default();

    let sw = saved_work_log();
    let to_dto = |it: auth_core::WipItem| WipItemDto {
        has_saved_work: sw
            .as_ref()
            .and_then(|l| l.saved_for_branch(repo_path, &it.branch).ok().flatten())
            .is_some(),
        is_current: current_branch.as_deref() == Some(it.branch.as_str()),
        id: it.id,
        branch: it.branch,
        work_type: it.work_type,
        status: it.status,
        created_at: it.created_at,
    };

    let mut current = None;
    let mut other = Vec::new();
    let mut waiting = Vec::new();
    for dto in items.into_iter().map(to_dto) {
        if dto.is_current {
            current = Some(dto);
        } else if dto.status == "waiting" {
            waiting.push(dto);
        } else {
            other.push(dto);
        }
    }
    WorkList { current, other, waiting }
}

/// Read-only: the user's unfinished / handed-off work in this repo.
pub async fn list_work_items(repo_path: String) -> Result<WorkList, String> {
    Ok(build_work_list(&repo_path).await)
}

/// Come back to a tracked branch: Work Safe guard (stash anything dirty on the
/// branch being left) -> checkout -> auto-apply that branch's Saved Work onto
/// the now-clean tree -> emit `workflow:state:changed`. A restore collision is
/// non-fatal: the stash is kept and the conflict markers are reported.
pub async fn continue_work(
    repo_path: String,
    work_item_id: i64,
) -> Result<ContinueOutcome, String> {
    let log = wip_item_log().ok_or("work-item log unavailable")?;
    let item = log.get(work_item_id).map_err(|e| e.to_string())?.ok_or("no such work item")?;
    if item.repository != repo_path {
        return Err("that work item belongs to a different repository".to_string());
    }
    if item.status != "active" && item.status != "waiting" {
        return Err(format!("work item is {} -- nothing to continue", item.status));
    }

    let mut repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    guard_working_tree(&repo_path, &mut repo, &format!("continuing {}", item.branch))?;

    run_git(&repo_path, "checkout", &item.branch, || {
        git_core::checkout_branch(&repo, &item.branch)
    })
    .map_err(|e| e.to_string())?;
    audit_best_effort(&repo_path, "continue_work", &item.branch, None);

    // Auto-apply the branch's Saved Work. The tree is clean here (guard stashed
    // whatever was dirty on the branch we just left), so this is safe.
    let swlog = saved_work_log();
    let saved_rec = swlog
        .as_ref()
        .and_then(|l| l.saved_for_branch(&repo_path, &item.branch).ok().flatten());

    let (restore_outcome, conflicting_files) = match (swlog.as_ref(), saved_rec.as_ref()) {
        (Some(l), Some(rec)) => {
            match run_git(&repo_path, "restore_work", &rec.branch, || {
                git_core::restore_work(&mut repo, &rec.stash_oid)
            }) {
                Ok(()) => {
                    l.set_status(rec.id, "restored").ok();
                    audit_best_effort(&repo_path, "resume_work", &rec.branch, None);
                    ("restored".to_string(), vec![])
                }
                Err(git_core::SaveWorkError::Conflict { files }) => {
                    l.set_status(rec.id, "conflict").ok();
                    audit_best_effort(&repo_path, "resume_work_conflict", &rec.branch, None);
                    ("conflict".to_string(), files)
                }
                Err(_) => ("error".to_string(), vec![]),
            }
        }
        _ => ("none".to_string(), vec![]),
    };

    let saved_work = saved_rec.map(saved_work_dto);
    let status = build_status(&repo_path).await?;
    Ok(ContinueOutcome { status, saved_work, restore_outcome, conflicting_files })
}

/// Outcome of `inspect_branch`: the status on the inspected branch plus the two
/// facts `end_branch_inspection` needs to put the user back exactly where they
/// were -- the branch to return to and the Saved Work row id (if entering
/// inspection had to stash a dirty tree).
#[derive(Debug, Clone, Serialize)]
pub struct InspectionOutcome {
    pub status: RepoStatus,
    pub original_branch: String,
    pub saved_work_id: Option<i64>,
}

/// Temporary read-only branch inspection: check out `develop` or `master` so
/// the user can look at its real state without running checkout themselves or
/// hand-managing uncommitted work.
///
/// Work Safe is the protection layer: `guard_working_tree` stashes a dirty tree
/// (tracked + untracked) before the checkout and hard-STOPs on a half-finished
/// merge/rebase or a failed save -- in either case nothing is checked out and
/// the original branch is kept. If the checkout itself fails after a stash, the
/// stash is re-applied so the tree is left exactly as it started. Never creates
/// a branch, never resets, never discards.
///
/// Pair with `end_branch_inspection`. Only `develop` and the repo's production
/// branch (`main`/`master`) are valid targets.
pub async fn inspect_branch(
    repo_path: String,
    target: String,
) -> Result<InspectionOutcome, String> {
    let mut repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let production = resolve_production(&repo)?;
    if target != "develop" && target != production {
        return Err(format!(
            "branch inspection only supports 'develop' or '{production}', not '{target}'"
        ));
    }
    let original_branch = current_branch(&repo)?;
    if original_branch == target {
        return Err(format!("already on '{target}' -- nothing to inspect"));
    }
    // Fail before Work Safe touches the tree if the target does not exist.
    repo.find_branch(&target, git2::BranchType::Local)
        .map_err(|_| format!("no local branch '{target}' to inspect"))?;

    let saved_work_id =
        guard_working_tree(&repo_path, &mut repo, &format!("inspecting {target}"))?;

    if let Err(e) = run_git(&repo_path, "checkout", &target, || {
        git_core::checkout_branch(&repo, &target)
    }) {
        // Checkout failed after we stashed -- put the tree back so the user is
        // left exactly where they started. Work Safe: never leave it worse.
        if let Some(id) = saved_work_id {
            if let Some(log) = saved_work_log() {
                if let Ok(Some(rec)) = log.get(id) {
                    if git_core::restore_work(&mut repo, &rec.stash_oid).is_ok() {
                        log.set_status(id, "restored").ok();
                    }
                }
            }
        }
        return Err(format!("checkout '{target}' failed: {e}"));
    }
    audit_best_effort(&repo_path, "inspect_branch", &target, None);

    let status = build_status(&repo_path).await?;
    Ok(InspectionOutcome { status, original_branch, saved_work_id })
}

/// Return from `inspect_branch`: check out `original_branch` and, when entering
/// inspection stashed a dirty tree, re-apply that exact Saved Work entry. A
/// restore collision is non-fatal -- the entry is kept and the working
/// directory holds the markers (`outcome: "conflict"`); nothing is reset. When
/// nothing was stashed, `outcome` is `"none"`.
pub async fn end_branch_inspection(
    repo_path: String,
    original_branch: String,
    saved_work_id: Option<i64>,
) -> Result<ResumeOutcome, String> {
    let mut repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    // The inspected branch is never written to, so it is normally clean; guard
    // anyway in case the user edited during inspection -- Work Safe protects
    // those edits rather than losing them to the checkout.
    guard_working_tree(&repo_path, &mut repo, &format!("returning to {original_branch}"))?;

    run_git(&repo_path, "checkout", &original_branch, || {
        git_core::checkout_branch(&repo, &original_branch)
    })
    .map_err(|e| format!("checkout '{original_branch}' failed: {e}"))?;
    audit_best_effort(&repo_path, "end_branch_inspection", &original_branch, None);

    let Some(id) = saved_work_id else {
        let status = build_status(&repo_path).await?;
        return Ok(ResumeOutcome { outcome: "none".into(), conflicting_files: vec![], status });
    };

    let log = saved_work_log().ok_or("saved-work log unavailable")?;
    let rec = log
        .get(id)
        .map_err(|e| e.to_string())?
        .ok_or("the Saved Work entry for this inspection no longer exists")?;

    match run_git(&repo_path, "restore_work", &rec.branch, || {
        git_core::restore_work(&mut repo, &rec.stash_oid)
    }) {
        Ok(()) => {
            log.set_status(id, "restored").ok();
            audit_best_effort(&repo_path, "resume_work", &rec.branch, None);
            let status = build_status(&repo_path).await?;
            Ok(ResumeOutcome { outcome: "restored".into(), conflicting_files: vec![], status })
        }
        Err(git_core::SaveWorkError::Conflict { files }) => {
            log.set_status(id, "conflict").ok();
            audit_best_effort(&repo_path, "resume_work_conflict", &rec.branch, None);
            let status = build_status(&repo_path).await?;
            Ok(ResumeOutcome { outcome: "conflict".into(), conflicting_files: files, status })
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Abandon a tracked work item. V1: flips the WIP status to `dropped` only --
/// never deletes the git branch, never touches Saved Work. `confirmation`
/// must equal the branch name (the frontend collects a type-to-confirm).
pub async fn drop_work(
    repo_path: String,
    work_item_id: i64,
    confirmation: String,
) -> Result<WorkList, String> {
    let log = wip_item_log().ok_or("work-item log unavailable")?;
    let item = log.get(work_item_id).map_err(|e| e.to_string())?.ok_or("no such work item")?;
    if item.repository != repo_path {
        return Err("that work item belongs to a different repository".to_string());
    }
    if confirmation.trim() != item.branch {
        return Err("confirmation text does not match the branch name".to_string());
    }
    log.set_status(&repo_path, &item.branch, "dropped").map_err(|e| e.to_string())?;
    audit_best_effort(&repo_path, "drop_work", &item.branch, None);
    Ok(build_work_list(&repo_path).await)
}

pub fn audit_best_effort(repository: &str, action: &str, branch: &str, mr_pr: Option<&str>) {
    let Some(audit) = AuditLog::default_path().ok().and_then(|p| AuditLog::open(p).ok()) else {
        return;
    };
    audit
        .log(&AuditEntry {
            timestamp: chrono::Utc::now(),
            user: whoami_user(),
            provider: "gitlab".to_string(),
            repository: repository.to_string(),
            branch: (!branch.is_empty()).then(|| branch.to_string()),
            mr_pr: mr_pr.map(str::to_string),
            action: action.to_string(),
            result: "success".to_string(),
            error: None,
        })
        .ok();
}

pub fn resolve_role_best_effort(
    user: &str,
    repository: &str,
    provider_role: Option<OverrideRole>,
    config: &Config,
) -> String {
    let audit = AuditLog::default_path()
        .ok()
        .and_then(|p| AuditLog::open(p).ok());

    match (provider_role, audit) {
        (Some(role), Some(audit)) => auth_core::resolve_role(user, repository, role, config, &audit)
            .map(|r| format!("{r:?}"))
            .unwrap_or_else(|_| "unknown".to_string()),
        (Some(role), None) => format!("{role:?}"),
        (None, Some(audit)) => {
            // No live provider role (offline / no token yet) -- still
            // honor a local override if one exists, and audit it.
            match config.find_override(user, repository) {
                Some(role) => {
                    audit
                        .log(&AuditEntry {
                            timestamp: chrono::Utc::now(),
                            user: user.to_string(),
                            provider: "n/a".to_string(),
                            repository: repository.to_string(),
                            branch: None,
                            mr_pr: None,
                            action: "resolve_role".to_string(),
                            result: format!("{role:?}:local_override"),
                            error: None,
                        })
                        .ok();
                    format!("{role:?}")
                }
                None => "unknown (no token)".to_string(),
            }
        }
        (None, None) => "unknown (no token)".to_string(),
    }
}

pub fn whoami_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Server-side Owner gate shared by Conflict Resolution and the role-override
/// commands -- re-verified per call via `resolve_workflow_role`, never trusts
/// a cached or frontend-supplied role.
pub async fn require_owner(repo_path: &str) -> Result<(), String> {
    match resolve_workflow_role(repo_path).await {
        WorkflowRole::Owner => Ok(()),
        _ => Err("this action is Owner-only".to_string()),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConflictInfo {
    pub branch: String,
    pub target_branch: String,
    pub conflicting_files: Vec<String>,
}

/// Owner-only: merges `target_branch` into the currently checked-out branch
/// **in the real project working directory** (Work Safe -- no worktree),
/// leaving conflict markers + `MERGE_HEAD` for the Owner to resolve in place.
/// Guards the tree first (dirty -> Save Work). Returns the conflicting files.
pub async fn start_conflict_resolution(
    repo_path: String,
    target_branch: String,
) -> Result<ConflictInfo, String> {
    require_owner(&repo_path).await?;

    let mut repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    guard_working_tree(
        &repo_path,
        &mut repo,
        &format!("resolving conflict with {target_branch}"),
    )?;
    let branch = current_branch(&repo)?;

    let merge = run_git(&repo_path, "merge", &target_branch, || {
        git_core::merge_target_into_head(&repo, &target_branch)
    })
    .map_err(|e| e.to_string())?;

    if let Some(log) = conflict_log() {
        log.start(&branch, &target_branch, &merge.target_commit.to_string()).ok();
    }
    audit_best_effort(&repo_path, "start_conflict_resolution", &branch, None);

    Ok(ConflictInfo {
        branch,
        target_branch,
        conflicting_files: merge.conflicting_files,
    })
}


/// Owner-only: verifies the working directory has no leftover conflict markers
/// and passes `git diff --check`, then records the resolution as a real
/// two-parent merge commit and pushes it back to the *original* branch --
/// never a new branch, never a second MR.
pub async fn verify_and_commit_resolution(
    repo_path: String,
) -> Result<RepoStatus, String> {
    require_owner(&repo_path).await?;

    let in_progress = conflict_log()
        .and_then(|log| log.current().ok().flatten())
        .ok_or("no conflict resolution in progress")?;

    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let workdir = repo
        .workdir()
        .ok_or("repository has no working directory")?
        .to_path_buf();
    git_core::verify_resolved(&repo, &workdir).map_err(|issues| {
        issues
            .iter()
            .map(|i| {
                let loc = i.line.map(|l| format!(":{l}")).unwrap_or_default();
                format!("{}{loc}: {}", i.file, i.detail)
            })
            .collect::<Vec<_>>()
            .join("; ")
    })?;

    let target_commit = git2::Oid::from_str(&in_progress.target_commit).map_err(|e| e.to_string())?;
    run_git(&repo_path, "commit_merge", &in_progress.branch, || {
        git_core::commit_merge(
            &repo,
            target_commit,
            &format!(
                "merge: resolve conflict from {} into {}",
                in_progress.target_branch, in_progress.branch
            ),
        )
    })
    .map_err(|e| e.to_string())?;
    run_git(&repo_path, "push", &in_progress.branch, || {
        git_core::push(&repo, &in_progress.branch)
    })
    .map_err(|e| e.to_string())?;

    if let Some(log) = conflict_log() {
        log.clear().ok();
    }

    audit_best_effort(&repo_path, "verify_and_commit_resolution", &in_progress.branch, None);
    build_status(&repo_path).await
}

/// One capability's display state for the wizard/re-validate UI: never a
/// generic "invalid token" -- always a specific reason per spec.
#[derive(Debug, Clone, Serialize)]
pub struct CapabilityItem {
    pub label: String,
    pub status: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TokenValidation {
    pub capabilities: Vec<CapabilityItem>,
}

pub fn capability_item(label: &str, cap: Capability, reason_no: &str) -> CapabilityItem {
    let (status, reason) = match cap {
        Capability::Yes => ("yes", "confirmed via provider API".to_string()),
        Capability::No => ("no", reason_no.to_string()),
        Capability::Unknown => ("unknown", "could not reach provider to confirm".to_string()),
    };
    CapabilityItem {
        label: label.to_string(),
        status: status.to_string(),
        reason,
    }
}

/// Re-runs the same non-mutating capability probe used at Checkpoint-0
/// against the currently saved token, for the First Connection wizard and
/// for re-validating after a token rotation.
pub async fn re_validate_token(repo_path: String) -> Result<TokenValidation, String> {
    let info = git_core::read_repo_info(&repo_path).map_err(|e| e.to_string())?;
    let client = provider_client_for(&info.remote_url)
        .await
        .ok_or("no token saved for this host, or the remote is not a reachable GitLab/GitHub host")?;
    let report = client.detect_capabilities().await;
    Ok(TokenValidation {
        capabilities: vec![
            capability_item("Repository access", report.repo_access, "the token cannot see this repository"),
            capability_item("Read pull / merge requests", report.read_mr, "insufficient permissions"),
            capability_item("Create pull / merge requests", report.create_mr, "requires write access"),
            capability_item(
                "Read mergeability / conflicts",
                report.mergeability_read,
                "insufficient permissions",
            ),
            capability_item("Read pipeline / checks status", report.pipeline_read, "insufficient permissions"),
            capability_item("Read review status", report.review_read, "insufficient permissions"),
        ],
    })
}

/// Removes the saved token so the app falls back to the First Connection
/// (no-token) state. Deletes from whichever service (`github`/`gitlab`) the
/// repo's remote resolves to -- the same dispatch `save_token` uses.
pub async fn delete_token(repo_path: String, host: String) -> Result<(), String> {
    let service = credential_service(&repo_path, &host).await;
    auth_core::CredentialStore::delete(service, &host, "default").map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct RoleOverrideDto {
    pub user: String,
    pub repository: String,
    pub role: String,
}

pub fn override_role_str(role: OverrideRole) -> &'static str {
    match role {
        OverrideRole::Owner => "owner",
        OverrideRole::Member => "member",
    }
}

pub fn parse_override_role(role: &str) -> Result<OverrideRole, String> {
    match role {
        "owner" => Ok(OverrideRole::Owner),
        "member" => Ok(OverrideRole::Member),
        other => Err(format!("unknown role: {other}")),
    }
}

/// Owner-only: lists every local role override. Same gate as writing --
/// this panel has no Member-facing use.
pub async fn list_role_overrides(repo_path: String) -> Result<Vec<RoleOverrideDto>, String> {
    require_owner(&repo_path).await?;
    let path = auth_core::default_config_path().map_err(|e| e.to_string())?;
    let config = auth_core::load_config(&path).map_err(|e| e.to_string())?;
    Ok(config
        .overrides
        .into_iter()
        .map(|o| RoleOverrideDto {
            user: o.user,
            repository: o.repository,
            role: override_role_str(o.role).to_string(),
        })
        .collect())
}

/// Owner-only: writes (or replaces) a role override for `user`+`repository`.
/// Every write is audit-logged so a later `resolve_role` audit row can be
/// cross-checked against who changed it and when.
pub async fn set_role_override(
    repo_path: String,
    user: String,
    repository: String,
    role: String,
) -> Result<(), String> {
    require_owner(&repo_path).await?;
    let role = parse_override_role(&role)?;
    let path = auth_core::default_config_path().map_err(|e| e.to_string())?;
    let mut config = auth_core::load_config(&path).map_err(|e| e.to_string())?;
    config.overrides.retain(|o| !(o.user == user && o.repository == repository));
    config.overrides.push(RoleOverride { user: user.clone(), repository: repository.clone(), role });
    auth_core::save_config(&path, &config).map_err(|e| e.to_string())?;
    audit_best_effort(&repository, "set_role_override", &user, None);
    Ok(())
}

/// Owner-only: removes a role override, falling back to the provider's own
/// role mapping for that user+repository.
pub async fn remove_role_override(repo_path: String, user: String, repository: String) -> Result<(), String> {
    require_owner(&repo_path).await?;
    let path = auth_core::default_config_path().map_err(|e| e.to_string())?;
    let mut config = auth_core::load_config(&path).map_err(|e| e.to_string())?;
    config.overrides.retain(|o| !(o.user == user && o.repository == repository));
    auth_core::save_config(&path, &config).map_err(|e| e.to_string())?;
    audit_best_effort(&repository, "remove_role_override", &user, None);
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditEntryDto {
    pub timestamp: String,
    pub user: String,
    pub repository: String,
    pub branch: Option<String>,
    pub action: String,
    pub result: String,
    pub error: Option<String>,
}

/// Read-only audit log view. Ungated -- the schema already forbids
/// token/diff/secret columns, so there's nothing here worth Member-hiding.
pub fn get_audit_log(limit: u32) -> Result<Vec<AuditEntryDto>, String> {
    let path = AuditLog::default_path().map_err(|e| e.to_string())?;
    let log = AuditLog::open(&path).map_err(|e| e.to_string())?;
    let rows = log.recent(limit).map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|e| AuditEntryDto {
            timestamp: e.timestamp.to_rfc3339(),
            user: e.user,
            repository: e.repository,
            branch: e.branch,
            action: e.action,
            result: e.result,
            error: e.error,
        })
        .collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandLogDto {
    pub timestamp: String,
    pub operation: String,
    /// Already secret-masked at write time.
    pub args: String,
    /// `"success"` | `"failure"`.
    pub outcome: String,
    pub duration_ms: i64,
    /// Already secret-masked at write time.
    pub error: Option<String>,
}

/// Read-only: rows from the real `command_log` SQLite table for this repo,
/// newest first. `args`/`error` were masked before they were ever stored, so
/// nothing here needs re-masking. The frontend never opens SQLite itself.
pub fn get_command_log(repo_path: String, limit: u32) -> Result<Vec<CommandLogDto>, String> {
    let Some(log) = AuditLog::default_path().ok().and_then(|p| CommandLog::open(p).ok()) else {
        return Ok(vec![]);
    };
    Ok(log
        .for_repo(&repo_path, limit)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|r| CommandLogDto {
            timestamp: r.timestamp,
            operation: r.operation,
            args: r.args,
            outcome: r.outcome,
            duration_ms: r.duration_ms,
            error: r.error,
        })
        .collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionPreview {
    pub current_version: String,
    pub next_version: String,
}

/// Read-only: the current `VERSION` and the patch-bumped value a hotfix would
/// write, computed by the one version primitive in `git_core`. Purely a
/// preview -- it writes nothing. The frontend must not compute version bumps
/// itself.
pub fn get_hotfix_version_preview(repo_path: String) -> Result<VersionPreview, String> {
    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let workdir = repo.workdir().ok_or("repository has no working directory")?;
    let current = git_core::read_version_file(workdir).map_err(|e| e.to_string())?;
    Ok(VersionPreview {
        current_version: current.to_string(),
        next_version: current.bump_patch().to_string(),
    })
}

// --- Release workflow -------------------------------------------------------
//
// A Release Candidate is a one-shot immutable snapshot of `develop` submitted
// for production review. The release branch carries only VERSION + CHANGELOG +
// one prep commit -- never product code. Once `finish_release` opens the
// `release/* -> production` MR the developer is done and is parked back on
// `develop`; human review/merge runs in parallel and never blocks develop.
// Multiple candidates may coexist on the provider: VC Flow tracks one current
// candidate plus any number of `superseded` ones (wip_items.status).

#[derive(Debug, Clone, Serialize)]
pub struct PendingCandidate {
    pub branch: String,
    pub version: String,
    pub mr_iid: String,
    pub merged: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReleasePreview {
    pub current_version: String,
    pub commit_count: usize,
    /// `"major"` | `"minor"` | `"patch"` -- Conventional Commit impact of the
    /// range, defaulting to `"patch"` for an all-chore/empty range (still ships).
    pub impact: String,
    pub suggested_version: String,
    /// Markdown lines to prefill the CHANGELOG textarea.
    pub changelog_seed: Vec<String>,
    pub pending_candidates: Vec<PendingCandidate>,
    /// Count of commits on `origin/<production>` that `origin/develop` lacks
    /// (`rev-list origin/<production> --not origin/develop`, merges excluded).
    /// Non-zero => production is ahead; sync production -> develop before a new
    /// release. The frontend gates the [Prepare Release] button on this.
    pub production_commits_not_in_develop: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SupersededCandidate {
    pub version: String,
    pub branch: String,
    pub mr_iid: String,
    pub web_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReleaseStatusDto {
    pub version: String,
    pub candidate_branch: String,
    pub production: Option<MrStatus>,
    pub sync: Option<MrStatus>,
    /// Production MR merged but no sync MR opened yet -> surface [Sync Develop].
    pub sync_required: bool,
    /// Both MRs merged.
    pub complete: bool,
    pub superseded: Vec<SupersededCandidate>,
}

/// `origin/<production>:VERSION` parsed, or `0.0.0` when the repo has never
/// released (no remote production ref / no VERSION there yet). The current
/// production version -- not local develop's, which may lag hotfixes.
pub fn production_version(repo: &Repository, production: &str) -> git_core::Version {
    git_core::read_file_at_ref(repo, &format!("refs/remotes/origin/{production}"), "VERSION")
        .ok()
        .and_then(|s| git_core::Version::parse(&s).ok())
        .unwrap_or(git_core::Version { major: 0, minor: 0, patch: 0 })
}

pub fn release_range_refs(repo: &Repository, production: &str) -> (String, String) {
    let prod = format!("refs/remotes/origin/{production}");
    let prod = if repo.revparse_single(&prod).is_ok() { prod } else { production.to_string() };
    let dev = "refs/remotes/origin/develop".to_string();
    let dev = if repo.revparse_single(&dev).is_ok() { dev } else { "develop".to_string() };
    (prod, dev)
}

/// `release/1.4.0-2` -> `"1.4.0"` (the `-N` supersede suffix is dropped).
pub fn version_from_release_branch(branch: &str) -> String {
    branch
        .strip_prefix("release/")
        .and_then(|r| r.split('-').next())
        .unwrap_or("")
        .to_string()
}

/// The production-target MR iid tracked for a release candidate branch.
pub fn candidate_production_mr(repo_path: &str, branch: &str, production: &str) -> Option<String> {
    work_item_log()?
        .mrs_for_branch(repo_path, branch)
        .ok()?
        .into_iter()
        .find(|m| m.target_branch == production)
        .map(|m| m.mr_iid)
}

/// The `develop`-target (sync) MR iid tracked for a release candidate branch.
pub fn candidate_sync_mr(repo_path: &str, branch: &str) -> Option<String> {
    work_item_log()?
        .mrs_for_branch(repo_path, branch)
        .ok()?
        .into_iter()
        .find(|m| m.target_branch == "develop")
        .map(|m| m.mr_iid)
}

/// Reconciliation truth for a release candidate's develop-sync (spec D3):
/// **git ancestry first, MR state second**.
pub struct ReleaseSyncState {
    /// The shipped release commit is an ancestor of `origin/develop` -- the
    /// `v<version>` tag / `chore: release <version>` prep commit has arrived,
    /// regardless of any MR.
    reached_develop: bool,
    /// The develop-sync is done: production is merged AND either the release
    /// commit reached develop or a `production -> develop` sync MR merged.
    satisfied: bool,
    /// Production is merged but the sync is not done -- a sync MR is still
    /// owed. This, not "a sync MR row exists", is what blocks the next release.
    sync_required: bool,
}

/// Decide (and reconcile) whether a release candidate's develop-sync is done.
///
/// A merge request is only the mechanism that moves the release commit onto
/// `develop`; the proof it arrived is git ancestry. So a closed / empty /
/// un-mergeable sync MR never counts as "synced", and -- conversely -- once
/// the release commit is an ancestor of `origin/develop` the sync is done
/// even with no sync MR at all. When the sync is satisfied this advances a
/// still-open WIP row to `completed`, so a stale or manually-interrupted DB
/// row can never deadlock the next release (`SYNC_REQUIRED`).
///
/// Callers pass the two MR-merge booleans (provider-free, so this stays
/// unit-testable); `fetch_origin` must have run first for the ancestry check
/// to see current refs.
pub fn reconcile_release_sync(
    repo: &Repository,
    repo_path: &str,
    candidate_branch: &str,
    production: &str,
    production_merged: bool,
    sync_mr_merged: bool,
) -> ReleaseSyncState {
    let (prod_ref, dev_ref) = release_range_refs(repo, production);
    let version = version_from_release_branch(candidate_branch);
    let reached_develop = !version.is_empty()
        && git_core::release_reached_branch(repo, &version, &prod_ref, &dev_ref)
            .ok()
            .flatten()
            .unwrap_or(false);

    let satisfied = production_merged && (reached_develop || sync_mr_merged);
    let sync_required = production_merged && !satisfied;

    if satisfied {
        if let Some(log) = wip_item_log() {
            if log.complete_if_open(repo_path, candidate_branch).unwrap_or(false) {
                audit_best_effort(repo_path, "reconcile_release_sync", candidate_branch, None);
            }
        }
    }

    ReleaseSyncState { reached_develop, satisfied, sync_required }
}

/// Read-only, NOT Owner-gated (D6): what `create_release_candidate` would do --
/// the current production version, the Conventional Commit impact of
/// `develop` since the last release, the suggested next version, a CHANGELOG
/// seed, and any candidates already in flight.
pub async fn get_release_preview(repo_path: String) -> Result<ReleasePreview, String> {
    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let _ = git_core::fetch_origin(&repo);
    let production = resolve_production(&repo)?;
    let current = production_version(&repo, &production);

    let (prod_ref, dev_ref) = release_range_refs(&repo, &production);
    let commits =
        git_core::commits_to_release(&repo, &prod_ref, &dev_ref).map_err(|e| e.to_string())?;
    // Reverse range: commits on production that develop lacks. Non-zero means
    // production is ahead (hotfix / un-synced release) and develop must sync
    // before a new release is prepared. ponytail: reuse commits_to_release with
    // swapped refs rather than a new git helper.
    let production_commits_not_in_develop =
        git_core::commits_to_release(&repo, &dev_ref, &prod_ref)
            .map_err(|e| e.to_string())?
            .len();
    let bump = git_core::conventional_bump(&commits);
    let suggested = git_core::suggest_version(&current, bump);

    let remote_url =
        repo.find_remote("origin").ok().and_then(|r| r.url().map(str::to_string));
    let client = provider_client_for(&remote_url).await;

    let mut pending = Vec::new();
    for item in wip_item_log()
        .and_then(|l| l.by_type(&repo_path, "release").ok())
        .unwrap_or_default()
    {
        if !matches!(item.status.as_str(), "active" | "waiting") {
            continue;
        }
        let Some(iid) = candidate_production_mr(&repo_path, &item.branch, &production) else {
            continue;
        };
        let merged = match &client {
            Some(c) => mr_snapshot(c, &iid).await.map(|m| m.merged).unwrap_or(false),
            None => false,
        };
        if merged {
            // Production merged: if the release already reached develop (git
            // truth, D3) this candidate is done -- reconcile the stale WIP row
            // and drop it from the pending list so it stops blocking.
            let sync_merged = match (&client, candidate_sync_mr(&repo_path, &item.branch)) {
                (Some(c), Some(sid)) => {
                    mr_snapshot(c, &sid).await.map(|m| m.merged).unwrap_or(false)
                }
                _ => false,
            };
            let state = reconcile_release_sync(
                &repo, &repo_path, &item.branch, &production, merged, sync_merged,
            );
            if state.satisfied {
                continue;
            }
        }
        pending.push(PendingCandidate {
            version: version_from_release_branch(&item.branch),
            branch: item.branch,
            mr_iid: iid,
            merged,
        });
    }

    Ok(ReleasePreview {
        current_version: current.to_string(),
        commit_count: commits.len(),
        impact: bump.as_str().to_string(),
        suggested_version: suggested.to_string(),
        changelog_seed: git_core::changelog_seed(&commits),
        pending_candidates: pending,
        production_commits_not_in_develop,
    })
}

/// Owner-only, step 1 of 2 (D2). Must run on `develop`. Fast-forwards develop,
/// creates `release/<version>[-N]` (D4), writes VERSION + prepends CHANGELOG,
/// and makes the single `chore: release <version>` prep commit -- then leaves
/// the user on the release branch to review the diff before `finish_release`.
///
/// Errors (returned as prefixed strings the frontend classifies):
/// - `SYNC_REQUIRED: ...`  a prior candidate's production MR already merged (D3)
/// - `SUPERSEDE_REQUIRED: ...`  un-merged candidates exist and
///   `supersede_confirmed` is false
pub async fn create_release_candidate(
    repo_path: String,
    version: String,
    changelog_body: String,
    supersede_confirmed: bool,
) -> Result<RepoStatusWithPath, String> {
    require_owner(&repo_path).await?;

    let mut repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    if current_branch(&repo)? != "develop" {
        return Err("release candidates are prepared from develop -- switch to develop first".into());
    }
    let production = resolve_production(&repo)?;

    guard_working_tree(&repo_path, &mut repo, "preparing a release candidate")?;

    let _ = git_core::fetch_origin(&repo);
    run_git(&repo_path, "fast_forward", "develop", || {
        git_core::fast_forward_from_origin(&repo, "develop")
    })
    .map_err(|e| e.to_string())?;

    let current = production_version(&repo, &production);
    let new_v = git_core::Version::parse(&version).map_err(|e| e.to_string())?;
    if new_v <= current {
        return Err(format!(
            "version {new_v} must be greater than the current production version {current}"
        ));
    }

    let mut pending: Vec<auth_core::WipItem> = wip_item_log()
        .and_then(|l| l.by_type(&repo_path, "release").ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|i| matches!(i.status.as_str(), "active" | "waiting"))
        .collect();

    if !pending.is_empty() {
        let remote_url =
            repo.find_remote("origin").ok().and_then(|r| r.url().map(str::to_string));
        if let Some(client) = provider_client_for(&remote_url).await {
            let mut unresolved = Vec::new();
            for item in pending {
                let prod_iid = candidate_production_mr(&repo_path, &item.branch, &production);
                let prod_merged = match &prod_iid {
                    Some(iid) => mr_snapshot(&client, iid).await.map(|m| m.merged).unwrap_or(false),
                    None => false,
                };
                if !prod_merged {
                    // Candidate still in review -- a real supersede, not a sync gap.
                    unresolved.push(item);
                    continue;
                }
                // Production merged: is the shipped release already on develop?
                // Reconcile against git ancestry (D3) before letting a stale WIP
                // row block this release forever.
                let sync_merged = match candidate_sync_mr(&repo_path, &item.branch) {
                    Some(iid) => mr_snapshot(&client, &iid).await.map(|m| m.merged).unwrap_or(false),
                    None => false,
                };
                let state = reconcile_release_sync(
                    &repo, &repo_path, &item.branch, &production, prod_merged, sync_merged,
                );
                if state.sync_required {
                    let iid = prod_iid.as_deref().unwrap_or("?");
                    return Err(format!(
                        "SYNC_REQUIRED: release candidate {} has already merged to {production} \
                         (MR {iid}). Sync develop before preparing another release.",
                        item.branch
                    ));
                }
                // Otherwise reconcile_release_sync just marked it `completed`;
                // it is no longer pending and must not be superseded.
            }
            pending = unresolved;
        }
        if !supersede_confirmed && !pending.is_empty() {
            let names: Vec<&str> = pending.iter().map(|i| i.branch.as_str()).collect();
            return Err(format!(
                "SUPERSEDE_REQUIRED: {} pending release candidate(s) will be superseded: {}",
                pending.len(),
                names.join(", ")
            ));
        }
    }

    let mut name = format!("release/{new_v}");
    let mut n = 2;
    while git_core::ref_exists(&repo, &name) {
        name = format!("release/{new_v}-{n}");
        n += 1;
    }

    run_git(&repo_path, "create_release_branch", &name, || {
        git_core::create_release_branch(&repo, &name, "develop")
    })
    .map_err(|e| e.to_string())?;

    let workdir = repo
        .workdir()
        .ok_or("repository has no working directory")?
        .to_path_buf();
    git_core::write_version_file(&workdir, &new_v).map_err(|e| e.to_string())?;
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    git_core::prepend_section(&workdir, &new_v, &today, &changelog_body).map_err(|e| e.to_string())?;

    let msg = format!("chore: release {new_v}");
    run_git(&repo_path, "commit", &msg, || git_core::commit_all(&repo, &msg))
        .map_err(|e| e.to_string())?;

    if let Some(log) = wip_item_log() {
        log.start(&repo_path, &name, "release").ok();
        for item in &pending {
            log.set_status(&repo_path, &item.branch, "superseded").ok();
        }
    }
    audit_best_effort(&repo_path, "create_release_candidate", &name, None);

    let status = build_status(&repo_path).await?;
    Ok(RepoStatusWithPath { status, repo_path })
}

/// Owner-only, step 2 of 2 (D2). Pushes the release branch and opens the single
/// `release/* -> production` MR, then parks the user back on `develop`. The
/// `production -> develop` sync is a separate action once a candidate merges
/// (`sync_develop_after_release`).
pub async fn finish_release(
    repo_path: String,
    title: String,
) -> Result<RepoStatusWithPath, String> {
    require_owner(&repo_path).await?;

    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let branch = current_branch(&repo)?;
    if classify_branch(&branch) != BranchClass::Release {
        return Err("finish_release must run on a release/* branch".into());
    }
    let production = resolve_production(&repo)?;

    run_git(&repo_path, "push", &branch, || git_core::push(&repo, &branch))
        .map_err(|e| e.to_string())?;

    let remote_url =
        repo.find_remote("origin").ok().and_then(|r| r.url().map(str::to_string));
    let client = provider_client_for(&remote_url)
        .await
        .ok_or("could not reach the provider API for this remote (check token/host)")?;
    let mr = client
        .create_merge_request(&branch, &production, &title)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(log) = work_item_log() {
        log.add_mr(&repo_path, &branch, &production, &mr.id).ok();
    }
    if let Some(log) = wip_item_log() {
        log.set_status(&repo_path, &branch, "waiting").ok();
    }
    audit_best_effort(&repo_path, "finish_release", &branch, Some(&mr.id));

    // Developer side done -- park back on develop. Best-effort (see finish_hotfix).
    let _ = run_git(&repo_path, "checkout", "develop", || {
        git_core::checkout_branch(&repo, "develop")
    });

    let status = build_status(&repo_path).await?;
    Ok(RepoStatusWithPath { status, repo_path })
}

/// AppHandle- and provider-free core of `sync_develop_after_release`'s tagging
/// step, so it is directly testable. Given that the production MR merge is
/// *already confirmed* by the caller, fetch `origin` and idempotently publish
/// the `v<version>` tag on `origin/<production>`.
///
/// `production_mr_merged == false` is a hard stop -- no fetch, no tag. The
/// version comes from `version_from_release_branch` (drops any `-N` supersede
/// suffix), so a superseded candidate still tags the plain `vX.Y.Z`.
pub fn sync_release_tag_inner(
    repo_path: &str,
    candidate_branch: &str,
    production_mr_merged: bool,
) -> Result<git_core::TagOutcome, String> {
    if !production_mr_merged {
        return Err("the release's production merge request is not merged yet -- merge it before \
                    syncing develop"
            .into());
    }

    let repo = Repository::discover(repo_path).map_err(|e| e.to_string())?;
    let production = resolve_production(&repo)?;
    let _ = git_core::fetch_origin(&repo);

    let version = version_from_release_branch(candidate_branch);
    if version.is_empty() {
        return Err(format!("could not derive a version from '{candidate_branch}'"));
    }
    let tag = format!("v{version}");
    let target = format!("refs/remotes/origin/{production}");

    run_git(repo_path, "tag_release", &tag, || {
        git_core::ensure_release_tag(&repo, &tag, &target, &format!("Release {version}"))
    })
    .map_err(|e| e.to_string())
}

/// Owner-only: tags the shipped release on `origin/<production>` and opens the
/// `production -> develop` sync MR for a merged release candidate. Surfaced by
/// the Active Release panel only after that candidate's production MR has
/// actually merged (D3 remediation path too).
///
/// The production-merge state is re-confirmed here against the provider -- the
/// frontend `sync_required` gate is a hint, not the authority. Safe for an
/// Owner to retry: an existing tag and an already-open sync MR are both no-ops.
pub async fn sync_develop_after_release(
    repo_path: String,
    candidate_branch: String,
    title: String,
) -> Result<RepoStatus, String> {
    require_owner(&repo_path).await?;

    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let production = resolve_production(&repo)?;

    let remote_url =
        repo.find_remote("origin").ok().and_then(|r| r.url().map(str::to_string));
    let client = provider_client_for(&remote_url)
        .await
        .ok_or("could not reach the provider API for this remote (check token/host)")?;

    // Re-confirm the production MR is merged before tagging anything.
    let prod_mr = candidate_production_mr(&repo_path, &candidate_branch, &production)
        .ok_or("no production merge request is tracked for this release candidate")?;
    let merged = mr_snapshot(&client, &prod_mr).await.map(|m| m.merged).unwrap_or(false);
    sync_release_tag_inner(&repo_path, &candidate_branch, merged)?;

    // Git truth (D3): if the shipped release commit is already an ancestor of
    // origin/develop -- because develop had every commit this release carried,
    // or a prior sync already landed -- there is nothing to merge. Do not open
    // an empty MR the user then has to close; just reconcile the WIP row.
    // `sync_release_tag_inner` fetched origin, so refs are current.
    let state = reconcile_release_sync(
        &repo, &repo_path, &candidate_branch, &production, merged, false,
    );
    if state.reached_develop {
        audit_best_effort(&repo_path, "sync_develop_after_release", &candidate_branch, None);
        return build_status(&repo_path).await;
    }

    // Reuse an already-open sync MR on retry rather than opening a duplicate --
    // but a Closed / Merged one must be replaced, not reused.
    let existing_sync = candidate_sync_mr(&repo_path, &candidate_branch);
    let reusable = match &existing_sync {
        Some(iid) => match fetch_mr_status(&client, iid).await {
            Ok(s) => !matches!(s.status.as_str(), "Closed" | "Merged"),
            // Can't read it -> assume open so we never open a duplicate.
            Err(_) => true,
        },
        None => false,
    };

    let mr_id = if reusable {
        existing_sync.unwrap()
    } else {
        let mr = client
            .create_merge_request(&production, "develop", &format!("sync: {title}"))
            .await
            .map_err(|e| e.to_string())?;
        if let Some(log) = work_item_log() {
            log.add_mr(&repo_path, &candidate_branch, "develop", &mr.id).ok();
        }
        mr.id
    };
    audit_best_effort(&repo_path, "sync_develop_after_release", &candidate_branch, Some(&mr_id));

    build_status(&repo_path).await
}

/// Read-only: the current release candidate's two MRs plus any superseded
/// candidates. `None` when no release candidate is tracked for this repo.
pub async fn get_release_status(repo_path: String) -> Result<Option<ReleaseStatusDto>, String> {
    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let production = resolve_production(&repo)?;

    let all = wip_item_log()
        .and_then(|l| l.by_type(&repo_path, "release").ok())
        .unwrap_or_default();
    let Some(current) = all
        .iter()
        .find(|i| matches!(i.status.as_str(), "active" | "waiting"))
        .cloned()
    else {
        return Ok(None);
    };

    let remote_url =
        repo.find_remote("origin").ok().and_then(|r| r.url().map(str::to_string));
    let client = provider_client_for(&remote_url).await;

    let mrs = work_item_log()
        .and_then(|l| l.mrs_for_branch(&repo_path, &current.branch).ok())
        .unwrap_or_default();
    let prod_iid = mrs.iter().find(|m| m.target_branch == production).map(|m| m.mr_iid.clone());
    let sync_iid = mrs.iter().find(|m| m.target_branch == "develop").map(|m| m.mr_iid.clone());

    let mut prod_status = None;
    let mut sync_status = None;
    if let Some(c) = &client {
        if let Some(i) = &prod_iid {
            prod_status = fetch_mr_status(c, i).await.ok();
        }
        if let Some(i) = &sync_iid {
            sync_status = fetch_mr_status(c, i).await.ok();
        }
    }

    let prod_merged = prod_status.as_ref().map(|s| s.status == "Merged").unwrap_or(false);
    let sync_merged = sync_status.as_ref().map(|s| s.status == "Merged").unwrap_or(false);

    // Git ancestry is the authority (D3): a closed / un-mergeable sync MR never
    // counts as synced, and the release reaching develop by any route (tag /
    // prep-commit ancestry, or a merged sync MR) completes it. Reconciles the
    // WIP row when satisfied.
    let _ = git_core::fetch_origin(&repo);
    let state = reconcile_release_sync(
        &repo, &repo_path, &current.branch, &production, prod_merged, sync_merged,
    );
    let sync_required = state.sync_required;
    let complete = state.satisfied;

    let mut superseded = Vec::new();
    for item in all.iter().filter(|i| i.status == "superseded") {
        let Some(iid) = candidate_production_mr(&repo_path, &item.branch, &production) else {
            continue;
        };
        let web_url = match &client {
            Some(c) => fetch_mr_status(c, &iid).await.ok().map(|s| s.web_url),
            None => None,
        };
        superseded.push(SupersededCandidate {
            version: version_from_release_branch(&item.branch),
            branch: item.branch.clone(),
            mr_iid: iid,
            web_url,
        });
    }

    Ok(Some(ReleaseStatusDto {
        version: version_from_release_branch(&current.branch),
        candidate_branch: current.branch,
        production: prod_status,
        sync: sync_status,
        sync_required,
        complete,
        superseded,
    }))
}

/// Fast-forward `develop` or the production branch to `origin/<branch>`.
/// Refuses on any other branch, a dirty tree, or a real divergence -- it is a
/// plain pull that never touches uncommitted work (Work Safe). Surfaced as the
/// next action whenever HEAD sits on a stale develop/production.
pub async fn update_branch(repo_path: String) -> Result<RepoStatus, String> {
    let repo = Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let branch = current_branch(&repo)?;
    let production = resolve_production(&repo).ok();
    if branch != "develop" && production.as_deref() != Some(branch.as_str()) {
        return Err("update is only for develop or the production branch".into());
    }
    if git_core::read_repository_state(&repo)
        .map_err(|e| e.to_string())?
        .working_tree
        .is_dirty()
    {
        return Err("commit or save your work first -- update does a plain fast-forward".into());
    }

    let _ = git_core::fetch_origin(&repo);
    run_git(&repo_path, "fast_forward", &branch, || {
        git_core::fast_forward_from_origin(&repo, &branch)
    })
    .map_err(|e| e.to_string())?;
    audit_best_effort(&repo_path, "update_branch", &branch, None);
    build_status(&repo_path).await
}
#[cfg(test)]
mod tests {
    use super::*;
    use provider_core::github::github_api_base;
    use std::path::Path;
    use std::process::Command;
    use std::sync::OnceLock;
    use tempfile::{tempdir, TempDir};

    /// Point every `*_log()` helper at a throwaway sqlite file for the whole
    /// test binary, so tests never touch the real audit database. Repo-scoped
    /// columns keep per-test rows apart (each test uses a unique tempdir path).
    fn isolate_db() {
        static DB_DIR: OnceLock<TempDir> = OnceLock::new();
        let dir = DB_DIR.get_or_init(|| tempdir().unwrap());
        std::env::set_var("GWE_DATA_DIR", dir.path());
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").args(args).current_dir(dir).status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    fn git_out(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").args(args).current_dir(dir).output().unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// A work repo on `main` with one commit and a bare `origin` it can push to.
    fn repo_with_origin() -> (TempDir, TempDir) {
        let origin = tempdir().unwrap();
        git(origin.path(), &["init", "--bare", "-b", "main"]);
        let work = tempdir().unwrap();
        git(work.path(), &["init", "-b", "main"]);
        git(work.path(), &["config", "user.email", "t@e.com"]);
        git(work.path(), &["config", "user.name", "T"]);
        std::fs::write(work.path().join("a.txt"), "a\n").unwrap();
        git(work.path(), &["add", "a.txt"]);
        git(work.path(), &["commit", "-m", "init"]);
        git(work.path(), &["remote", "add", "origin", origin.path().to_str().unwrap()]);
        git(work.path(), &["push", "origin", "main"]);
        git(work.path(), &["fetch", "origin"]);
        (origin, work)
    }

    fn noop(_: &str) {}

    #[test]
    fn init_clean_repo_creates_and_pushes_develop_without_feature_initial() {
        isolate_db();
        let (origin, work) = repo_with_origin();
        let main_tip = git_out(work.path(), &["rev-parse", "main"]);
        let rp = work.path().to_str().unwrap();

        let out = initialize_workflow_inner(rp, &noop).unwrap();

        assert_eq!(out.final_branch, "develop");
        assert!(out.develop_created && out.develop_pushed);
        assert!(out.saved_work_label.is_none());
        assert_eq!(git_out(work.path(), &["rev-parse", "--abbrev-ref", "HEAD"]), "develop");
        assert!(git_out(work.path(), &["branch", "--list", "feature/initial"]).is_empty());
        assert_eq!(git_out(work.path(), &["rev-parse", "main"]), main_tip, "main must not move");
        let refs = git_out(origin.path(), &["for-each-ref", "--format=%(refname)"]);
        assert!(refs.contains("refs/heads/develop"), "origin refs: {refs}");
    }

    #[test]
    fn init_dirty_repo_saves_restores_onto_feature_initial() {
        isolate_db();
        let (_origin, work) = repo_with_origin();
        let main_tip = git_out(work.path(), &["rev-parse", "main"]);
        std::fs::write(work.path().join("a.txt"), "changed\n").unwrap();
        std::fs::write(work.path().join("new.txt"), "new\n").unwrap();
        let rp = work.path().to_str().unwrap();

        let out = initialize_workflow_inner(rp, &noop).unwrap();

        assert_eq!(out.final_branch, "feature/initial");
        assert!(out.restored);
        assert!(out.conflicts.is_empty());
        assert_eq!(out.saved_work_label.as_deref(), Some(INIT_WORKFLOW_SAVE_LABEL));
        assert_eq!(std::fs::read_to_string(work.path().join("a.txt")).unwrap(), "changed\n");
        assert_eq!(std::fs::read_to_string(work.path().join("new.txt")).unwrap(), "new\n");
        assert_eq!(git_out(work.path(), &["rev-parse", "main"]), main_tip);

        // The Save Work row exists, was recorded on `main`, and is now restored.
        let rec = saved_work_log()
            .unwrap()
            .actionable_entries(rp)
            .unwrap()
            .into_iter()
            .find(|r| r.label == INIT_WORKFLOW_SAVE_LABEL)
            .expect("init Save Work row");
        assert_eq!(rec.branch, "main");
        assert_eq!(rec.status, "restored");
    }

    #[test]
    fn init_develop_push_failure_keeps_saved_work() {
        isolate_db();
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "t@e.com"]);
        git(dir.path(), &["config", "user.name", "T"]);
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "init"]);
        git(dir.path(), &["remote", "add", "origin", "http://127.0.0.1:1/r.git"]);
        std::fs::write(dir.path().join("a.txt"), "dirty\n").unwrap();
        let rp = dir.path().to_str().unwrap();

        let err = initialize_workflow_inner(rp, &noop).unwrap_err();
        assert!(err.contains("Could not publish 'develop'"), "{err}");
        assert!(err.contains("Saved Work"), "{err}");

        let rec = saved_work_log()
            .unwrap()
            .actionable_entries(rp)
            .unwrap()
            .into_iter()
            .find(|r| r.label == INIT_WORKFLOW_SAVE_LABEL)
            .expect("row");
        assert_eq!(rec.status, "saved", "stash must stay resumable");
        assert!(!git_out(dir.path(), &["stash", "list"]).is_empty());
    }

    #[test]
    fn init_is_idempotent_when_develop_exists() {
        isolate_db();
        let (_origin, work) = repo_with_origin();
        git(work.path(), &["branch", "develop"]);
        let rp = work.path().to_str().unwrap();

        let out = initialize_workflow_inner(rp, &noop).unwrap();
        assert!(!out.develop_created && !out.develop_pushed);
        assert!(out.notes.iter().any(|n| n.contains("already")));
    }

    #[test]
    fn init_rejects_repo_with_no_production_branch() {
        isolate_db();
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "-b", "trunk"]);
        git(dir.path(), &["config", "user.email", "t@e.com"]);
        git(dir.path(), &["config", "user.name", "T"]);
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "init"]);

        let err = initialize_workflow_inner(dir.path().to_str().unwrap(), &noop).unwrap_err();
        assert!(err.contains("No 'main' or 'master'"), "{err}");
    }

    #[tokio::test]
    async fn setup_state_plain_dir_is_not_a_repo() {
        isolate_db();
        let dir = tempdir().unwrap();
        let s = get_setup_state(dir.path().to_str().unwrap().to_string()).await.unwrap();
        assert_eq!(s.phase, "not_a_repo");
        assert!(s.needs_git_init);
    }

    #[tokio::test]
    async fn setup_state_unborn_repo_needs_first_commit() {
        isolate_db();
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        let s = get_setup_state(dir.path().to_str().unwrap().to_string()).await.unwrap();
        assert_eq!(s.phase, "needs_first_commit");
    }

    #[tokio::test]
    async fn setup_state_repo_without_remote_is_preflight_failed() {
        isolate_db();
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "t@e.com"]);
        git(dir.path(), &["config", "user.name", "T"]);
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "init"]);

        let s = get_setup_state(dir.path().to_str().unwrap().to_string()).await.unwrap();
        assert_eq!(s.phase, "preflight_failed");
        assert_eq!(s.checks.len(), 7);
    }

    #[test]
    fn release_branch_classification_and_version_parsing() {
        assert_eq!(classify_branch("release/1.4.0"), BranchClass::Release);
        assert_eq!(classify_branch("release/1.4.0-2"), BranchClass::Release);
        assert_eq!(branch_work_type("release/1.4.0-3").as_deref(), Some("release"));
        assert_eq!(version_from_release_branch("release/1.4.0"), "1.4.0");
        assert_eq!(version_from_release_branch("release/1.4.0-2"), "1.4.0");
        assert_eq!(version_from_release_branch("release/2.0.0-5"), "2.0.0");
    }

    #[test]
    fn release_branch_name_picks_free_dash_n_suffix() {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "-b", "develop"]);
        git(dir.path(), &["config", "user.email", "t@e.com"]);
        git(dir.path(), &["config", "user.name", "T"]);
        std::fs::write(dir.path().join("VERSION"), "1.3.0\n").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "init"]);
        git(dir.path(), &["branch", "release/1.4.0"]);

        let repo = Repository::open(dir.path()).unwrap();
        let mut name = "release/1.4.0".to_string();
        let mut n = 2;
        while git_core::ref_exists(&repo, &name) {
            name = format!("release/1.4.0-{n}");
            n += 1;
        }
        assert_eq!(name, "release/1.4.0-2");
    }

    #[test]
    fn sync_release_tag_needs_a_confirmed_merge() {
        let (_origin, work) = repo_with_origin();
        let rp = work.path().to_str().unwrap();

        let err = sync_release_tag_inner(rp, "release/0.2.3", false).unwrap_err();
        assert!(err.contains("not merged"));
        // nothing was tagged, locally or on origin.
        assert!(Repository::open(work.path())
            .unwrap()
            .revparse_single("refs/tags/v0.2.3")
            .is_err());
    }

    #[test]
    fn sync_release_tag_tags_the_production_tip_once_merged() {
        let (_origin, work) = repo_with_origin();
        let rp = work.path().to_str().unwrap();
        let prod_tip = git_out(work.path(), &["rev-parse", "refs/remotes/origin/main"]);

        let outcome = sync_release_tag_inner(rp, "release/0.2.3-2", true).unwrap();
        assert_eq!(outcome, git_core::TagOutcome::Created);
        // supersede suffix dropped -> plain vX.Y.Z, pointing at origin/main.
        assert_eq!(git_out(work.path(), &["rev-parse", "v0.2.3^{commit}"]), prod_tip);
    }

    #[test]
    fn sync_release_tag_is_idempotent_on_retry() {
        let (_origin, work) = repo_with_origin();
        let rp = work.path().to_str().unwrap();

        sync_release_tag_inner(rp, "release/0.2.3", true).unwrap();
        let first = git_out(work.path(), &["rev-parse", "v0.2.3"]);

        // production moves on; a retry must not re-point the shipped tag.
        std::fs::write(work.path().join("a.txt"), "more\n").unwrap();
        git(work.path(), &["commit", "-am", "more"]);
        git(work.path(), &["push", "origin", "main"]);

        let outcome = sync_release_tag_inner(rp, "release/0.2.3", true).unwrap();
        assert_eq!(outcome, git_core::TagOutcome::AlreadyPresent);
        assert_eq!(git_out(work.path(), &["rev-parse", "v0.2.3"]), first);
    }

    /// Work repo with `origin/main` + `origin/develop`, a shipped release
    /// commit (`chore: release <v>` + `v<v>` tag) on main, and a `release/<v>`
    /// WIP row in `waiting`. `develop_has_release` controls whether develop
    /// already contains that commit.
    fn release_synced_fixture(version: &str, develop_has_release: bool) -> (TempDir, TempDir, String) {
        isolate_db();
        let origin = tempdir().unwrap();
        git(origin.path(), &["init", "--bare", "-b", "main"]);
        let work = tempdir().unwrap();
        let w = work.path();
        git(w, &["init", "-b", "main"]);
        git(w, &["config", "user.email", "t@e.com"]);
        git(w, &["config", "user.name", "T"]);
        std::fs::write(w.join("a.txt"), "a\n").unwrap();
        git(w, &["add", "."]);
        git(w, &["commit", "-m", "init"]);
        git(w, &["branch", "develop"]);
        // ship the release on main
        std::fs::write(w.join("VERSION"), format!("{version}\n")).unwrap();
        git(w, &["add", "."]);
        git(w, &["commit", "-m", &format!("chore: release {version}")]);
        git(w, &["tag", &format!("v{version}")]);
        if develop_has_release {
            git(w, &["checkout", "develop"]);
            git(w, &["merge", "--ff-only", "main"]);
            git(w, &["checkout", "main"]);
        }
        git(w, &["remote", "add", "origin", origin.path().to_str().unwrap()]);
        git(w, &["push", "origin", "main", "develop", "--tags"]);
        git(w, &["fetch", "origin"]);

        let rp = w.to_str().unwrap().to_string();
        let branch = format!("release/{version}");
        wip_item_log().unwrap().start(&rp, &branch, "release").unwrap();
        wip_item_log().unwrap().set_status(&rp, &branch, "waiting").unwrap();
        (origin, work, rp)
    }

    #[test]
    fn reconcile_completes_wip_when_release_already_in_develop() {
        let (_o, work, rp) = release_synced_fixture("0.4.0", true);
        let repo = Repository::discover(work.path()).unwrap();

        let state = reconcile_release_sync(&repo, &rp, "release/0.4.0", "main", true, false);

        assert!(state.reached_develop);
        assert!(state.satisfied);
        assert!(!state.sync_required, "a shipped+synced release must not block");
        let row = wip_item_log().unwrap().by_type(&rp, "release").unwrap();
        assert_eq!(row[0].status, "completed", "stale WIP row reconciled");
    }

    #[test]
    fn reconcile_keeps_sync_required_when_release_not_in_develop() {
        let (_o, work, rp) = release_synced_fixture("0.5.0", false);
        let repo = Repository::discover(work.path()).unwrap();

        // production merged, no merged sync MR, commit not on develop.
        let state = reconcile_release_sync(&repo, &rp, "release/0.5.0", "main", true, false);

        assert!(!state.reached_develop);
        assert!(!state.satisfied);
        assert!(state.sync_required);
        assert_eq!(
            wip_item_log().unwrap().by_type(&rp, "release").unwrap()[0].status,
            "waiting",
            "must not complete a genuinely un-synced release"
        );
    }

    #[test]
    fn reconcile_completes_via_merged_sync_mr_even_if_ancestry_lags() {
        let (_o, work, rp) = release_synced_fixture("0.6.0", false);
        let repo = Repository::discover(work.path()).unwrap();

        // sync MR reported merged -> satisfied even though our local view of
        // develop does not yet show the commit.
        let state = reconcile_release_sync(&repo, &rp, "release/0.6.0", "main", true, true);

        assert!(state.satisfied);
        assert!(!state.sync_required);
        assert_eq!(
            wip_item_log().unwrap().by_type(&rp, "release").unwrap()[0].status,
            "completed"
        );
    }

    #[test]
    fn reconcile_is_inert_until_production_merges() {
        let (_o, work, rp) = release_synced_fixture("0.7.0", true);
        let repo = Repository::discover(work.path()).unwrap();

        let state = reconcile_release_sync(&repo, &rp, "release/0.7.0", "main", false, false);

        assert!(!state.satisfied);
        assert!(!state.sync_required, "nothing owed before production merges");
        assert_eq!(
            wip_item_log().unwrap().by_type(&rp, "release").unwrap()[0].status,
            "waiting"
        );
    }

    // --- Workflow Guard -------------------------------------------------

    fn open_on(work: &Path, branch: &str) -> Repository {
        git(work, &["checkout", branch]);
        Repository::discover(work).unwrap()
    }

    #[test]
    fn protected_branch_classification_matches_branch_class() {
        assert!(branch_is_protected("main"));
        assert!(branch_is_protected("master"));
        assert!(branch_is_protected("develop"));
        assert!(!branch_is_protected("feature/x"));
        assert!(!branch_is_protected("bug/x"));
        assert!(!branch_is_protected("hotfix/x"));
        assert!(!branch_is_protected("release/1.0.0"));
    }

    #[test]
    fn guard_blocks_commit_and_push_on_protected_branches() {
        isolate_db();
        let (_origin, work) = repo_with_origin();
        git(work.path(), &["branch", "develop"]);

        for b in ["main", "develop"] {
            let repo = open_on(work.path(), b);
            let commit_err = reject_protected_branch(&repo, "commit").unwrap_err();
            assert!(commit_err.contains("Workflow Guard"), "{commit_err}");
            assert!(commit_err.contains(b), "{commit_err}");
            assert!(reject_protected_branch(&repo, "push").is_err());
        }
    }

    #[test]
    fn guard_allows_commit_and_push_on_feature_branch() {
        isolate_db();
        let (_origin, work) = repo_with_origin();
        git(work.path(), &["checkout", "-b", "feature/thing"]);
        let repo = Repository::discover(work.path()).unwrap();
        assert!(reject_protected_branch(&repo, "commit").is_ok());
        assert!(reject_protected_branch(&repo, "push").is_ok());
    }

    #[test]
    fn move_changes_from_develop_creates_feature_branch_and_restores() {
        isolate_db();
        let (_origin, work) = repo_with_origin();
        git(work.path(), &["branch", "develop"]);
        git(work.path(), &["checkout", "develop"]);
        std::fs::write(work.path().join("a.txt"), "work in progress\n").unwrap();
        std::fs::write(work.path().join("new.txt"), "brand new\n").unwrap();
        let rp = work.path().to_str().unwrap();

        let (branch, outcome, conflicts) =
            move_changes_to_new_branch_inner(rp, "feature", "rescued").unwrap();

        assert_eq!(branch, "feature/rescued");
        assert_eq!(outcome, "restored");
        assert!(conflicts.is_empty());
        assert_eq!(git_out(work.path(), &["rev-parse", "--abbrev-ref", "HEAD"]), "feature/rescued");
        assert_eq!(std::fs::read_to_string(work.path().join("a.txt")).unwrap(), "work in progress\n");
        assert_eq!(std::fs::read_to_string(work.path().join("new.txt")).unwrap(), "brand new\n");
        // develop's committed tree is untouched (a.txt still the seed content).
        assert_eq!(git_out(work.path(), &["show", "develop:a.txt"]), "a");
    }

    #[test]
    fn move_changes_from_production_bases_on_develop_and_restores() {
        isolate_db();
        let (_origin, work) = repo_with_origin();
        git(work.path(), &["branch", "develop"]);
        // stay on main (production)
        std::fs::write(work.path().join("a.txt"), "hotfix-ish edit\n").unwrap();
        let rp = work.path().to_str().unwrap();

        let (branch, outcome, _) = move_changes_to_new_branch_inner(rp, "bug", "oops").unwrap();

        assert_eq!(branch, "bug/oops");
        assert_eq!(outcome, "restored");
        // new branch descends from develop, not main
        let base = git_out(work.path(), &["merge-base", "HEAD", "develop"]);
        let dev = git_out(work.path(), &["rev-parse", "develop"]);
        assert_eq!(base, dev);
        assert_eq!(std::fs::read_to_string(work.path().join("a.txt")).unwrap(), "hotfix-ish edit\n");
        assert_eq!(git_out(work.path(), &["rev-parse", "--abbrev-ref", "main"]), "main");
    }

    #[test]
    fn move_changes_surfaces_restore_conflict_without_discarding() {
        isolate_db();
        let (_origin, work) = repo_with_origin();
        // develop diverges from main on the same file the user is editing.
        git(work.path(), &["checkout", "-b", "develop"]);
        std::fs::write(work.path().join("a.txt"), "develop version\n").unwrap();
        git(work.path(), &["commit", "-am", "develop edits a"]);
        git(work.path(), &["checkout", "main"]);
        std::fs::write(work.path().join("a.txt"), "my uncommitted edit\n").unwrap();
        let rp = work.path().to_str().unwrap();

        let (branch, outcome, conflicts) =
            move_changes_to_new_branch_inner(rp, "feature", "collide").unwrap();

        assert_eq!(branch, "feature/collide");
        assert_eq!(outcome, "conflict");
        assert!(conflicts.iter().any(|f| f == "a.txt"), "{conflicts:?}");
        // The Saved Work entry is kept for recovery -- nothing discarded.
        let kept = saved_work_log()
            .unwrap()
            .actionable_entries(rp)
            .unwrap()
            .into_iter()
            .any(|r| r.status == "conflict");
        assert!(kept, "conflicted Saved Work entry must be kept");
    }

    #[test]
    fn move_changes_refuses_on_clean_protected_branch() {
        isolate_db();
        let (_origin, work) = repo_with_origin();
        git(work.path(), &["branch", "develop"]);
        let rp = work.path().to_str().unwrap();
        let err = move_changes_to_new_branch_inner(rp, "feature", "nope").unwrap_err();
        assert!(err.contains("clean"), "{err}");
    }

    #[test]
    fn move_changes_refuses_on_feature_branch() {
        isolate_db();
        let (_origin, work) = repo_with_origin();
        git(work.path(), &["checkout", "-b", "feature/already"]);
        std::fs::write(work.path().join("a.txt"), "x\n").unwrap();
        let rp = work.path().to_str().unwrap();
        let err = move_changes_to_new_branch_inner(rp, "feature", "nope").unwrap_err();
        assert!(err.contains("only applies on a protected branch"), "{err}");
    }

    #[test]
    fn github_remote_parses_to_owner_and_repo() {
        for url in [
            "git@github.com:owner/repository.git",
            "https://github.com/owner/repository.git",
            "https://github.com/owner/repository",
        ] {
            assert_eq!(detect_provider(url), Provider::GitHub, "url: {url}");
            assert_eq!(extract_https_host(url).as_deref(), Some("github.com"), "url: {url}");
            let path = extract_project_path(url).expect("path");
            assert_eq!(path.split_once('/'), Some(("owner", "repository")), "url: {url}");
        }
    }

    #[test]
    fn github_api_base_is_used_for_github_dot_com() {
        assert_eq!(github_api_base("github.com"), "https://api.github.com");
        assert_eq!(
            github_api_base("github.enterprise.example"),
            "https://github.enterprise.example/api/v3"
        );
    }

}
