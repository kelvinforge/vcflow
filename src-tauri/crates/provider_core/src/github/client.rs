use serde::Deserialize;

use crate::traits::{Mergeability, MergeRequest, MergeStatus, ProviderError, RepositoryInfo, Role};

/// GitHub REST API version pin -- sent as `X-GitHub-Api-Version` on every
/// request so a future default bump can't silently change response shapes.
const API_VERSION: &str = "2026-03-10";
/// GitHub rejects API requests with no `User-Agent`, so every client sends one.
const USER_AGENT: &str = "git-workflow-engine";

/// Resolves the REST API base URL for a GitHub host.
///
/// `github.com` -> `https://api.github.com` (the API lives on a separate
/// hostname). Any other host is treated as GitHub Enterprise Server, whose
/// API is under `/api/v3` on the same host. Accepts a bare host or one with a
/// scheme/path -- both are stripped first, so this never produces
/// `https://github.com/api/v3` or a non-API path as the base.
pub fn github_api_base(host: &str) -> String {
    let host = host
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    if host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("www.github.com") {
        "https://api.github.com".to_string()
    } else {
        format!("https://{host}/api/v3")
    }
}

#[derive(Debug, Deserialize)]
struct UserResponse {
    login: String,
}

#[derive(Debug, Deserialize)]
struct RepoResponse {
    id: u64,
    full_name: String,
    default_branch: Option<String>,
    permissions: Option<Permissions>,
}

/// The authenticated user's permission flags for a repo, from `GET /repos`.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Permissions {
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub maintain: bool,
    #[serde(default)]
    pub push: bool,
    #[serde(default)]
    pub triage: bool,
    #[serde(default)]
    pub pull: bool,
}

#[derive(Debug, Deserialize)]
struct PullResponse {
    number: u64,
    head: GitRef,
    base: GitRef,
    state: String,
    html_url: String,
    #[serde(default)]
    merged: bool,
    #[serde(default)]
    mergeable: Option<bool>,
    #[serde(default)]
    mergeable_state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitRef {
    #[serde(rename = "ref")]
    name: String,
}

/// Thin wrapper over the GitHub REST API for one repository, addressed by
/// `owner`/`repo`. Works for `github.com` and GitHub Enterprise Server alike
/// -- the difference is entirely in `base_url` (see `github_api_base`).
///
/// Every method is read-only except `create_merge_request`, matching the
/// `GitLabClient` contract: everything that mutates repo state goes through
/// `git_core`, never here.
pub struct GitHubClient {
    http: reqwest::Client,
    base_url: String,
    owner: String,
    repo: String,
    token: String,
}

impl GitHubClient {
    pub fn new(
        base_url: impl Into<String>,
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .build()
                .unwrap_or_default(),
            base_url: base_url.into(),
            owner: owner.into(),
            repo: repo.into(),
            token: token.into(),
        }
    }

    fn repo_url(&self, suffix: &str) -> String {
        format!(
            "{}/repos/{}/{}{}",
            self.base_url, self.owner, self.repo, suffix
        )
    }

    fn req(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
    }

    async fn send_json<T: for<'de> Deserialize<'de>>(
        &self,
        builder: reqwest::RequestBuilder,
        url: &str,
    ) -> Result<T, ProviderError> {
        let resp = builder
            .send()
            .await
            .map_err(|e| ProviderError::Network(sanitize(&e.to_string())))?;
        let resp = check_status(resp, url).await?;
        resp.json::<T>()
            .await
            .map_err(|e| ProviderError::UnexpectedResponse(sanitize(&e.to_string())))
    }

    /// `GET /user` -- the login of whoever the token belongs to.
    pub async fn get_current_user_login(&self) -> Result<String, ProviderError> {
        let url = format!("{}/user", self.base_url);
        let user: UserResponse = self
            .send_json(self.req(reqwest::Method::GET, &url), &url)
            .await?;
        Ok(user.login)
    }

    async fn get_repo(&self) -> Result<RepoResponse, ProviderError> {
        let url = self.repo_url("");
        self.send_json(self.req(reqwest::Method::GET, &url), &url)
            .await
    }

    /// `GET /repos/{owner}/{repo}` -- id, full name, default branch.
    pub async fn get_repository_info(&self) -> Result<RepositoryInfo, ProviderError> {
        let r = self.get_repo().await?;
        Ok(RepositoryInfo {
            id: r.id.to_string(),
            full_name: r.full_name,
            default_branch: r.default_branch.unwrap_or_else(|| "main".to_string()),
        })
    }

    /// The authenticated user's permission flags for this repo, or `None`
    /// when the repo was reachable but returned no `permissions` block.
    pub async fn get_permissions(&self) -> Result<Option<Permissions>, ProviderError> {
        Ok(self.get_repo().await?.permissions)
    }

