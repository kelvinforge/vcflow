//! Tauri adapter layer. Every command here is a thin wrapper: argument
//! marshalling in, `workflow_service` call, `workflow:state:changed` emit out.
//! No workflow orchestration lives in this file -- see `workflow_service` for
//! snapshot assembly, git/provider/auth orchestration, and safety guards. This
//! is what lets a non-Tauri consumer (the `vcflow` CLI, a future JSON-RPC/VS
//! Code adapter) reuse the exact same workflow implementation.

use tauri::{AppHandle, Emitter};

use workflow_service::{
    AuditEntryDto, CommandLogDto, ConflictInfo, ContinueOutcome, HotfixStatus, InspectionOutcome,
    MoveChangesOutcome, MrStatus, NextActionDto, PreflightDto, ReleasePreview, ReleaseStatusDto,
    RepoStatus, RepoStatusWithPath, ResumeOutcome, RoleOverrideDto, SavedWorkDto, SetupStateDto,
    TokenValidation, VersionPreview, WorkList, WorkflowInitDto,
};

use crate::events::WORKFLOW_STATE_CHANGED;

// --- Read-only status ---------------------------------------------------

#[tauri::command]
pub async fn get_repo_status(repo_path: String) -> Result<RepoStatus, String> {
    workflow_service::get_repo_status(repo_path).await
}

#[tauri::command]
pub async fn refresh_repo_status(repo_path: String) -> Result<RepoStatus, String> {
    workflow_service::refresh_repo_status(repo_path).await
}

/// Read-only: computes the focus-card next step via
/// `workflow_service::get_next_action` (repo + provider state -> the same
/// `workflow_engine::next_action` a future CLI/VS Code adapter also calls --
/// one snapshot-assembly path, not a Tauri-only copy).
#[tauri::command]
pub async fn get_next_action(repo_path: String) -> Result<NextActionDto, String> {
    workflow_service::get_next_action(&repo_path).await
}

// --- Repository preflight + Initial Workflow Setup ----------------------

#[tauri::command]
pub async fn repository_preflight(repo_path: String) -> Result<PreflightDto, String> {
    workflow_service::repository_preflight(repo_path).await
}

/// The setup gate's mutation. Emits `workflow:init:step` at each step so the
/// Setup Card shows live text -- `workflow_service::initialize_workflow` takes
/// a plain `Fn(&str)` closure for that, so this wrapper is the only place that
/// knows about `AppHandle`/Tauri events.
#[tauri::command]
pub async fn initialize_workflow(
    app: AppHandle,
    repo_path: String,
) -> Result<WorkflowInitDto, String> {
    let app_for_emit = app.clone();
    let emit = move |step: &str| {
        let _ = app_for_emit
            .emit(crate::events::WORKFLOW_INIT_STEP, serde_json::json!({ "step": step }));
    };

    let out = workflow_service::initialize_workflow(repo_path.clone(), &emit).await?;

    if let Ok(status) = workflow_service::build_status(&repo_path).await {
        let _ = app.emit(WORKFLOW_STATE_CHANGED, &status);
    }
    Ok(out)
}

#[tauri::command]
pub async fn get_setup_state(repo_path: String) -> Result<SetupStateDto, String> {
    workflow_service::get_setup_state(repo_path).await
}

// --- Credentials ---------------------------------------------------------

#[tauri::command]
pub async fn save_token(repo_path: String, host: String, token: String) -> Result<(), String> {
    workflow_service::save_token(repo_path, host, token).await
}

#[tauri::command]
pub async fn set_remote_url(repo_path: String, url: String) -> Result<(), String> {
    workflow_service::set_remote_url(repo_path, url).await
}

#[tauri::command]
pub async fn re_validate_token(repo_path: String) -> Result<TokenValidation, String> {
    workflow_service::re_validate_token(repo_path).await
}

#[tauri::command]
pub async fn delete_token(repo_path: String, host: String) -> Result<(), String> {
    workflow_service::delete_token(repo_path, host).await
}

// --- Work items ------------------------------------------------------------

#[tauri::command]
pub async fn create_work_item(
    app: AppHandle,
    repo_path: String,
    kind: String,
    slug: String,
) -> Result<RepoStatus, String> {
    let status = workflow_service::create_work_item(repo_path, kind, slug).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &status).map_err(|e| e.to_string())?;
    Ok(status)
}

#[tauri::command]
pub async fn move_changes_to_new_branch(
    app: AppHandle,
    repo_path: String,
    kind: String,
    slug: String,
) -> Result<MoveChangesOutcome, String> {
    let result = workflow_service::move_changes_to_new_branch(repo_path, kind, slug).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &result.status).map_err(|e| e.to_string())?;
    Ok(result)
}

#[tauri::command]
pub async fn commit_work_item(
    app: AppHandle,
    repo_path: String,
    message: String,
) -> Result<RepoStatus, String> {
    let status = workflow_service::commit_work_item(repo_path, message).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &status).map_err(|e| e.to_string())?;
    Ok(status)
}

