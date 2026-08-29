// Thin bindings over the Tauri command layer. This file is the ONLY place
// the frontend touches the backend. No workflow logic lives here -- every
// function is a 1:1 pass-through to a registered command. The backend is the
// single source of truth for workflow state and next actions.

import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"

// --- RepoStatus ------------------------------------------------------------

export interface RepoStatus {
  branch: string
  version: string | null
  remote_url: string | null
  provider: string
  ssh_ok: boolean
  ssh_error: string | null
  /** GitLab API reachable AND the saved token authenticated. */
  gitlab_ok: boolean
  gitlab_error: string | null
  role: string
  /** Working tree has uncommitted changes (tracked or untracked). */
  dirty: boolean
  dirty_count: number
  /** "merge" / "rebase" / "cherry-pick" / ... when a git op is half-finished. */
  in_progress_op: string | null
  ahead: number
  behind: number
  diverged: boolean
  /** The repo's production branch: "main" or "master", whichever exists. */
  production_branch: string
}

export interface RepoStatusWithPath {
  status: RepoStatus
  repo_path: string
}

export function getRepoStatus(repoPath: string): Promise<RepoStatus> {
  return invoke<RepoStatus>("get_repo_status", { repoPath })
}

/** fetch origin + recompute. Safe to call repeatedly. */
export function refreshRepoStatus(repoPath: string): Promise<RepoStatus> {
  return invoke<RepoStatus>("refresh_repo_status", { repoPath })
}

// --- NextAction (the workflow contract) -----------------------------------

export type PrimaryActionId =
  | "resolve_in_working_dir"
  | "resolve_mr_conflict"
  | "commit"
  | "finish"
  | "finish_hotfix"
  | "return_to_develop"
  | "start_work_item"

export interface NextActionDto {
  title: string
  description: string
  /** snake_case action id, or null when there is nothing for this user to do. */
  primary: PrimaryActionId | null
  helper: string | null
}

export function getNextAction(repoPath: string): Promise<NextActionDto> {
  return invoke<NextActionDto>("get_next_action", { repoPath })
}

// --- Work items ----------------------------------------------------------

export type WorkItemKind = "feature" | "bug" | "chore"

export function createWorkItem(
  repoPath: string,
  kind: WorkItemKind,
  slug: string,
): Promise<RepoStatus> {
  return invoke<RepoStatus>("create_work_item", { repoPath, kind, slug })
}

export function commitWorkItem(repoPath: string, message: string): Promise<RepoStatus> {
  return invoke<RepoStatus>("commit_work_item", { repoPath, message })
}

export function pushWorkItem(repoPath: string): Promise<RepoStatus> {
  return invoke<RepoStatus>("push_work_item", { repoPath })
}

export function finishWorkItem(repoPath: string, title: string): Promise<RepoStatus> {
  return invoke<RepoStatus>("finish_work_item", { repoPath, title })
}

// --- MR / Hotfix status --------------------------------------------------

export interface MrStatus {
  id: string
  web_url: string
  status: string
  mergeability: string
}

export interface HotfixStatus {
  master: MrStatus | null
  develop: MrStatus | null
}

/** null => no real MR exists (do NOT show "Handoff Complete"). */
export function getMrStatus(repoPath: string): Promise<MrStatus | null> {
  return invoke<MrStatus | null>("get_mr_status", { repoPath })
}

export function getHotfixStatus(repoPath: string): Promise<HotfixStatus | null> {
  return invoke<HotfixStatus | null>("get_hotfix_status", { repoPath })
}

export function createHotfix(repoPath: string, slug: string): Promise<RepoStatusWithPath> {
  return invoke<RepoStatusWithPath>("create_hotfix", { repoPath, slug })
}

export function finishHotfix(repoPath: string, title: string): Promise<RepoStatusWithPath> {
  return invoke<RepoStatusWithPath>("finish_hotfix", { repoPath, title })
}

export interface VersionPreview {
  current_version: string
  next_version: string
}

export function getHotfixVersionPreview(repoPath: string): Promise<VersionPreview> {
  return invoke<VersionPreview>("get_hotfix_version_preview", { repoPath })
}

// --- Saved Work (Work Safe) --------------------------------------------

export type SavedWorkStatus = "saved" | "conflict" | "restored" | "discarded"

export interface SavedWorkDto {
  id: number
  repo: string
  original_branch: string
  original_commit: string
  label: string
  created_at: string
  status: SavedWorkStatus
}

