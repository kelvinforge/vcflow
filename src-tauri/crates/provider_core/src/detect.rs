/// Which Git hosting provider a remote URL points at.
///
/// Detection is URL-pattern-only and deliberately conservative: only the
/// well-known SaaS hostnames (`gitlab.com`, `github.com`) are classified.
/// Any other host -- including a self-hosted GitLab instance -- comes back
/// `Unknown` here; confirming a self-hosted host is actually GitLab requires
/// an API probe (see `GitLabProvider`), not a hostname guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    GitLab,
    GitHub,
    Unknown,
}

/// Classifies a remote URL in either `git@host:owner/repo.git` (SCP-like SSH)
/// or `https://host/owner/repo.git` form.
pub fn detect_provider(remote_url: &str) -> Provider {
    match extract_host(remote_url) {
        Some(host) => classify_host(&host),
        None => Provider::Unknown,
    }
}

fn extract_host(url: &str) -> Option<String> {
    if let Some(rest) = url.strip_prefix("git@") {
        // git@host:owner/repo.git
        rest.split(':').next().map(str::to_lowercase)
    } else if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("ssh://git@"))
    {
        // host/owner/repo.git or host:port/owner/repo.git
        rest.split(['/', ':']).next().map(str::to_lowercase)
    } else {
        None
    }
}

fn classify_host(host: &str) -> Provider {
    if host == "gitlab.com" || host.ends_with(".gitlab.com") {
        Provider::GitLab
    } else if host == "github.com" || host.ends_with(".github.com") {
        Provider::GitHub
    } else {
        Provider::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_known_url_forms() {
        let cases = [
            ("git@gitlab.com:group/repo.git", Provider::GitLab),
            ("https://gitlab.com/group/repo.git", Provider::GitLab),
            ("git@github.com:owner/repo.git", Provider::GitHub),
            ("https://github.com/owner/repo.git", Provider::GitHub),
        ];
        for (url, expected) in cases {
            assert_eq!(detect_provider(url), expected, "url: {url}");
        }
    }

    #[test]
    fn self_hosted_host_is_unknown_not_guessed() {
        let cases = [
            "git@172.16.18.167:group/repo.git",
            "https://172.16.18.167/group/repo.git",
            "git@git.internal.example.com:group/repo.git",
        ];
        for url in cases {
            assert_eq!(detect_provider(url), Provider::Unknown, "url: {url}");
        }
    }

    #[test]
    fn garbage_input_is_unknown() {
        assert_eq!(detect_provider("not a url"), Provider::Unknown);
        assert_eq!(detect_provider(""), Provider::Unknown);
    }
}
