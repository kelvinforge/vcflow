use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::AuditLog;

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("credential not found")]
    NotFound,
    #[error("credential file cannot be decrypted on this machine -- save the token again")]
    Corrupt,
    #[error("credential storage error: {0}")]
    Storage(String),
}

impl From<std::io::Error> for CredentialError {
    fn from(e: std::io::Error) -> Self {
        CredentialError::Storage(e.to_string())
    }
}

/// Keychain service the tokens lived under before the encrypted file.
#[cfg(not(target_os = "macos"))]
const LEGACY_KEYRING_SERVICE: &str = "git-workflow-engine";
const NONCE_LEN: usize = 12;

/// Serializes read-modify-write of the credential file across commands, and
/// holds the keys whose legacy keychain item this process already tried.
// ponytail: in-process lock only; two app instances saving at the same
// instant can still lose one save. Add a file lock if that ever matters.
static FILE_LOCK: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

static MACHINE_KEY: OnceLock<[u8; 32]> = OnceLock::new();

/// Reads/writes provider access tokens, keyed by `provider|host|account`.
/// Never stores anything but the token string itself.
///
/// Tokens live in one AES-256-GCM encrypted file next to the audit database
/// (`credentials.enc`), not the OS keychain: a macOS keychain item only
/// trusts the build that created it, and ad-hoc signed builds change
/// identity on every update, so each update lost the token.
///
/// The key is derived from this machine's ID and the home path, so the file
/// copied on its own can't be decrypted elsewhere. The ID is not secret:
/// anyone with both the file and the machine ID (a full-disk backup, code
/// running as the same user) can rebuild the key. Accepted tradeoff, see
/// bugs/keychain-access-after-update in the vault.
pub struct CredentialStore;

impl CredentialStore {
    fn key(provider: &str, host: &str, account: &str) -> String {
        format!("{provider}|{host}|{account}")
    }

    fn file() -> Result<CredentialFile, CredentialError> {
        let path = AuditLog::default_path()
            .map_err(|e| CredentialError::Storage(e.to_string()))?
            .with_file_name("credentials.enc");
        Ok(CredentialFile::new(path, machine_key()?))
    }

    pub fn get(provider: &str, host: &str, account: &str) -> Result<String, CredentialError> {
        let key = Self::key(provider, host, account);
        let mut tried = FILE_LOCK.lock().expect("credential file lock poisoned");
        let file = Self::file()?;
        match file.get(&key) {
            // Migration: a token saved before the encrypted file may still
            // sit in the OS keychain. Try it once per key per process, so a
            // missing token doesn't hit the keychain on every command.
            Err(CredentialError::NotFound) if tried.insert(key.clone()) => {
                let token = legacy_token(&key).ok_or(CredentialError::NotFound)?;
                // A failed copy must not lose the token we just read: return
                // it anyway and let the next read retry the copy.
                if file.set(&key, &token).is_err() {
                    tried.remove(&key);
                }
                Ok(token)
            }
            other => other,
        }
    }

    pub fn set(
        provider: &str,
        host: &str,
        account: &str,
        token: &str,
    ) -> Result<(), CredentialError> {
        let _guard = FILE_LOCK.lock().expect("credential file lock poisoned");
        Self::file()?.set(&Self::key(provider, host, account), token)
    }

    pub fn delete(provider: &str, host: &str, account: &str) -> Result<(), CredentialError> {
        let key = Self::key(provider, host, account);
        let _guard = FILE_LOCK.lock().expect("credential file lock poisoned");
        // Drop the legacy keychain copy too, or `get`'s migration would
        // bring a deleted token back.
        let legacy_deleted = delete_legacy(&key);
        match Self::file()?.delete(&key) {
            Err(CredentialError::NotFound) if legacy_deleted => Ok(()),
            other => other,
        }
    }
}

/// Token from the pre-encrypted-file OS keychain item, if readable.
/// macOS: never tried. The item only trusts the build that created it (the
/// bug this file fixes), so the read fails and can raise an access prompt.
#[cfg(target_os = "macos")]
fn legacy_token(_key: &str) -> Option<String> {
    None
}