export interface ResumeOutcome {
  /** "restored" (applied + dropped) or "conflict" (kept, markers in workdir). */
  outcome: "restored" | "conflict"
  conflicting_files: string[]
  status: RepoStatus
}

export function saveWork(repoPath: string): Promise<RepoStatus> {
  return invoke<RepoStatus>("save_work", { repoPath })
}

export function listSavedWork(repoPath: string): Promise<SavedWorkDto[]> {
  return invoke<SavedWorkDto[]>("list_saved_work", { repoPath })
}

export function resumeWork(repoPath: string, id: number): Promise<ResumeOutcome> {
  return invoke<ResumeOutcome>("resume_work", { repoPath, id })
}

export function discardWork(repoPath: string, id: number): Promise<RepoStatus> {
  return invoke<RepoStatus>("discard_work", { repoPath, id })
}

// --- Work-in-progress items (branch continuation) --------------------

export type WipStatus = "active" | "waiting"

export interface WipItemDto {
  id: number
  branch: string
  work_type: string
  status: WipStatus
  created_at: string
  is_current: boolean
  has_saved_work: boolean
}

export interface WorkList {
  current: WipItemDto | null
  other: WipItemDto[]
  waiting: WipItemDto[]
}

export interface ContinueOutcome {
  status: RepoStatus
  /** The branch's saved work as it was before the auto-restore, or null. */
  saved_work: SavedWorkDto | null
  /** "restored" | "conflict" | "none" | "error" — what the auto-restore did. */
  restore_outcome: "restored" | "conflict" | "none" | "error"
  conflicting_files: string[]
}

export function listWorkItems(repoPath: string): Promise<WorkList> {
  return invoke<WorkList>("list_work_items", { repoPath })
}

export function continueWork(repoPath: string, workItemId: number): Promise<ContinueOutcome> {
  return invoke<ContinueOutcome>("continue_work", { repoPath, workItemId })
}

// --- Temporary branch inspection (develop / production branch) ----------

/** "develop" or the repo's production branch (main/master) — resolved at
 *  runtime from RepoStatus.production_branch, not a fixed union. */
export type InspectTarget = string

export interface InspectionOutcome {
  status: RepoStatus
  /** Branch to return to when inspection ends. */
  original_branch: string
  /** Saved Work row id if entering inspection stashed a dirty tree. */
  saved_work_id: number | null
}

export interface EndInspectionOutcome {
  /** "restored" | "conflict" | "none" — what happened to the stashed work. */
  outcome: "restored" | "conflict" | "none"
  conflicting_files: string[]
  status: RepoStatus
}

export function inspectBranch(
  repoPath: string,
  target: InspectTarget,
): Promise<InspectionOutcome> {
  return invoke<InspectionOutcome>("inspect_branch", { repoPath, target })
}

export function endBranchInspection(
  repoPath: string,
  originalBranch: string,
  savedWorkId: number | null,
): Promise<EndInspectionOutcome> {
  return invoke<EndInspectionOutcome>("end_branch_inspection", {
    repoPath,
    originalBranch,
    savedWorkId,
  })
}

/** `confirmation` must equal the branch name or the backend rejects it. */
export function dropWork(
  repoPath: string,
  workItemId: number,
  confirmation: string,
): Promise<WorkList> {
  return invoke<WorkList>("drop_work", { repoPath, workItemId, confirmation })
}

// --- Conflict resolution (Owner-only) ---------------------------------

export interface ConflictInfo {
  branch: string
  target_branch: string
  conflicting_files: string[]
}

export function startConflictResolution(
  repoPath: string,
  targetBranch: string,
): Promise<ConflictInfo> {
  return invoke<ConflictInfo>("start_conflict_resolution", { repoPath, targetBranch })
}

export function openInExternalTool(repoPath: string): Promise<void> {
  return invoke<void>("open_in_external_tool", { repoPath })
}

export function verifyAndCommitResolution(repoPath: string): Promise<RepoStatus> {
  return invoke<RepoStatus>("verify_and_commit_resolution", { repoPath })
}

// --- Token / capabilities -------------------------------------------

export interface CapabilityItem {
  label: string
  status: "yes" | "no" | "unknown"
  reason: string
}

export interface TokenValidation {
  capabilities: CapabilityItem[]
}

/** Saves a PAT to the OS keychain. The backend picks the GitHub vs GitLab
 *  credential path from the repo's remote -- the frontend never sees or
 *  stores the token. */
