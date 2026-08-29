use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use keyring::Entry;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("credential not found")]
    NotFound,
    #[error("keyring backend error: {0}")]
    Backend(#[from] keyring::Error),
}

fn entries() -> &'static Mutex<HashMap<String, Entry>> {
    static ENTRIES: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    ENTRIES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Reads/writes provider access tokens in the OS keychain, keyed by
/// `provider|host|account`. Never stores anything but the token string
/// itself.
///
/// Entries are cached by key rather than recreated per call: the real OS
/// backends (Keychain/Secret Service/Credential Manager) persist by key
/// regardless, but keyring's platform-independent mock backend used in tests
/// only persists per `Entry` instance -- caching keeps get/set/delete
/// operating on the same logical credential either way.
pub struct CredentialStore;

impl CredentialStore {
    fn key(provider: &str, host: &str, account: &str) -> String {
        format!("{provider}|{host}|{account}")
    }

    fn with_entry<T>(
        provider: &str,
        host: &str,
        account: &str,
        f: impl FnOnce(&Entry) -> keyring::Result<T>,
    ) -> Result<T, CredentialError> {
        let key = Self::key(provider, host, account);
        let mut map = entries().lock().expect("credential entry cache poisoned");
        let entry = match map.get(&key) {
            Some(e) => e,
            None => {
                let new_entry = Entry::new("git-workflow-engine", &key)?;
                map.entry(key).or_insert(new_entry)
            }
        };
        f(entry).map_err(|e| match e {
            keyring::Error::NoEntry => CredentialError::NotFound,
            other => CredentialError::Backend(other),
        })
    }

    pub fn get(provider: &str, host: &str, account: &str) -> Result<String, CredentialError> {
        Self::with_entry(provider, host, account, |e| e.get_password())
    }

    pub fn set(
        provider: &str,
        host: &str,
        account: &str,
        token: &str,
    ) -> Result<(), CredentialError> {
        Self::with_entry(provider, host, account, |e| e.set_password(token))
    }

    pub fn delete(provider: &str, host: &str, account: &str) -> Result<(), CredentialError> {
        Self::with_entry(provider, host, account, |e| e.delete_credential())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyring::{mock, set_default_credential_builder};
    use std::sync::Once;

    static INIT: Once = Once::new();

    /// Swaps in keyring's platform-independent mock backend so tests never
    /// touch the real OS keychain (no permission prompts, no CI flakiness).
    /// Must run before the first `Entry` is created anywhere in this binary.
    fn use_mock_backend() {
        INIT.call_once(|| {
            set_default_credential_builder(mock::default_credential_builder());
        });
    }

    #[test]
    fn round_trips_a_token() {
        use_mock_backend();
        CredentialStore::set("gitlab", "172.16.18.167", "alice", "s3cr3t").unwrap();
        let token = CredentialStore::get("gitlab", "172.16.18.167", "alice").unwrap();
        assert_eq!(token, "s3cr3t");
    }

    #[test]
    fn delete_removes_the_entry() {
        use_mock_backend();
        CredentialStore::set("gitlab", "host", "bob", "tok").unwrap();
        CredentialStore::delete("gitlab", "host", "bob").unwrap();
        assert!(matches!(
            CredentialStore::get("gitlab", "host", "bob"),
            Err(CredentialError::NotFound)
        ));
    }

    #[test]
    fn github_and_gitlab_entries_for_one_host_are_independent() {
        use_mock_backend();
        CredentialStore::set("github", "example.com", "default", "gh-token").unwrap();
        CredentialStore::set("gitlab", "example.com", "default", "gl-token").unwrap();
        assert_eq!(
            CredentialStore::get("github", "example.com", "default").unwrap(),
            "gh-token"
        );
        assert_eq!(
            CredentialStore::get("gitlab", "example.com", "default").unwrap(),
            "gl-token"
        );
        CredentialStore::delete("github", "example.com", "default").unwrap();
        assert!(matches!(
            CredentialStore::get("github", "example.com", "default"),
            Err(CredentialError::NotFound)
        ));
        // Deleting the GitHub entry must not touch the GitLab one.
        assert_eq!(
            CredentialStore::get("gitlab", "example.com", "default").unwrap(),
            "gl-token"
        );
    }

    #[test]
    fn missing_entry_is_not_found() {
        use_mock_backend();
        assert!(matches!(
            CredentialStore::get("gitlab", "host", "never-set-xyz"),
            Err(CredentialError::NotFound)
        ));
    }
}
