use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("network/transport error: {0}")]
    Network(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("provider returned an unexpected response: {0}")]
    UnexpectedResponse(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    Owner,
    Member,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryInfo {
    pub id: String,
    pub full_name: String,
    pub default_branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeStatus {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRequest {
    pub id: String,
    pub source_branch: String,
    pub target_branch: String,
    pub status: MergeStatus,
    pub web_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mergeability {
    Mergeable,
    Conflicted,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineStatus {
    Pending,
    Running,
    Success,
    Failed,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewStatus {
    Approved,
    ChangesRequested,
    Pending,
}

/// Abstraction over a Git hosting provider's API (GitLab, GitHub, ...).
///
/// Every method is read-only except `create_merge_request`, the only
/// state-changing call the app makes through this trait -- everything else
/// that mutates repo state goes through `git_core`, never through here.
#[async_trait]
pub trait GitProvider: Send + Sync {
    async fn get_repository_info(&self) -> Result<RepositoryInfo, ProviderError>;
    async fn get_current_user_role(&self) -> Result<Role, ProviderError>;
    async fn get_merge_request(&self, id: &str) -> Result<MergeRequest, ProviderError>;
    async fn create_merge_request(
        &self,
        source_branch: &str,
        target_branch: &str,
        title: &str,
    ) -> Result<MergeRequest, ProviderError>;
    async fn check_mergeability(&self, id: &str) -> Result<Mergeability, ProviderError>;
    async fn get_pipeline_status(&self, id: &str) -> Result<PipelineStatus, ProviderError>;
    async fn get_review_status(&self, id: &str) -> Result<ReviewStatus, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stub impl -- exists only to prove `GitProvider` is dyn-compatible
    /// (object-safe) before any real provider is written.
    struct StubProvider;

    #[async_trait]
    impl GitProvider for StubProvider {
        async fn get_repository_info(&self) -> Result<RepositoryInfo, ProviderError> {
            Ok(RepositoryInfo {
                id: "1".into(),
                full_name: "group/repo".into(),
                default_branch: "develop".into(),
            })
        }
        async fn get_current_user_role(&self) -> Result<Role, ProviderError> {
            Ok(Role::Member)
        }
        async fn get_merge_request(&self, id: &str) -> Result<MergeRequest, ProviderError> {
            Ok(MergeRequest {
                id: id.into(),
                source_branch: "feature/x".into(),
                target_branch: "develop".into(),
                status: MergeStatus::Open,
                web_url: "https://example.com/mr/1".into(),
            })
        }
        async fn create_merge_request(
            &self,
            source_branch: &str,
            target_branch: &str,
            _title: &str,
        ) -> Result<MergeRequest, ProviderError> {
            Ok(MergeRequest {
                id: "1".into(),
                source_branch: source_branch.into(),
                target_branch: target_branch.into(),
                status: MergeStatus::Open,
                web_url: "https://example.com/mr/1".into(),
            })
        }
        async fn check_mergeability(&self, _id: &str) -> Result<Mergeability, ProviderError> {
            Ok(Mergeability::Mergeable)
        }
        async fn get_pipeline_status(&self, _id: &str) -> Result<PipelineStatus, ProviderError> {
            Ok(PipelineStatus::Success)
        }
        async fn get_review_status(&self, _id: &str) -> Result<ReviewStatus, ProviderError> {
            Ok(ReviewStatus::Approved)
        }
    }

    #[tokio::test]
    async fn stub_is_dyn_compatible_and_callable() {
        let provider: Box<dyn GitProvider> = Box::new(StubProvider);
        let info = provider.get_repository_info().await.unwrap();
        assert_eq!(info.full_name, "group/repo");
        assert_eq!(provider.get_current_user_role().await.unwrap(), Role::Member);
    }
}
