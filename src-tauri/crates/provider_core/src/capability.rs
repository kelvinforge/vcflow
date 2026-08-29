use auth_core::Capability;

/// Result of a non-mutating capability probe against the current credential.
/// Shared by every provider's `detect_capabilities` -- the labels are
/// provider-neutral (a GitLab "merge request" and a GitHub "pull request"
/// map to the same slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityReport {
    pub repo_access: Capability,
    pub read_mr: Capability,
    pub create_mr: Capability,
    pub mergeability_read: Capability,
    pub pipeline_read: Capability,
    pub review_read: Capability,
}

impl CapabilityReport {
    /// Every slot set to the same value -- the "all denied" / "all unknown"
    /// shorthands each provider needs for its auth-error and network-error
    /// arms.
    pub const fn uniform(c: Capability) -> Self {
        Self {
            repo_access: c,
            read_mr: c,
            create_mr: c,
            mergeability_read: c,
            pipeline_read: c,
            review_read: c,
        }
    }
}