#[tauri::command]
pub async fn push_work_item(app: AppHandle, repo_path: String) -> Result<RepoStatus, String> {
    let status = workflow_service::push_work_item(repo_path).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &status).map_err(|e| e.to_string())?;
    Ok(status)
}

#[tauri::command]
pub async fn finish_work_item(
    app: AppHandle,
    repo_path: String,
    title: String,
) -> Result<RepoStatus, String> {
    let status = workflow_service::finish_work_item(repo_path, title).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &status).map_err(|e| e.to_string())?;
    Ok(status)
}

#[tauri::command]
pub async fn get_mr_status(repo_path: String) -> Result<Option<MrStatus>, String> {
    workflow_service::get_mr_status(repo_path).await
}

// --- Hotfix ------------------------------------------------------------

#[tauri::command]
pub async fn create_hotfix(
    app: AppHandle,
    repo_path: String,
    slug: String,
) -> Result<RepoStatusWithPath, String> {
    let result = workflow_service::create_hotfix(repo_path, slug).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &result.status).map_err(|e| e.to_string())?;
    Ok(result)
}

#[tauri::command]
pub async fn finish_hotfix(
    app: AppHandle,
    repo_path: String,
    title: String,
) -> Result<RepoStatusWithPath, String> {
    let result = workflow_service::finish_hotfix(repo_path, title).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &result.status).map_err(|e| e.to_string())?;
    Ok(result)
}

#[tauri::command]
pub async fn get_hotfix_status(repo_path: String) -> Result<Option<HotfixStatus>, String> {
    workflow_service::get_hotfix_status(repo_path).await
}

// --- Saved work ----------------------------------------------------------

#[tauri::command]
pub async fn save_work(app: AppHandle, repo_path: String) -> Result<RepoStatus, String> {
    let status = workflow_service::save_work(repo_path).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &status).map_err(|e| e.to_string())?;
    Ok(status)
}

#[tauri::command]
pub fn list_saved_work(repo_path: String) -> Result<Vec<SavedWorkDto>, String> {
    workflow_service::list_saved_work(repo_path)
}

#[tauri::command]
pub async fn resume_work(app: AppHandle, repo_path: String, id: i64) -> Result<ResumeOutcome, String> {
    let result = workflow_service::resume_work(repo_path, id).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &result.status).map_err(|e| e.to_string())?;
    Ok(result)
}

#[tauri::command]
pub async fn discard_work(app: AppHandle, repo_path: String, id: i64) -> Result<RepoStatus, String> {
    let status = workflow_service::discard_work(repo_path, id).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &status).map_err(|e| e.to_string())?;
    Ok(status)
}

// --- Work-in-progress items (branch continuation) ------------------------

#[tauri::command]
pub async fn list_work_items(repo_path: String) -> Result<WorkList, String> {
    workflow_service::list_work_items(repo_path).await
}

#[tauri::command]
pub async fn continue_work(
    app: AppHandle,
    repo_path: String,
    work_item_id: i64,
) -> Result<ContinueOutcome, String> {
    let result = workflow_service::continue_work(repo_path, work_item_id).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &result.status).map_err(|e| e.to_string())?;
    Ok(result)
}

#[tauri::command]
pub async fn inspect_branch(
    app: AppHandle,
    repo_path: String,
    target: String,
) -> Result<InspectionOutcome, String> {
    let result = workflow_service::inspect_branch(repo_path, target).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &result.status).map_err(|e| e.to_string())?;
    Ok(result)
}

#[tauri::command]
pub async fn end_branch_inspection(
    app: AppHandle,
    repo_path: String,
    original_branch: String,
    saved_work_id: Option<i64>,
) -> Result<ResumeOutcome, String> {
    let result =
        workflow_service::end_branch_inspection(repo_path, original_branch, saved_work_id).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &result.status).map_err(|e| e.to_string())?;
    Ok(result)
}

#[tauri::command]
pub async fn drop_work(
    repo_path: String,
    work_item_id: i64,
    confirmation: String,
) -> Result<WorkList, String> {
    workflow_service::drop_work(repo_path, work_item_id, confirmation).await
}

// --- Conflict resolution ---------------------------------------------------

#[tauri::command]
pub async fn start_conflict_resolution(
    repo_path: String,
    target_branch: String,
) -> Result<ConflictInfo, String> {
    workflow_service::start_conflict_resolution(repo_path, target_branch).await
}

/// Owner-only: launches the Owner's own configured `git mergetool` in the
/// project working directory, falling back to the OS file-manager opener when
/// none is configured. Per spec, v1 builds no custom merge editor. Left as a
/// Tauri command (not moved to `workflow_service`): it only decides which OS
/// process to spawn, which a headless CLI would want to do exactly the same
/// way -- there is no workflow orchestration here to duplicate.
#[tauri::command]
pub async fn open_in_external_tool(repo_path: String) -> Result<(), String> {
    workflow_service::require_owner(&repo_path).await?;

    workflow_service::conflict_log()
        .and_then(|log| log.current().ok().flatten())
        .ok_or("no conflict resolution in progress")?;

    let has_mergetool = std::process::Command::new("git")
        .args(["config", "--get", "merge.tool"])
        .current_dir(&repo_path)
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);

    if has_mergetool {
        return std::process::Command::new("git")
            .args(["mergetool", "--no-prompt"])
            .current_dir(&repo_path)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string());
    }

    os_open(&repo_path)
}