#[cfg(not(target_os = "macos"))]
fn legacy_token(key: &str) -> Option<String> {
    keyring::Entry::new(LEGACY_KEYRING_SERVICE, key)
        .and_then(|e| e.get_password())
        .ok()
}

/// Best-effort delete of the legacy keychain item; true if one was removed.
#[cfg(target_os = "macos")]
fn delete_legacy(_key: &str) -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
fn delete_legacy(key: &str) -> bool {
    keyring::Entry::new(LEGACY_KEYRING_SERVICE, key)
        .and_then(|e| e.delete_credential())
        .is_ok()
}

/// 32-byte key from SHA-256 over this machine's ID and the user's home
/// path (separates OS users sharing a machine). Computed once per process:
/// on macOS reading the ID spawns `ioreg`. A failure isn't cached.
fn machine_key() -> Result<[u8; 32], CredentialError> {
    if let Some(key) = MACHINE_KEY.get() {
        return Ok(*key);
    }
    let id = machine_uid::get().map_err(|e| CredentialError::Storage(e.to_string()))?;
    let home = dirs::home_dir()
        .ok_or_else(|| CredentialError::Storage("no home directory".into()))?;
    let mut h = Sha256::new();
    h.update(b"vcflow-credentials-v1\0");
    h.update(id.as_bytes());
    h.update(b"\0");
    h.update(home.as_os_str().as_encoded_bytes());
    Ok(*MACHINE_KEY.get_or_init(|| h.finalize().into()))
}

/// The encrypted token file: `nonce (12 bytes) || AES-256-GCM ciphertext`
/// of a TOML table `key = token`. Rewritten whole on every change.
pub struct CredentialFile {
    path: PathBuf,
    key: [u8; 32],
}

impl CredentialFile {
    pub fn new(path: PathBuf, key: [u8; 32]) -> Self {
        Self { path, key }
    }

    pub fn get(&self, key: &str) -> Result<String, CredentialError> {
        self.load()?.remove(key).ok_or(CredentialError::NotFound)
    }

    pub fn set(&self, key: &str, token: &str) -> Result<(), CredentialError> {
        let mut map = self.load_or_empty()?;
        map.insert(key.to_string(), token.to_string());
        self.save(&map)
    }

    pub fn delete(&self, key: &str) -> Result<(), CredentialError> {
        let mut map = self.load_or_empty()?;
        if map.remove(key).is_none() {
            return Err(CredentialError::NotFound);
        }
        self.save(&map)
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.key))
    }

    fn load(&self) -> Result<HashMap<String, String>, CredentialError> {
        let raw = match std::fs::read(&self.path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(e) => return Err(e.into()),
        };
        if raw.len() < NONCE_LEN {
            return Err(CredentialError::Corrupt);
        }
        let (nonce, ciphertext) = raw.split_at(NONCE_LEN);
        let plain = self
            .cipher()
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| CredentialError::Corrupt)?;
        let text = String::from_utf8(plain).map_err(|_| CredentialError::Corrupt)?;
        toml::from_str(&text).map_err(|_| CredentialError::Corrupt)
    }

    /// A file this machine can't decrypt (machine ID or home path changed,
    /// copied from elsewhere, tampered) must not block saving a token, so
    /// writes start fresh -- after moving it to `credentials.enc.bak`, so
    /// the old tokens stay recoverable if the key comes back.
    fn load_or_empty(&self) -> Result<HashMap<String, String>, CredentialError> {
        match self.load() {
            Err(CredentialError::Corrupt) => {
                std::fs::rename(&self.path, self.path.with_extension("enc.bak"))?;
                Ok(HashMap::new())
            }
            other => other,
        }
    }

    fn save(&self, map: &HashMap<String, String>) -> Result<(), CredentialError> {
        let text = toml::to_string(map).map_err(|e| CredentialError::Storage(e.to_string()))?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher()
            .encrypt(&nonce, text.as_bytes())
            .map_err(|e| CredentialError::Storage(e.to_string()))?;
        let mut data = nonce.to_vec();
        data.extend_from_slice(&ciphertext);
        write_private(&self.path, &data)
    }
}

