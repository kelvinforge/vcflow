mod conflict_flow;
mod hotfix_flow;
mod next_action;
mod state;

pub use conflict_flow::{
    transition as transition_conflict, ConflictAction, ConflictState, ConflictTransitionError,
};
pub use hotfix_flow::{apply as apply_hotfix_action, HotfixAction, HotfixState, MrOutcome};
pub use next_action::{
    next_action, BranchClass, MrSnapshot, NextAction, PrimaryAction, WorkflowSnapshot,
};
pub use state::{transition, AllowedAction, Role, TransitionError, WorkItemState};