#[tauri::command]
pub async fn verify_and_commit_resolution(
    app: AppHandle,
    repo_path: String,
) -> Result<RepoStatus, String> {
    let status = workflow_service::verify_and_commit_resolution(repo_path).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &status).map_err(|e| e.to_string())?;
    Ok(status)
}

// --- Role overrides + audit/command log -----------------------------------

#[tauri::command]
pub async fn list_role_overrides(repo_path: String) -> Result<Vec<RoleOverrideDto>, String> {
    workflow_service::list_role_overrides(repo_path).await
}

#[tauri::command]
pub async fn set_role_override(
    repo_path: String,
    user: String,
    repository: String,
    role: String,
) -> Result<(), String> {
    workflow_service::set_role_override(repo_path, user, repository, role).await
}

#[tauri::command]
pub async fn remove_role_override(repo_path: String, user: String, repository: String) -> Result<(), String> {
    workflow_service::remove_role_override(repo_path, user, repository).await
}

#[tauri::command]
pub fn get_audit_log(limit: u32) -> Result<Vec<AuditEntryDto>, String> {
    workflow_service::get_audit_log(limit)
}

#[tauri::command]
pub fn get_command_log(repo_path: String, limit: u32) -> Result<Vec<CommandLogDto>, String> {
    workflow_service::get_command_log(repo_path, limit)
}

// --- Release workflow --------------------------------------------------

#[tauri::command]
pub fn get_hotfix_version_preview(repo_path: String) -> Result<VersionPreview, String> {
    workflow_service::get_hotfix_version_preview(repo_path)
}

#[tauri::command]
pub async fn get_release_preview(repo_path: String) -> Result<ReleasePreview, String> {
    workflow_service::get_release_preview(repo_path).await
}

#[tauri::command]
pub async fn create_release_candidate(
    app: AppHandle,
    repo_path: String,
    version: String,
    changelog_body: String,
    supersede_confirmed: bool,
) -> Result<RepoStatusWithPath, String> {
    let result = workflow_service::create_release_candidate(
        repo_path,
        version,
        changelog_body,
        supersede_confirmed,
    )
    .await?;
    app.emit(WORKFLOW_STATE_CHANGED, &result.status).map_err(|e| e.to_string())?;
    Ok(result)
}

#[tauri::command]
pub async fn finish_release(
    app: AppHandle,
    repo_path: String,
    title: String,
) -> Result<RepoStatusWithPath, String> {
    let result = workflow_service::finish_release(repo_path, title).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &result.status).map_err(|e| e.to_string())?;
    Ok(result)
}

#[tauri::command]
pub async fn sync_develop_after_release(
    app: AppHandle,
    repo_path: String,
    candidate_branch: String,
    title: String,
) -> Result<RepoStatus, String> {
    let status =
        workflow_service::sync_develop_after_release(repo_path, candidate_branch, title).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &status).map_err(|e| e.to_string())?;
    Ok(status)
}

#[tauri::command]
pub async fn get_release_status(repo_path: String) -> Result<Option<ReleaseStatusDto>, String> {
    workflow_service::get_release_status(repo_path).await
}

#[tauri::command]
pub async fn update_branch(app: AppHandle, repo_path: String) -> Result<RepoStatus, String> {
    let status = workflow_service::update_branch(repo_path).await?;
    app.emit(WORKFLOW_STATE_CHANGED, &status).map_err(|e| e.to_string())?;
    Ok(status)
}

// --- OS integration (not workflow logic) ------------------------------------

/// UI integration (not workflow logic): open the repo's working directory in
/// the OS file manager.
#[tauri::command]
pub fn open_working_directory(repo_path: String) -> Result<(), String> {
    let repo = git2::Repository::discover(&repo_path).map_err(|e| e.to_string())?;
    let dir = repo
        .workdir()
        .ok_or("repository has no working directory")?
        .to_string_lossy()
        .into_owned();
    os_open(&dir)
}

/// UI integration (not workflow logic): open an http(s) URL (e.g. an MR page)
/// in the OS default browser. Rejects anything that isn't a plain http/https
/// URL -- the value is passed to the opener as a single argument, never a
/// shell string.
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    let ok_scheme = url.starts_with("https://") || url.starts_with("http://");
    let host = url
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""))
        .unwrap_or("");
    let clean = !url.contains(|c: char| c.is_whitespace() || c.is_control());
    if !ok_scheme || host.is_empty() || !clean {
        return Err("only a plain http(s) URL can be opened".to_string());
    }
    os_open(&url)
}

/// Spawns the OS "open this path/URL" helper with `arg` as a single argument
/// (no shell). Shared by `open_working_directory`, `open_url`, and the
/// conflict tool fallback.
fn os_open(arg: &str) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(arg)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}