    /// `admin` or `maintain` -> `Owner` (can drive merges/releases), anything
    /// lower -> `Member`. Mirrors GitLab's Maintainer+ -> Owner mapping.
    pub async fn get_current_user_role(&self) -> Result<Role, ProviderError> {
        // Probe `/user` first so a bad token surfaces as a clean auth error
        // rather than a repo 404.
        let _ = self.get_current_user_login().await?;
        let perms = self.get_permissions().await?.unwrap_or_default();
        Ok(if perms.admin || perms.maintain {
            Role::Owner
        } else {
            Role::Member
        })
    }

    /// `GET /repos/{owner}/{repo}/pulls` -- every PR (any state).
    pub async fn list_pull_requests(&self) -> Result<Vec<MergeRequest>, ProviderError> {
        let url = self.repo_url("/pulls?state=all&per_page=100");
        let pulls: Vec<PullResponse> = self
            .send_json(self.req(reqwest::Method::GET, &url), &url)
            .await?;
        Ok(pulls.into_iter().map(to_merge_request).collect())
    }

    /// `GET /repos/{owner}/{repo}/pulls/{number}`.
    pub async fn get_merge_request(&self, number: &str) -> Result<MergeRequest, ProviderError> {
        let pr = self.get_pull(number).await?;
        Ok(to_merge_request(pr))
    }

    /// Reads GitHub's own mergeability verdict for an open PR. Read-only --
    /// reports conflicts, never resolves them.
    pub async fn check_mergeability(&self, number: &str) -> Result<Mergeability, ProviderError> {
        let pr = self.get_pull(number).await?;
        Ok(to_mergeability(&pr))
    }

    async fn get_pull(&self, number: &str) -> Result<PullResponse, ProviderError> {
        let url = self.repo_url(&format!("/pulls/{number}"));
        self.send_json(self.req(reqwest::Method::GET, &url), &url)
            .await
    }

    /// `POST /repos/{owner}/{repo}/pulls` -- opens a PR from `source_branch`
    /// into `target_branch`. The only state-changing call in this client.
    pub async fn create_merge_request(
        &self,
        source_branch: &str,
        target_branch: &str,
        title: &str,
    ) -> Result<MergeRequest, ProviderError> {
        let url = self.repo_url("/pulls");
        let body = serde_json::json!({
            "title": title,
            "head": source_branch,
            "base": target_branch,
        });
        let pr: PullResponse = self
            .send_json(self.req(reqwest::Method::POST, &url).json(&body), &url)
            .await?;
        Ok(to_merge_request(pr))
    }
}

fn to_merge_request(pr: PullResponse) -> MergeRequest {
    let status = if pr.merged {
        MergeStatus::Merged
    } else if pr.state == "closed" {
        MergeStatus::Closed
    } else {
        MergeStatus::Open
    };
    MergeRequest {
        id: pr.number.to_string(),
        source_branch: pr.head.name,
        target_branch: pr.base.name,
        status,
        web_url: pr.html_url,
    }
}

fn to_mergeability(pr: &PullResponse) -> Mergeability {
    match pr.mergeable_state.as_deref() {
        Some("dirty") => Mergeability::Conflicted,
        _ => match pr.mergeable {
            Some(true) => Mergeability::Mergeable,
            Some(false) => Mergeability::Conflicted,
            None => Mergeability::Unknown,
        },
    }
}

/// Maps a non-2xx GitHub response to a `ProviderError`, preserving the HTTP
/// status, the sanitized request path, and GitHub's own `message` field.
/// The token lives only in the `Authorization` header, which is never part of
/// the URL or the body, so it cannot appear here -- and `sanitize` masks any
/// known token shape defensively regardless.
async fn check_status(
    resp: reqwest::Response,
    url: &str,
) -> Result<reqwest::Response, ProviderError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let path = sanitize_url(url);
    let body = resp.text().await.unwrap_or_default();
    let message = extract_message(&body);
    let detail = format!("HTTP {} at {path}: {message}", status.as_u16());
    match status.as_u16() {
        401 => Err(ProviderError::Auth(format!("{detail} (authentication failed)"))),
        403 => Err(ProviderError::Auth(format!(
            "{detail} (permission denied or rate limit)"
        ))),
        404 => Err(ProviderError::NotFound(format!(
            "{detail} (repository/resource not found or not visible to this token)"
        ))),
        _ => Err(ProviderError::UnexpectedResponse(detail)),
    }
}

fn extract_message(body: &str) -> String {
    let msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_string))
        .unwrap_or_else(|| body.to_string());
    sanitize(&msg)
}

