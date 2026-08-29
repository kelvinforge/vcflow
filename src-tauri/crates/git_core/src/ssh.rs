use git2::{Cred, Direction, RemoteCallbacks, Repository};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SshError {
    #[error("no remote named 'origin'")]
    NoOrigin,
    #[error("could not reach remote: {0}")]
    Unreachable(String),
}

/// SSH auth for `git@host:path` remotes has no credentials callback by
/// default -- git2 fails every SSH connection with "authentication failed"
/// unless one is wired up. Tries the running ssh-agent first (as `git` CLI
/// does), then falls back to the user's default key files -- macOS's `ssh`
/// also tries default identities even when the agent has none loaded, since
/// it can decrypt them via Keychain; git2/libssh2 has no Keychain access, so
/// this fallback only covers unencrypted keys.
/// libssh2 re-invokes the returned closure once per rejected attempt, so
/// its captured state must advance across calls -- a stateless "try agent,
/// then loop over keys" restarts from the same (already-rejected) key every
/// retry and burns the server's MaxAuthTries without ever reaching a later
/// key. Key order matches OpenSSH's real default (id_rsa before id_ed25519).
pub(crate) fn make_credentials_callback(
) -> impl FnMut(&str, Option<&str>, git2::CredentialType) -> Result<Cred, git2::Error> {
    let mut attempt = 0usize;
    move |_url, username_from_url, _allowed_types| {
        let username = username_from_url.unwrap_or("git");
        let this_attempt = attempt;
        attempt += 1;

        if this_attempt == 0 {
            if let Ok(cred) = Cred::ssh_key_from_agent(username) {
                return Ok(cred);
            }
        }

        let key_names = ["id_rsa", "id_ed25519"];
        if let Some(key_name) = key_names.get(this_attempt.saturating_sub(1)) {
            if let Some(home) = dirs::home_dir() {
                let private = home.join(".ssh").join(key_name);
                let public = home.join(".ssh").join(format!("{key_name}.pub"));
                if private.exists() {
                    if let Ok(cred) = Cred::ssh_key(username, Some(&public), &private, None) {
                        return Ok(cred);
                    }
                }
            }
        }
        Err(git2::Error::from_str("no usable SSH credentials found"))
    }
}

/// Attempts to connect to the repo's `origin` remote over its configured
/// transport (SSH or HTTPS) without fetching or pushing anything. Never
/// creates, uploads, or modifies keys -- read-only connectivity check.
pub fn validate_remote_connection(repo: &Repository) -> Result<(), SshError> {
    let mut remote = repo.find_remote("origin").map_err(|_| SshError::NoOrigin)?;

    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(make_credentials_callback());

    remote
        .connect_auth(Direction::Fetch, Some(callbacks), None)
        .map_err(|e| SshError::Unreachable(human_reason(&e)))?;

    remote.disconnect().ok();
    Ok(())
}

fn human_reason(err: &git2::Error) -> String {
    let msg = err.message();
    if msg.contains("Could not resolve host") || msg.contains("nodename nor servname") {
        "host not found -- check remote URL/DNS".to_string()
    } else if msg.contains("Permission denied") || msg.contains("authentication") {
        "authentication failed -- check SSH key/agent".to_string()
    } else if msg.contains("timed out") || msg.contains("timeout") {
        "connection timed out".to_string()
    } else {
        msg.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    fn run(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("failed to run git");
        assert!(status.success(), "git {:?} failed", args);
    }

    #[test]
    fn unreachable_remote_returns_descriptive_error() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init"]);
        // Port 1 on loopback refuses immediately -- avoids slow DNS-timeout paths.
        run(
            dir.path(),
            &["remote", "add", "origin", "http://127.0.0.1:1/repo.git"],
        );

        let repo = Repository::open(dir.path()).unwrap();
        let result = validate_remote_connection(&repo);
        assert!(result.is_err());
        assert!(matches!(result, Err(SshError::Unreachable(_))));
    }

    #[test]
    fn missing_origin_is_reported() {
        let dir = tempdir().unwrap();
        run(dir.path(), &["init"]);
        let repo = Repository::open(dir.path()).unwrap();
        assert!(matches!(
            validate_remote_connection(&repo),
            Err(SshError::NoOrigin)
        ));
    }
}
