mod branch;
mod commit_push;
mod conflict_verify;
mod divergence;
mod fetch;
mod hotfix;
mod merge;
mod op_state;
mod preflight;
mod repo;
mod repo_state;
mod save_work;
mod ssh;
mod status;
mod sync;
mod version;

pub use branch::{checkout_branch, create_work_branch, BranchError, BranchKind};
pub use commit_push::{commit_all, commit_merge, push, CommitPushError};
pub use conflict_verify::{verify_resolved, Issue, VerifyError};
pub use divergence::{compare_refs, Divergence, DivergenceError};
pub use fetch::{fetch_origin, FetchError, FetchReport, UpdatedRef};
pub use hotfix::{create_hotfix_branch, HotfixError};
pub use merge::{merge_target_into_head, ConflictMerge, MergeError};
pub use op_state::{in_progress_operation, InProgressOp};
pub use preflight::{
    assemble_preflight, Check, CheckStatus, Preflight, PreflightProvider,
};
pub use repo::{head_branch, is_dirty, production_branch, read_repo_info, RepoError, RepoInfo};
pub use repo_state::{read_repository_state, RepoStateError, RepositoryState};
pub use save_work::{discard_work, restore_work, save_work, SavedWork, SaveWorkError};
pub use ssh::{validate_remote_connection, SshError};
pub use status::{read_working_tree_status, RenamedFile, StatusError, WorkingTreeStatus};
pub use sync::{fast_forward_from_origin, FastForward, SyncError};
pub use version::{read_version_file, write_version_file, BumpKind, Version, VersionError};
