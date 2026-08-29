use auth_core::Capability;

use super::client::GitLabClient;
use crate::capability::CapabilityReport;
use crate::traits::ProviderError;

/// GitLab access levels: https://docs.gitlab.com/ee/api/members.html
const GUEST: u32 = 10;
const REPORTER: u32 = 20;
const DEVELOPER: u32 = 30;

/// Probes what the current credential can do against this project, using
/// only non-mutating calls -- never a real test MR/pipeline/etc. `create_mr`
/// is inferred from permission metadata (Developer+ access), not by
/// attempting the create.
pub async fn detect_capabilities(client: &GitLabClient) -> CapabilityReport {
    match client.get_access_level().await {
        Ok(Some(level)) => from_access_level(level),
        Ok(None) => CapabilityReport {
            repo_access: Capability::Yes,
            read_mr: Capability::Unknown,
            create_mr: Capability::Unknown,
            mergeability_read: Capability::Unknown,
            pipeline_read: Capability::Unknown,
            review_read: Capability::Unknown,
        },
        Err(ProviderError::Auth(_)) | Err(ProviderError::NotFound(_)) => CapabilityReport {
            repo_access: Capability::No,
            read_mr: Capability::No,
            create_mr: Capability::No,
            mergeability_read: Capability::No,
            pipeline_read: Capability::No,
            review_read: Capability::No,
        },
        Err(_) => CapabilityReport {
            repo_access: Capability::Unknown,
            read_mr: Capability::Unknown,
            create_mr: Capability::Unknown,
            mergeability_read: Capability::Unknown,
            pipeline_read: Capability::Unknown,
            review_read: Capability::Unknown,
        },
    }
}

fn from_access_level(level: u32) -> CapabilityReport {
    let at_least = |threshold: u32| {
        if level >= threshold {
            Capability::Yes
        } else {
            Capability::No
        }
    };
    CapabilityReport {
        repo_access: Capability::Yes,
        read_mr: at_least(GUEST),
        create_mr: at_least(DEVELOPER),
        mergeability_read: at_least(GUEST),
        pipeline_read: at_least(REPORTER),
        review_read: at_least(GUEST),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_project_with_access_level(access_level: Option<u32>) -> MockServer {
        let server = MockServer::start().await;
        let permissions = access_level.map(|level| {
            serde_json::json!({
                "project_access": { "access_level": level },
                "group_access": null,
            })
        });
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/group%2Frepo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1,
                "path_with_namespace": "group/repo",
                "default_branch": "develop",
                "permissions": permissions,
            })))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn developer_access_can_create_mr_and_read_pipelines() {
        let server = mock_project_with_access_level(Some(DEVELOPER)).await;
        let client = GitLabClient::new(server.uri(), "group/repo", "token");
        let report = detect_capabilities(&client).await;
        assert_eq!(report.repo_access, Capability::Yes);
        assert_eq!(report.create_mr, Capability::Yes);
        assert_eq!(report.pipeline_read, Capability::Yes);
    }

    #[tokio::test]
    async fn guest_access_can_read_but_not_create_mr_or_pipelines() {
        let server = mock_project_with_access_level(Some(GUEST)).await;
        let client = GitLabClient::new(server.uri(), "group/repo", "token");
        let report = detect_capabilities(&client).await;
        assert_eq!(report.read_mr, Capability::Yes);
        assert_eq!(report.create_mr, Capability::No);
        assert_eq!(report.pipeline_read, Capability::No);
    }

    #[tokio::test]
    async fn missing_permissions_field_is_unknown() {
        let server = mock_project_with_access_level(None).await;
        let client = GitLabClient::new(server.uri(), "group/repo", "token");
        let report = detect_capabilities(&client).await;
        assert_eq!(report.repo_access, Capability::Yes);
        assert_eq!(report.create_mr, Capability::Unknown);
    }

    #[tokio::test]
    async fn unauthorized_means_no_access_to_everything() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/group%2Frepo"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let client = GitLabClient::new(server.uri(), "group/repo", "bad-token");
        let report = detect_capabilities(&client).await;
        assert_eq!(report.repo_access, Capability::No);
        assert_eq!(report.create_mr, Capability::No);
    }

    #[tokio::test]
    async fn server_unreachable_is_unknown() {
        // Nothing mounted -- and using a client pointed at a closed port
        // simulates a transient network failure distinct from a real 4xx.
        let client = GitLabClient::new("http://127.0.0.1:1", "group/repo", "token");
        let report = detect_capabilities(&client).await;
        assert_eq!(report.repo_access, Capability::Unknown);
        assert_eq!(report.create_mr, Capability::Unknown);
    }
}
