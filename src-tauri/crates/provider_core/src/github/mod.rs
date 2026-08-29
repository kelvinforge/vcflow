mod capability;
mod client;

pub use crate::capability::CapabilityReport;
pub use capability::detect_capabilities;
pub use client::{github_api_base, GitHubClient};