/// Write via a temp file + rename (no half-written file on crash), with
/// owner-only permissions on Unix.
fn write_private(path: &Path, data: &[u8]) -> Result<(), CredentialError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Per-process temp name: two instances never write into the same file.
    let tmp = path.with_extension(format!("enc.{}.tmp", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &tempfile::TempDir, key: [u8; 32]) -> CredentialFile {
        CredentialFile::new(dir.path().join("credentials.enc"), key)
    }

    #[test]
    fn round_trips_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let f = store(&dir, [7; 32]);
        f.set("gitlab|172.16.18.167|alice", "s3cr3t").unwrap();
        assert_eq!(f.get("gitlab|172.16.18.167|alice").unwrap(), "s3cr3t");
    }

    #[test]
    fn token_is_not_stored_in_plain_text() {
        let dir = tempfile::tempdir().unwrap();
        let f = store(&dir, [7; 32]);
        f.set("gitlab|host|bob", "plain-token-value").unwrap();
        let raw = std::fs::read(dir.path().join("credentials.enc")).unwrap();
        let needle = b"plain-token-value";
        assert!(!raw.windows(needle.len()).any(|w| w == needle));
    }

    #[test]
    fn delete_removes_only_that_entry() {
        let dir = tempfile::tempdir().unwrap();
        let f = store(&dir, [7; 32]);
        f.set("github|example.com|default", "gh-token").unwrap();
        f.set("gitlab|example.com|default", "gl-token").unwrap();
        f.delete("github|example.com|default").unwrap();
        assert!(matches!(f.get("github|example.com|default"), Err(CredentialError::NotFound)));
        assert_eq!(f.get("gitlab|example.com|default").unwrap(), "gl-token");
    }

    #[test]
    fn missing_file_or_entry_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let f = store(&dir, [7; 32]);
        assert!(matches!(f.get("gitlab|host|nobody"), Err(CredentialError::NotFound)));
        f.set("gitlab|host|someone", "x").unwrap();
        assert!(matches!(f.get("gitlab|host|nobody"), Err(CredentialError::NotFound)));
    }

    #[test]
    fn file_from_another_machine_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir, [7; 32]).set("gitlab|host|bob", "tok").unwrap();
        let other = store(&dir, [8; 32]);
        assert!(matches!(other.get("gitlab|host|bob"), Err(CredentialError::Corrupt)));
    }

    #[test]
    fn tampered_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let f = store(&dir, [7; 32]);
        f.set("gitlab|host|bob", "tok").unwrap();
        let path = dir.path().join("credentials.enc");
        let mut raw = std::fs::read(&path).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0x01;
        std::fs::write(&path, raw).unwrap();
        assert!(matches!(f.get("gitlab|host|bob"), Err(CredentialError::Corrupt)));
    }

    #[test]
    fn saving_over_an_unreadable_file_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        store(&dir, [7; 32]).set("gitlab|host|old", "lost").unwrap();
        let unreadable = std::fs::read(dir.path().join("credentials.enc")).unwrap();
        let f = store(&dir, [8; 32]);
        f.set("gitlab|host|bob", "new").unwrap();
        assert_eq!(f.get("gitlab|host|bob").unwrap(), "new");
        assert!(matches!(f.get("gitlab|host|old"), Err(CredentialError::NotFound)));
        // The unreadable file is kept, not destroyed: if the machine key
        // changed, the old tokens are still recoverable from the backup.
        let backup = std::fs::read(dir.path().join("credentials.enc.bak")).unwrap();
        assert_eq!(backup, unreadable);
        let old = CredentialFile::new(dir.path().join("credentials.enc.bak"), [7; 32]);
        assert_eq!(old.get("gitlab|host|old").unwrap(), "lost");
    }

    #[test]
    fn machine_key_is_stable_on_this_machine() {
        assert_eq!(machine_key().unwrap(), machine_key().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn file_is_readable_by_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        store(&dir, [7; 32]).set("gitlab|host|bob", "tok").unwrap();
        let mode = std::fs::metadata(dir.path().join("credentials.enc")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
