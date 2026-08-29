use thiserror::Error;

/// Who is attempting a transition. Owner/Member gating happens per-action
/// here; the actual role for a user is resolved upstream by
/// `auth_core::resolve_role` and passed in -- this crate stays pure logic,
/// no I/O, no knowledge of providers or credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Owner,
    Member,
}

/// Hotfix gets its own state machine -- this one covers Feature/Bug/Chore,
/// which all share the same shape (branch prefix is the only difference,
/// handled in `git_core::BranchKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkItemState {
    NotStarted,
    Developing,
    PushedForReview,
    /// Provider reported the MR can't be merged cleanly. Read-only until
    /// Phase 4 wires up Owner-only resolution.
    Conflicted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowedAction {
    StartDevelopment,
    /// Push + open MR, in one gated step -- there is no meaningful
    /// in-between state to gate separately.
    Finish,
    /// Not a human action: the app calls this after polling the provider
    /// and finding the MR unmergeable. No role gate -- it's a fact, not a
    /// request.
    ReportConflict,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransitionError {
    #[error("action {action:?} is not allowed for role {role:?} in state {state:?}")]
    RoleRejected {
        state: WorkItemState,
        action: AllowedAction,
        role: Role,
    },
    #[error("action {action:?} is not valid from state {state:?}")]
    InvalidTransition {
        state: WorkItemState,
        action: AllowedAction,
    },
}

/// The Feature/Bug/Chore work item flow is available to every workflow role:
/// an Owner can run the full Member flow (create work item, finish) in
/// addition to the Owner-only operations gated elsewhere (`require_owner` in
/// the command layer -- conflict resolution, role overrides). `RoleRejected`
/// is retained for callers that still pattern-match on it, but no transition
/// here currently produces it.
pub fn transition(
    state: WorkItemState,
    action: AllowedAction,
    role: Role,
) -> Result<WorkItemState, TransitionError> {
    let _ = role;
    match (state, action) {
        (WorkItemState::NotStarted, AllowedAction::StartDevelopment) => Ok(WorkItemState::Developing),
        (WorkItemState::Developing, AllowedAction::Finish) => Ok(WorkItemState::PushedForReview),
        (WorkItemState::PushedForReview, AllowedAction::ReportConflict) => {
            Ok(WorkItemState::Conflicted)
        }
        _ => Err(TransitionError::InvalidTransition { state, action }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_can_start_development() {
        let next = transition(
            WorkItemState::NotStarted,
            AllowedAction::StartDevelopment,
            Role::Member,
        )
        .unwrap();
        assert_eq!(next, WorkItemState::Developing);
    }

    #[test]
    fn owner_can_also_start_development() {
        let next = transition(
            WorkItemState::NotStarted,
            AllowedAction::StartDevelopment,
            Role::Owner,
        )
        .unwrap();
        assert_eq!(next, WorkItemState::Developing);
    }

    #[test]
    fn member_can_finish_to_pushed_for_review() {
        let next = transition(WorkItemState::Developing, AllowedAction::Finish, Role::Member).unwrap();
        assert_eq!(next, WorkItemState::PushedForReview);
    }

    #[test]
    fn owner_can_also_finish() {
        let next = transition(WorkItemState::Developing, AllowedAction::Finish, Role::Owner).unwrap();
        assert_eq!(next, WorkItemState::PushedForReview);
    }

    #[test]
    fn invalid_transition_still_rejected() {
        let err = transition(WorkItemState::NotStarted, AllowedAction::Finish, Role::Member).unwrap_err();
        assert!(matches!(err, TransitionError::InvalidTransition { .. }));
    }

    #[test]
    fn conflict_reported_from_pushed_for_review() {
        let next = transition(
            WorkItemState::PushedForReview,
            AllowedAction::ReportConflict,
            Role::Owner,
        )
        .unwrap();
        assert_eq!(next, WorkItemState::Conflicted);
    }
}
