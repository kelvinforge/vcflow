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
    ReturnToDevelop,
    StartWorkItem,
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
            diverged: false,
            mr: None,
        }
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
    fn merged_mr_sends_user_back_to_develop() {
        let s = WorkflowSnapshot {
            work_item: WorkItemState::PushedForReview,
            mr: Some(MrSnapshot { merged: true, conflicted: false }),
            ..base()
        };
        assert_eq!(next_action(&s).primary, Some(PrimaryAction::ReturnToDevelop));
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
        let s = WorkflowSnapshot {
            work_item: WorkItemState::NotStarted,
            branch: BranchClass::Develop,
            dirty: true,
            ..base()
        };
        let a = next_action(&s);
        assert_eq!(a.primary, Some(PrimaryAction::StartWorkItem));
        assert!(a.helper.is_some());
    }
}
