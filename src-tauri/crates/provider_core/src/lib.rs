mod capability;
mod detect;
pub mod github;
pub mod gitlab;
mod traits;

pub use capability::CapabilityReport;
pub use detect::{detect_provider, Provider};
pub use traits::{
    GitProvider, MergeRequest, MergeStatus, Mergeability, PipelineStatus, ProviderError,
    RepositoryInfo, ReviewStatus, Role,
};
