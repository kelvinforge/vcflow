use serde::Deserialize;

use crate::traits::{
    Mergeability, MergeRequest, MergeStatus, ProviderError, RepositoryInfo, Role,
};

#[derive(Debug, Deserialize)]
struct ProjectResponse {
    id: u64,
    path_with_namespace: String,
    default_branch: Option<String>,
    permissions: Option<Permissions>,
}

#[derive(Debug, Deserialize)]
struct Permissions {
    project_access: Option<AccessLevel>,
    group_access: Option<AccessLevel>,
}

#[derive(Debug, Deserialize)]
struct AccessLevel {
    access_level: u32,
}

#[derive(Debug, Deserialize)]
struct CurrentUserResponse {
    id: u64,
}

#[derive(Debug, Deserialize)]
struct MemberResponse {
    access_level: u32,
}

#[derive(Debug, Deserialize)]
struct MergeRequestResponse {
    iid: u64,
    source_branch: String,
    target_branch: String,
    state: String,
    web_url: String,
    /// "can_be_merged" / "cannot_be_merged" / "unchecked", per
    /// https://docs.gitlab.com/ee/api/merge_requests.html
    merge_status: Option<String>,
}

/// Thin wrapper over the GitLab REST v4 API for one project, addressed by
/// URL-encoded `namespace/project` path (works identically for gitlab.com
/// and self-hosted instances -- the host is whatever `base_url` points at).
pub struct GitLabClient {
    pub(crate) http: reqwest::Client,
    pub(crate) base_url: String,
    pub(crate) project_path: String,
    pub(crate) token: String,
}

impl GitLabClient {
    pub fn new(base_url: impl Into<String>, project_path: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            project_path: project_path.into(),
            token: token.into(),
        }
    }

    pub(crate) fn project_id_path(&self) -> String {
        urlencode(&self.project_path)
    }

    async fn get_project(&self) -> Result<ProjectResponse, ProviderError> {
        let url = format!(
            "{}/api/v4/projects/{}",
            self.base_url,
            self.project_id_path()
        );
        self.get_json(&url).await
    }

    pub async fn get_repository_info(&self) -> Result<RepositoryInfo, ProviderError> {
        let resp = self.get_project().await?;
        Ok(RepositoryInfo {
            id: resp.id.to_string(),
            full_name: resp.path_with_namespace,
            default_branch: resp.default_branch.unwrap_or_else(|| "develop".to_string()),
        })
    }

    /// Highest project/group access level for the current token, from the
    /// same project payload's `permissions` field. `Ok(None)` means the
    /// project was reachable but no permission info was returned; the
    /// `Err` cases (auth/not-found/network) propagate for the caller
    /// (capability detection) to interpret.
    pub(crate) async fn get_access_level(&self) -> Result<Option<u32>, ProviderError> {
        let resp = self.get_project().await?;
        let level = resp.permissions.and_then(|p| {
            [p.project_access, p.group_access]
                .into_iter()
                .flatten()
                .map(|a| a.access_level)
                .max()
        });
        Ok(level)
    }

    pub async fn get_current_user_role(&self) -> Result<Role, ProviderError> {
        let user_url = format!("{}/api/v4/user", self.base_url);
        let user: CurrentUserResponse = self.get_json(&user_url).await?;

        let member_url = format!(
            "{}/api/v4/projects/{}/members/all/{}",
            self.base_url,
            self.project_id_path(),
            user.id
        );
        let member: MemberResponse = self.get_json(&member_url).await?;
        Ok(map_access_level(member.access_level))
    }

    /// Opens an MR from `source_branch` into `target_branch`. The only
    /// state-changing call this app makes through a provider API --
    /// everything else that mutates repo state goes through `git_core`,
    /// never through here.
    pub async fn create_merge_request(
        &self,
        source_branch: &str,
        target_branch: &str,
        title: &str,
    ) -> Result<MergeRequest, ProviderError> {
        let url = format!(
            "{}/api/v4/projects/{}/merge_requests",
            self.base_url,
            self.project_id_path()
        );
        let body = serde_json::json!({
            "source_branch": source_branch,
            "target_branch": target_branch,
            "title": title,
        });
        let resp = self
            .http
            .post(&url)
            .header("PRIVATE-TOKEN", &self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;

        let resp = Self::check_status(resp, &url)?;
        let mr: MergeRequestResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::UnexpectedResponse(e.to_string()))?;
        Ok(to_merge_request(mr))
    }

    pub async fn get_merge_request(&self, iid: &str) -> Result<MergeRequest, ProviderError> {
        let url = format!(
            "{}/api/v4/projects/{}/merge_requests/{iid}",
            self.base_url,
            self.project_id_path()
        );
        let mr: MergeRequestResponse = self.get_json(&url).await?;
        Ok(to_merge_request(mr))
    }

    /// Reads the provider's own merge-status verdict for an already-open MR.
    /// Read-only -- reports conflicts, never resolves them (Phase 4).
    pub async fn check_mergeability(&self, iid: &str) -> Result<Mergeability, ProviderError> {
        let url = format!(
            "{}/api/v4/projects/{}/merge_requests/{iid}",
            self.base_url,
            self.project_id_path()
        );
        let mr: MergeRequestResponse = self.get_json(&url).await?;
        Ok(to_mergeability(mr.merge_status.as_deref()))
    }

    pub(crate) fn check_status(resp: reqwest::Response, url: &str) -> Result<reqwest::Response, ProviderError> {
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::Auth(format!("HTTP {status}")));
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(ProviderError::NotFound(url.to_string()));
        }
        if !status.is_success() {
            return Err(ProviderError::UnexpectedResponse(format!("HTTP {status}")));
        }
        Ok(resp)
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T, ProviderError> {
        let resp = self
            .http
            .get(url)
            .header("PRIVATE-TOKEN", &self.token)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;

        let resp = Self::check_status(resp, url)?;
        resp.json::<T>()
            .await
            .map_err(|e| ProviderError::UnexpectedResponse(e.to_string()))
    }
}

