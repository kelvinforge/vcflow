use crate::state::{Role, WorkItemState};

/// Everything `next_action` needs, assembled by the command layer from
/// `git_core::RepositoryState` + the tracked work item + the provider MR.
/// Pure inputs -- no repo handle, no network, no clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSnapshot {
    pub role: Role,
    pub branch: BranchClass,
    pub work_item: WorkItemState,
    /// Label of a half-finished git op (`merge`/`rebase`/...), if any.
    pub in_progress_op: Option<String>,
    pub dirty: bool,
    /// Local commits on the current branch that `origin/<branch>` lacks.
    pub ahead: usize,
    /// Commits on `origin/<branch>` the local branch lacks -- a plain
    /// fast-forward pull catches up (only actioned for develop/production).
    pub behind: usize,
    /// Current branch vs `origin/<branch>`: both sides carry unique commits.
    /// Work Safe: never auto-resolved.
    pub diverged: bool,
    pub mr: Option<MrSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchClass {
    /// `feature/*`, `bug/*`, `chore/*`.
    WorkItem,
    Hotfix,
    /// `release/x.y.z[-N]` -- a short-lived release-preparation branch. Not a
    /// development branch: it carries only VERSION + CHANGELOG.
    Release,
    Develop,
    Master,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MrSnapshot {
    pub merged: bool,
    pub conflicted: bool,
}

/// The single thing the focus card shows: "what do I do now".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextAction {
    pub title: String,
    pub description: String,
    /// `None` => nothing for this user to do now (waiting on a review, or on
    /// the Owner).
    pub primary: Option<PrimaryAction>,
    pub helper: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryAction {
    /// A half-finished git op or a real divergence -- the user resolves it in
    /// the working directory. The engine never does it for them, so this is a
    /// "go do it" pointer, not a one-click. (Work Safe.)
    ResolveInWorkingDir,
    /// Owner-only: start conflict resolution for the MR.
    ResolveMrConflict,
    Commit,
    /// Push the current branch to `origin` (follow-up commits onto an open MR).
    Push,
    /// Push + open the work-item MR into `develop`, in one gated step.
    Finish,
    /// Push + open the hotfix MR into `master` (and the `master -> develop`
    /// sync MR), in one gated step. Distinct from `Finish` so the frontend
    /// never has to inspect the branch prefix to pick the command.
    FinishHotfix,
    /// Push + open the `release/x.y.z -> production` MR, in one gated step.
    /// The `production -> develop` sync is a separate action (`SyncDevelop`),
    /// surfaced only after a candidate actually merges.
    FinishRelease,
    /// Owner-only: open the `production -> develop` sync MR after a release
    /// candidate merged. Never emitted by `next_action` -- the Active Release
    /// panel drives it; the variant exists so the command layer has one id.
    SyncDevelop,
    ReturnToDevelop,
    /// Fast-forward the current branch to `origin/<branch>`. Only offered on
    /// develop / production when it is behind, clean, and not diverged -- a
    /// pure pull that never touches uncommitted work.
    UpdateBranch,
    StartWorkItem,
    /// Workflow Guard: uncommitted changes on a protected branch
    /// (`main`/`master`/`develop`). Stash the changes, branch off `develop`,
    /// and re-apply them there. (Work Safe -- nothing is discarded.)
    MoveToNewBranch,
}

/// Pure decision: given the observed state, the one recommended next step.
/// Ordered by urgency -- a blocked working tree is surfaced before any
/// workflow progress.
pub fn next_action(s: &WorkflowSnapshot) -> NextAction {
    if let Some(op) = &s.in_progress_op {
        return NextAction {
            title: format!("Finish the {op} in progress"),
            description: format!(
                "A git {op} is half-done in your working directory. Complete or abort it there -- \
                 this tool never resolves it for you."
            ),
            primary: Some(PrimaryAction::ResolveInWorkingDir),
            helper: None,
        };
    }

    if s.diverged {
        return NextAction {
            title: "Your branch has diverged from origin".into(),
            description: "Local and remote both have commits the other lacks. Reconcile it in your \
                          working directory -- this tool never force-pushes or resets for you."
                .into(),
            primary: Some(PrimaryAction::ResolveInWorkingDir),
            helper: None,
        };
    }

    // Workflow Guard: uncommitted changes on a protected branch. `main`/`master`
    // is a hard "not allowed"; `develop` is a warning. Both route to the same
    // one-click recovery (stash -> branch off develop -> re-apply). The
    // command-layer guards in the Tauri layer are what actually block the
    // commit/push -- this is the explain-and-recover surface.
    if s.dirty && matches!(s.branch, BranchClass::Master | BranchClass::Develop) {
        let (title, description) = if s.branch == BranchClass::Master {
            (
                "Production branch is protected",
                "You can't commit or push on the production branch. Move your changes to a feature \
                 branch and keep working there.",
            )
        } else {
            (
                "develop is protected",
                "You can't commit or push on develop. Move your changes to a feature branch and \
                 keep working there.",
            )
        };
        return NextAction {
            title: title.into(),
            description: description.into(),
            primary: Some(PrimaryAction::MoveToNewBranch),
            helper: Some("Your changes move with you — nothing is lost.".into()),
        };
    }

    let conflicted = s.work_item == WorkItemState::Conflicted
        || s.mr.map(|m| m.conflicted).unwrap_or(false);
    if conflicted {
        return match s.role {
            Role::Owner => NextAction {
                title: "Resolve the merge conflict".into(),
                description: "The merge request can't merge cleanly. Resolve it in the project \
                              working directory, verify, then commit back to the same branch."
                    .into(),
                primary: Some(PrimaryAction::ResolveMrConflict),
                helper: None,
            },
            Role::Member => NextAction {
                title: "Conflict -- waiting for the Owner".into(),
                description: "The merge request has a conflict. Only the Owner resolves conflicts; \
                              there is nothing for you to do here."
                    .into(),
                primary: None,
                helper: None,
            },
        };
    }

    if let Some(mr) = s.mr {
        if mr.merged {
            // The branch still exists and carries new work: let them land it as
            // a follow-up. `finish` opens a fresh MR (the tracked MR pointer is
            // overwritten), so this is a normal commit -> finish cycle again.
            if s.dirty {
                return NextAction {
                    title: "Commit your follow-up changes".into(),
                    description: "This branch's merge request is already merged, but there is new \
                                  uncommitted work here. Commit it, then finish to open a new \
                                  merge request into develop."
                        .into(),
                    primary: Some(PrimaryAction::Commit),
                    helper: None,
                };
            }
            if s.ahead > 0 {
                return NextAction {
                    title: "Finish the follow-up work".into(),
                    description: "This branch's merge request is already merged, but there are new \
                                  local commits. Push the branch and open a new merge request into \
                                  develop."
                        .into(),
                    primary: Some(PrimaryAction::Finish),
                    helper: None,
                };
            }
            return NextAction {
                title: "Merged".into(),
                description: "Your merge request is merged. Return to develop to start the next \
                              piece of work."
                    .into(),
                primary: Some(PrimaryAction::ReturnToDevelop),
                helper: None,
            };
        }
    }

    // Keep develop / production current: if HEAD is one of them, behind origin,
    // clean and not diverged, a fast-forward pull is the next step -- before
    // starting any work off a stale base.
    if matches!(s.branch, BranchClass::Develop | BranchClass::Master)
        && s.behind > 0
        && !s.dirty
    {
        let name = if s.branch == BranchClass::Develop { "develop" } else { "the production branch" };
        return NextAction {
            title: format!("Update {name}"),
            description: format!(
                "{name} is {} commit(s) behind origin. Fast-forward it so new work starts from \
                 the current base.",
                s.behind
            ),
            primary: Some(PrimaryAction::UpdateBranch),
            helper: None,
        };
    }

    // A release-preparation branch has only three states here: a half-done op
    // or divergence (handled above), a dirty tree (product code that doesn't
    // belong -- no Commit offered), or clean and ready to finish.
    if s.branch == BranchClass::Release {
        if s.dirty {
            return NextAction {
                title: "Release branches carry only VERSION and CHANGELOG".into(),
                description: "There are uncommitted changes on this release branch. Product code \
                              belongs on a bug/* branch off develop, not here -- this tool will \
                              not commit it on a release branch."
                    .into(),
                primary: None,
                helper: Some(
                    "Save your work, switch back to develop, and start a bug/* branch for it."
                        .into(),
                ),
            };
        }
        return NextAction {
            title: "Finish the release".into(),
            description: "VERSION and CHANGELOG are committed. Push the branch and open the \
                          release merge request into the production branch. The \
                          production -> develop sync is a separate step once it merges."
                .into(),
            primary: Some(PrimaryAction::FinishRelease),
            helper: None,
        };
    }

    match s.work_item {
        WorkItemState::PushedForReview => {
            if s.dirty {
                NextAction {
                    title: "Commit your follow-up changes".into(),
                    description: "You have new uncommitted work on this branch while the merge \
                                  request is open. Commit it, then push to update the MR."
                        .into(),
                    primary: Some(PrimaryAction::Commit),
                    helper: None,
                }
            } else if s.ahead > 0 {
                NextAction {
                    title: "Push your follow-up commits".into(),
                    description: "You have local commits the open merge request doesn't have yet. \
                                  Push the branch to update it."
                        .into(),
                    primary: Some(PrimaryAction::Push),
                    helper: None,
                }
            } else {
                NextAction {
                    title: "Waiting for review".into(),
                    description: "Your merge request is open. Wait for review and pipeline to pass."
                        .into(),
                    primary: None,
                    helper: None,
                }
            }
        }
        WorkItemState::Developing => {
            if s.dirty {
                NextAction {
                    title: "Commit your changes".into(),
                    description: "You have uncommitted work on this branch. Commit it, then finish \
                                  the work item."
                        .into(),
                    primary: Some(PrimaryAction::Commit),
                    helper: None,
                }
            } else if s.branch == BranchClass::Hotfix {
                NextAction {
                    title: "Finish the hotfix".into(),
                    description: "Everything is committed. Push the branch and open the hotfix \
                                  merge request into master (the master -> develop sync MR is \
                                  opened alongside it)."
                        .into(),
                    primary: Some(PrimaryAction::FinishHotfix),
                    helper: None,
                }
            } else {
                NextAction {
                    title: "Finish the work item".into(),
                    description: "Everything is committed. Push the branch and open the merge \
                                  request into develop."
                        .into(),
                    primary: Some(PrimaryAction::Finish),
                    helper: None,
                }
            }
        }
        WorkItemState::NotStarted | WorkItemState::Conflicted => NextAction {
            title: "Start a work item".into(),
            description: "Create a feature, bug, or chore branch off develop.".into(),
            primary: Some(PrimaryAction::StartWorkItem),
            helper: s
                .dirty
                .then(|| "Your current changes are saved automatically first.".to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> WorkflowSnapshot {
        WorkflowSnapshot {
            role: Role::Member,
            branch: BranchClass::WorkItem,
            work_item: WorkItemState::Developing,
            in_progress_op: None,
            dirty: false,
            ahead: 0,
            behind: 0,
            diverged: false,
            mr: None,
        }
    }

    #[test]
    fn workflow_guard_blocks_dirty_production() {
        let s = WorkflowSnapshot {
            branch: BranchClass::Master,
            work_item: WorkItemState::NotStarted,
            dirty: true,
            ..base()
        };
        let a = next_action(&s);
        assert_eq!(a.primary, Some(PrimaryAction::MoveToNewBranch));
        assert_eq!(a.title, "Production branch is protected");
    }

    #[test]
    fn workflow_guard_warns_dirty_develop() {
        let s = WorkflowSnapshot {
            branch: BranchClass::Develop,
            work_item: WorkItemState::NotStarted,
            dirty: true,
            ..base()
        };
        let a = next_action(&s);
        assert_eq!(a.primary, Some(PrimaryAction::MoveToNewBranch));
        assert_eq!(a.title, "develop is protected");
    }

    #[test]
    fn workflow_guard_ignores_clean_protected_branch() {
        let s = WorkflowSnapshot {
            branch: BranchClass::Master,
            work_item: WorkItemState::NotStarted,
            dirty: false,
            ..base()
        };
        assert_ne!(next_action(&s).primary, Some(PrimaryAction::MoveToNewBranch));
    }

    #[test]
    fn workflow_guard_does_not_touch_feature_branches() {
        let s = WorkflowSnapshot { branch: BranchClass::WorkItem, dirty: true, ..base() };
        assert_eq!(next_action(&s).primary, Some(PrimaryAction::Commit));
    }

    #[test]
    fn in_progress_op_beats_everything() {
        let s = WorkflowSnapshot {
            in_progress_op: Some("rebase".into()),
            dirty: true,
            mr: Some(MrSnapshot { merged: false, conflicted: true }),
            ..base()
        };
        let a = next_action(&s);
        assert_eq!(a.primary, Some(PrimaryAction::ResolveInWorkingDir));
        assert!(a.title.contains("rebase"));
    }

    #[test]
    fn divergence_is_never_auto_resolved() {
        let s = WorkflowSnapshot { diverged: true, ..base() };
        assert_eq!(next_action(&s).primary, Some(PrimaryAction::ResolveInWorkingDir));
    }

    #[test]
    fn member_waits_on_conflict_owner_resolves() {
        let s = WorkflowSnapshot {
            mr: Some(MrSnapshot { merged: false, conflicted: true }),
            ..base()
        };
        assert_eq!(next_action(&s).primary, None);

        let s = WorkflowSnapshot { role: Role::Owner, ..s };
        assert_eq!(next_action(&s).primary, Some(PrimaryAction::ResolveMrConflict));
    }

    #[test]
    fn developing_dirty_commits_clean_finishes() {
        let dirty = WorkflowSnapshot { dirty: true, ..base() };
        assert_eq!(next_action(&dirty).primary, Some(PrimaryAction::Commit));

        let clean = base();
        assert_eq!(next_action(&clean).primary, Some(PrimaryAction::Finish));
    }

    #[test]
    fn hotfix_dirty_commits_clean_finishes_hotfix() {
        let dirty = WorkflowSnapshot { branch: BranchClass::Hotfix, dirty: true, ..base() };
        assert_eq!(next_action(&dirty).primary, Some(PrimaryAction::Commit));

        let clean = WorkflowSnapshot { branch: BranchClass::Hotfix, ..base() };
        assert_eq!(next_action(&clean).primary, Some(PrimaryAction::FinishHotfix));
    }

    #[test]
    fn develop_behind_offers_fast_forward_pull() {
        let s = WorkflowSnapshot {
            branch: BranchClass::Develop,
            work_item: WorkItemState::NotStarted,
            behind: 3,
            ..base()
        };
        assert_eq!(next_action(&s).primary, Some(PrimaryAction::UpdateBranch));

        // Dirty -> no auto pull; in sync -> not offered.
        let dirty = WorkflowSnapshot { dirty: true, ..s.clone() };
        assert_ne!(next_action(&dirty).primary, Some(PrimaryAction::UpdateBranch));
        let synced = WorkflowSnapshot { behind: 0, ..s };
        assert_eq!(next_action(&synced).primary, Some(PrimaryAction::StartWorkItem));
    }

    #[test]
    fn work_branch_behind_is_not_auto_pulled() {
        let s = WorkflowSnapshot { branch: BranchClass::WorkItem, behind: 2, ..base() };
        assert_ne!(next_action(&s).primary, Some(PrimaryAction::UpdateBranch));
    }

    #[test]
    fn release_dirty_gives_guidance_clean_finishes() {
        let dirty = WorkflowSnapshot {
            branch: BranchClass::Release,
            work_item: WorkItemState::NotStarted,
            dirty: true,
            ..base()
        };
        let a = next_action(&dirty);
        assert_eq!(a.primary, None);
        assert!(a.helper.is_some());

        let clean = WorkflowSnapshot {
            branch: BranchClass::Release,
            work_item: WorkItemState::NotStarted,
            ..base()
        };
        assert_eq!(next_action(&clean).primary, Some(PrimaryAction::FinishRelease));
    }

    #[test]
    fn release_in_progress_op_still_wins() {
        let s = WorkflowSnapshot {
            branch: BranchClass::Release,
            in_progress_op: Some("rebase".into()),
            dirty: true,
            ..base()
        };
        assert_eq!(next_action(&s).primary, Some(PrimaryAction::ResolveInWorkingDir));
    }

    #[test]
    fn merged_mr_sends_user_back_to_develop() {
        let s = WorkflowSnapshot {
            work_item: WorkItemState::PushedForReview,
            mr: Some(MrSnapshot { merged: true, conflicted: false }),
            ..base()
        };
        assert_eq!(next_action(&s).primary, Some(PrimaryAction::ReturnToDevelop));
    }

    #[test]
    fn merged_mr_with_new_work_offers_followup() {
        let merged = MrSnapshot { merged: true, conflicted: false };
        let dirty = WorkflowSnapshot {
            work_item: WorkItemState::PushedForReview,
            mr: Some(merged),
            dirty: true,
            ..base()
        };
        assert_eq!(next_action(&dirty).primary, Some(PrimaryAction::Commit));

        let ahead = WorkflowSnapshot {
            work_item: WorkItemState::PushedForReview,
            mr: Some(merged),
            ahead: 2,
            ..base()
        };
        assert_eq!(next_action(&ahead).primary, Some(PrimaryAction::Finish));
    }

    #[test]
    fn pushed_for_review_waits() {
        let s = WorkflowSnapshot {
            work_item: WorkItemState::PushedForReview,
            mr: Some(MrSnapshot { merged: false, conflicted: false }),
            ..base()
        };
        assert_eq!(next_action(&s).primary, None);
    }

    #[test]
    fn pushed_for_review_dirty_commits_then_ahead_pushes() {
        let mr = Some(MrSnapshot { merged: false, conflicted: false });

        let dirty = WorkflowSnapshot {
            work_item: WorkItemState::PushedForReview,
            dirty: true,
            mr,
            ..base()
        };
        assert_eq!(next_action(&dirty).primary, Some(PrimaryAction::Commit));

        let ahead = WorkflowSnapshot {
            work_item: WorkItemState::PushedForReview,
            ahead: 2,
            mr,
            ..base()
        };
        assert_eq!(next_action(&ahead).primary, Some(PrimaryAction::Push));
    }

    #[test]
    fn not_started_offers_start_and_notes_autosave_when_dirty() {
        // On a non-protected branch the dirty-tree autosave note still shows.
        // (Dirty on develop/master is intercepted by the Workflow Guard -- see
        // the `workflow_guard_*` tests.)
        let s = WorkflowSnapshot {
            work_item: WorkItemState::NotStarted,
            branch: BranchClass::Other,
            dirty: true,
            ..base()
        };
        let a = next_action(&s);
        assert_eq!(a.primary, Some(PrimaryAction::StartWorkItem));
        assert!(a.helper.is_some());
    }
}
