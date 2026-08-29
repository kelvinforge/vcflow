use thiserror::Error;

use crate::state::Role;

/// Mirrors the spec's Conflict Workflow diagram: a Member's MR going into
/// `WaitingForOwner`, then only the Owner walking it through
/// in-place resolution back to a push on the *original* branch (never a new MR).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictState {
    Detected,
    WaitingForOwner,
    Resolving,
    Verified,
    Committed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAction {
    /// Automatic on MR conflict detection -- no role gate, it's a fact, not
    /// an action anyone triggers.
    Wait,
    /// Owner-only trigger -- same risk tier as Release triggering (spec:
    /// both gated identically via `resolve_role()`, never a hidden UI button).
    StartResolution,
    /// Runs `git_core::verify_resolved` (markers + `git diff --check`);
    /// carries no payload here, the pure state machine only cares that a
    /// verify step happened before commit, never that it can be skipped.
    Verify,
    CommitAndPush,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConflictTransitionError {
    #[error("action {action:?} is not allowed for role {role:?} in state {state:?}")]
    RoleRejected {
        state: ConflictState,
        action: ConflictAction,
        role: Role,
    },
    #[error("action {action:?} is not valid from state {state:?}")]
    InvalidTransition {
        state: ConflictState,
        action: ConflictAction,
    },
}

pub fn transition(
    state: ConflictState,
    action: ConflictAction,
    role: Role,
) -> Result<ConflictState, ConflictTransitionError> {
    use ConflictAction::*;
    use ConflictState::*;

    match (state, action) {
        (Detected, Wait) => Ok(WaitingForOwner),
        (WaitingForOwner, StartResolution) => {
            if role != Role::Owner {
                return Err(ConflictTransitionError::RoleRejected { state, action, role });
            }
            Ok(Resolving)
        }
        (Resolving, Verify) => {
            if role != Role::Owner {
                return Err(ConflictTransitionError::RoleRejected { state, action, role });
            }
            Ok(Verified)
        }
        (Verified, CommitAndPush) => {
            if role != Role::Owner {
                return Err(ConflictTransitionError::RoleRejected { state, action, role });
            }
            Ok(Committed)
        }
        _ => Err(ConflictTransitionError::InvalidTransition { state, action }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_cannot_start_resolution() {
        let err = transition(ConflictState::WaitingForOwner, ConflictAction::StartResolution, Role::Member)
            .unwrap_err();
        assert!(matches!(err, ConflictTransitionError::RoleRejected { .. }));
    }

    #[test]
    fn member_cannot_verify_or_commit_even_if_state_reached() {
        let err = transition(ConflictState::Resolving, ConflictAction::Verify, Role::Member).unwrap_err();
        assert!(matches!(err, ConflictTransitionError::RoleRejected { .. }));

        let err = transition(ConflictState::Verified, ConflictAction::CommitAndPush, Role::Member)
            .unwrap_err();
        assert!(matches!(err, ConflictTransitionError::RoleRejected { .. }));
    }

    #[test]
    fn owner_can_start_resolution() {
        let next = transition(ConflictState::WaitingForOwner, ConflictAction::StartResolution, Role::Owner)
            .unwrap();
        assert_eq!(next, ConflictState::Resolving);
    }

    #[test]
    fn cannot_skip_verify_straight_to_commit() {
        let err = transition(ConflictState::Resolving, ConflictAction::CommitAndPush, Role::Owner)
            .unwrap_err();
        assert!(matches!(err, ConflictTransitionError::InvalidTransition { .. }));
    }

    #[test]
    fn full_happy_path_walks_every_state() {
        let role = Role::Owner;
        let mut state = ConflictState::Detected;
        for action in [
            ConflictAction::Wait,
            ConflictAction::StartResolution,
            ConflictAction::Verify,
            ConflictAction::CommitAndPush,
        ] {
            state = transition(state, action, role).unwrap();
        }
        assert_eq!(state, ConflictState::Committed);
    }
}