fn to_merge_request(mr: MergeRequestResponse) -> MergeRequest {
    let status = match mr.state.as_str() {
        "merged" => MergeStatus::Merged,
        "closed" => MergeStatus::Closed,
        _ => MergeStatus::Open,
    };
    MergeRequest {
        id: mr.iid.to_string(),
        source_branch: mr.source_branch,
        target_branch: mr.target_branch,
        status,
        web_url: mr.web_url,
    }
}

fn to_mergeability(merge_status: Option<&str>) -> Mergeability {
    match merge_status {
        Some("can_be_merged") => Mergeability::Mergeable,
        Some("cannot_be_merged") => Mergeability::Conflicted,
        _ => Mergeability::Unknown,
    }
}

/// GitLab access levels: https://docs.gitlab.com/ee/api/members.html
/// 40 = Maintainer, 50 = Owner -> Owner. 30 = Developer (and anything lower) -> Member.
fn map_access_level(access_level: u32) -> Role {
    if access_level >= 40 {
        Role::Owner
    } else {
        Role::Member
    }
}

fn urlencode(s: &str) -> String {
    s.replace('/', "%2F")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn maps_maintainer_and_owner_to_owner_and_developer_to_member() {
        for (access_level, expected) in [(40u32, Role::Owner), (50, Role::Owner), (30, Role::Member)]
        {
            let server = MockServer::start().await;

            Mock::given(method("GET"))
                .and(path("/api/v4/user"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": 7 })))
                .mount(&server)
                .await;

            Mock::given(method("GET"))
                .and(path("/api/v4/projects/group%2Frepo/members/all/7"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({ "access_level": access_level })),
                )
                .mount(&server)
                .await;

            let client = GitLabClient::new(server.uri(), "group/repo", "token");
            let role = client.get_current_user_role().await.unwrap();
            assert_eq!(role, expected, "access_level {access_level}");
        }
    }

    #[tokio::test]
    async fn fetches_repository_info() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/group%2Frepo"))
            .and(header("PRIVATE-TOKEN", "token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 42,
                "path_with_namespace": "group/repo",
                "default_branch": "develop",
            })))
            .mount(&server)
            .await;

        let client = GitLabClient::new(server.uri(), "group/repo", "token");
        let info = client.get_repository_info().await.unwrap();
        assert_eq!(info.id, "42");
        assert_eq!(info.full_name, "group/repo");
        assert_eq!(info.default_branch, "develop");
    }

    #[tokio::test]
    async fn unauthorized_maps_to_auth_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/group%2Frepo"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = GitLabClient::new(server.uri(), "group/repo", "bad-token");
        let err = client.get_repository_info().await.unwrap_err();
        assert!(matches!(err, ProviderError::Auth(_)));
    }
}
