use auth_core::Capability;

use super::client::GitHubClient;
use crate::capability::CapabilityReport;
use crate::traits::ProviderError;

/// Probes what the current token can do against this repo, using only the
/// non-mutating `GET /repos/{owner}/{repo}` permission block -- never a real
/// test PR. `create_mr` needs write (`push`/`maintain`/`admin`); everything
/// else follows from the repo being readable at all.
pub async fn detect_capabilities(client: &GitHubClient) -> CapabilityReport {
    match client.get_permissions().await {
        Ok(perms) => {
            let p = perms.unwrap_or_default();
            let can_write = if p.push || p.maintain || p.admin {
                Capability::Yes
            } else {
                Capability::No
            };
            CapabilityReport {
                repo_access: Capability::Yes,
                read_mr: Capability::Yes,
                create_mr: can_write,
                mergeability_read: Capability::Yes,
                pipeline_read: Capability::Yes,
                review_read: Capability::Yes,
            }
        }
        Err(ProviderError::Auth(_)) | Err(ProviderError::NotFound(_)) => {
            CapabilityReport::uniform(Capability::No)
        }
        Err(_) => CapabilityReport::uniform(Capability::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn repo_with_permissions(perms: serde_json::Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 1,
                "full_name": "octo/repo",
                "default_branch": "main",
                "permissions": perms,
            })))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn write_access_can_create_pull_requests() {
        let server = repo_with_permissions(serde_json::json!({ "push": true, "pull": true })).await;
        let client = GitHubClient::new(server.uri(), "octo", "repo", "t");
        let report = detect_capabilities(&client).await;
        assert_eq!(report.repo_access, Capability::Yes);
        assert_eq!(report.create_mr, Capability::Yes);
    }

    #[tokio::test]
    async fn read_only_access_cannot_create_pull_requests() {
        let server = repo_with_permissions(serde_json::json!({ "pull": true })).await;
        let client = GitHubClient::new(server.uri(), "octo", "repo", "t");
        let report = detect_capabilities(&client).await;
        assert_eq!(report.read_mr, Capability::Yes);
        assert_eq!(report.create_mr, Capability::No);
    }

    #[tokio::test]
    async fn unauthorized_means_no_capabilities() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "message": "Bad credentials",
            })))
            .mount(&server)
            .await;
        let client = GitHubClient::new(server.uri(), "octo", "repo", "bad");
        let report = detect_capabilities(&client).await;
        assert_eq!(report.repo_access, Capability::No);
        assert_eq!(report.create_mr, Capability::No);
    }

    #[tokio::test]
    async fn unreachable_host_is_unknown() {
        let client = GitHubClient::new("http://127.0.0.1:1", "octo", "repo", "t");
        let report = detect_capabilities(&client).await;
        assert_eq!(report.repo_access, Capability::Unknown);
    }
}