export function saveToken(repoPath: string, host: string, token: string): Promise<void> {
  return invoke<void>("save_token", { repoPath, host, token })
}

export function reValidateToken(repoPath: string): Promise<TokenValidation> {
  return invoke<TokenValidation>("re_validate_token", { repoPath })
}

export function deleteToken(repoPath: string, host: string): Promise<void> {
  return invoke<void>("delete_token", { repoPath, host })
}

// --- Role overrides (Owner-only) -----------------------------------

export interface RoleOverrideDto {
  user: string
  repository: string
  role: string
}

export function listRoleOverrides(repoPath: string): Promise<RoleOverrideDto[]> {
  return invoke<RoleOverrideDto[]>("list_role_overrides", { repoPath })
}

export function setRoleOverride(
  repoPath: string,
  user: string,
  repository: string,
  role: string,
): Promise<void> {
  return invoke<void>("set_role_override", { repoPath, user, repository, role })
}

export function removeRoleOverride(
  repoPath: string,
  user: string,
  repository: string,
): Promise<void> {
  return invoke<void>("remove_role_override", { repoPath, user, repository })
}

// --- Logs ---------------------------------------------------------

export interface AuditEntryDto {
  timestamp: string
  user: string
  repository: string
  branch: string | null
  action: string
  result: string
  error: string | null
}

export function getAuditLog(limit: number): Promise<AuditEntryDto[]> {
  return invoke<AuditEntryDto[]>("get_audit_log", { limit })
}

export interface CommandLogDto {
  timestamp: string
  operation: string
  args: string
  outcome: string
  duration_ms: number
  error: string | null
}

export function getCommandLog(repoPath: string, limit: number): Promise<CommandLogDto[]> {
  return invoke<CommandLogDto[]>("get_command_log", { repoPath, limit })
}

// --- OS integration ----------------------------------------------

export function openWorkingDirectory(repoPath: string): Promise<void> {
  return invoke<void>("open_working_directory", { repoPath })
}

export function openUrl(url: string): Promise<void> {
  return invoke<void>("open_url", { url })
}

/** Native OS folder picker. Returns the chosen path, or null if cancelled. */
export async function pickRepo(): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog")
  const picked = await open({ directory: true, multiple: false })
  return typeof picked === "string" ? picked : null
}

// --- Setup gate (Preflight + Initial Workflow, in front of the workflow) ---

export type SetupPhase =
  | "not_a_repo"
  | "needs_first_commit"
  | "preflight_failed"
  | "needs_initial_workflow"
  | "recover"
  | "ready"

export interface CheckDto {
  id: string
  /** "pass" | "warning" | "fail". */
  status: string
  title: string
  message: string
  blocking: boolean
}

export interface SetupStateDto {
  phase: SetupPhase
  /** The 7 preflight rows -- always present. */
  checks: CheckDto[]
  needs_git_init: boolean
  /** Working tree dirty (only meaningful for "needs_initial_workflow"). */
  dirty: boolean
  /** Interrupted-init Saved Work awaiting restore ("recover" phase). */
  recover_entries: SavedWorkDto[]
  notes: string[]
}

/** Derived setup phase. Read-only, not on the poll timers. */
export function getSetupState(repoPath: string): Promise<SetupStateDto> {
  return invoke<SetupStateDto>("get_setup_state", { repoPath })
}

export interface WorkflowInitDto {
  final_branch: string
  develop_created: boolean
  develop_pushed: boolean
  /** Saved Work label when a dirty tree was stashed, else null. */
  saved_work_label: string | null
  restored: boolean
  conflicts: string[]
  notes: string[]
}

export function initializeWorkflow(repoPath: string): Promise<WorkflowInitDto> {
  return invoke<WorkflowInitDto>("initialize_workflow", { repoPath })
}

/** Live step text emitted during initializeWorkflow ("Saving your work…"). */
export function onWorkflowInitStep(handler: (step: string) => void) {
  return listen<{ step: string }>("workflow:init:step", (e) => handler(e.payload.step))
}

// --- Events -----------------------------------------------------

/**
 * "workflow state may have changed -- re-read the backend". NOT a filesystem
 * watcher, and the payload is not authoritative: on this event, re-run the
 * refresh cycle rather than trusting what it carries.
 */
export function onWorkflowStateChanged(handler: () => void) {
  return listen("workflow:state:changed", () => handler())
}