fn sanitize_url(url: &str) -> String {
    let no_query = url.split('?').next().unwrap_or(url);
    truncate(&auth_core::mask_secrets(no_query), 200)
}

fn sanitize(s: &str) -> String {
    truncate(&auth_core::mask_secrets(s.trim()), 300)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn base_url_for_github_dot_com() {
        assert_eq!(github_api_base("github.com"), "https://api.github.com");
        assert_eq!(github_api_base("https://github.com"), "https://api.github.com");
        assert_eq!(
            github_api_base("https://github.com/owner/repo.git"),
            "https://api.github.com"
        );
        assert_eq!(github_api_base("GitHub.com"), "https://api.github.com");
    }

    #[test]
    fn base_url_for_enterprise_server() {
        assert_eq!(
            github_api_base("github.company.com"),
            "https://github.company.com/api/v3"
        );
        assert_eq!(
            github_api_base("https://github.company.com"),
            "https://github.company.com/api/v3"
        );
    }

    #[tokio::test]
    async fn uses_bearer_auth_and_version_headers() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .and(header("authorization", "Bearer secret-token"))
            .and(header("accept", "application/vnd.github+json"))
            .and(header("x-github-api-version", API_VERSION))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "login": "octocat" })),
            )
            .mount(&server)
            .await;

        let client = GitHubClient::new(server.uri(), "octo", "repo", "secret-token");
        assert_eq!(client.get_current_user_login().await.unwrap(), "octocat");
    }

    #[tokio::test]
    async fn maps_admin_and_maintain_to_owner() {
        for (perm, expected) in [
            ("admin", Role::Owner),
            ("maintain", Role::Owner),
            ("push", Role::Member),
            ("pull", Role::Member),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/user"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({ "login": "octocat" })),
                )
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/repos/octo/repo"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": 1,
                    "full_name": "octo/repo",
                    "default_branch": "main",
                    "permissions": { perm: true },
                })))
                .mount(&server)
                .await;

            let client = GitHubClient::new(server.uri(), "octo", "repo", "t");
            assert_eq!(
                client.get_current_user_role().await.unwrap(),
                expected,
                "perm {perm}"
            );
        }
    }

    #[tokio::test]
    async fn parses_and_creates_pull_requests() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/octo/repo/pulls"))
            .and(header("authorization", "Bearer t"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "number": 7,
                "head": { "ref": "feature/x" },
                "base": { "ref": "develop" },
                "state": "open",
                "html_url": "https://github.com/octo/repo/pull/7",
            })))
            .mount(&server)
            .await;

        let client = GitHubClient::new(server.uri(), "octo", "repo", "t");
        let mr = client
            .create_merge_request("feature/x", "develop", "Add x")
            .await
            .unwrap();
        assert_eq!(mr.id, "7");
        assert_eq!(mr.source_branch, "feature/x");
        assert_eq!(mr.status, MergeStatus::Open);
        assert_eq!(mr.web_url, "https://github.com/octo/repo/pull/7");
    }

    #[tokio::test]
    async fn dirty_mergeable_state_is_conflicted() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/octo/repo/pulls/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "number": 7,
                "head": { "ref": "feature/x" },
                "base": { "ref": "develop" },
                "state": "open",
                "html_url": "https://example.com/pull/7",
                "mergeable": false,
                "mergeable_state": "dirty",
            })))
            .mount(&server)
            .await;

        let client = GitHubClient::new(server.uri(), "octo", "repo", "t");
        assert_eq!(
            client.check_mergeability("7").await.unwrap(),
            Mergeability::Conflicted
        );
    }

    #[tokio::test]
    async fn error_statuses_map_to_variants_without_leaking_token() {
        let cases = [
            (401, "Bad credentials"),
            (403, "API rate limit exceeded"),
            (404, "Not Found"),
        ];
        for (code, message) in cases {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/repos/octo/repo"))
                .respond_with(
                    ResponseTemplate::new(code)
                        .set_body_json(serde_json::json!({ "message": message })),
                )
                .mount(&server)
                .await;

            let client = GitHubClient::new(server.uri(), "octo", "repo", "super-secret-token-value");
            let err = client.get_repository_info().await.unwrap_err();
            let rendered = err.to_string();

            assert!(rendered.contains(&code.to_string()), "status in: {rendered}");
            assert!(rendered.contains(message), "message in: {rendered}");
            assert!(
                !rendered.contains("super-secret-token-value"),
                "token leaked: {rendered}"
            );
            match code {
                401 | 403 => assert!(matches!(err, ProviderError::Auth(_))),
                404 => assert!(matches!(err, ProviderError::NotFound(_))),
                _ => unreachable!(),
            }
        }
    }
}
