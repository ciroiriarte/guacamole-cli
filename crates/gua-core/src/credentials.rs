//! Pluggable credential (auth-token) storage with expiry (issue #7).
//!
//! Tokens are held in a [`secrecy::SecretString`] so they are zeroized on drop
//! and never accidentally logged. Two backends implement [`CredentialStore`]:
//!
//! - [`KeyringStore`] — the OS secret store, behind the `keyring-store` feature
//!   (on by default). The secure path.
//! - [`FileStore`] — a per-profile file under the OS data dir, `0600` on Unix.
//!   Always compiled; the fallback, and the only backend in a
//!   `--no-default-features` (keyring-free) build.
//!
//! [`default_store`] returns the keyring backend when the feature is enabled and
//! the file backend otherwise.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use crate::error::Error;

/// An authentication token with optional expiry.
#[derive(Debug)]
pub struct Token {
    /// The opaque `authToken` value from the gateway.
    pub value: SecretString,
    /// Expiry as seconds since the Unix epoch, if known.
    pub expires_at: Option<u64>,
}

impl Token {
    /// Create a token that never auto-expires locally.
    pub fn new(value: impl Into<String>) -> Self {
        Token {
            value: SecretString::new(value.into()),
            expires_at: None,
        }
    }

    /// Create a token that expires `ttl_secs` seconds from now.
    pub fn with_ttl(value: impl Into<String>, ttl_secs: u64) -> Self {
        Token {
            value: SecretString::new(value.into()),
            expires_at: Some(now_epoch().saturating_add(ttl_secs)),
        }
    }

    /// Whether the token is known to be expired (false if no expiry is set).
    pub fn is_expired(&self) -> bool {
        matches!(self.expires_at, Some(exp) if now_epoch() >= exp)
    }
}

/// On-disk / in-keyring representation (secret exposed only for persistence).
#[derive(Serialize, Deserialize)]
struct StoredToken {
    token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
}

impl StoredToken {
    fn from_token(t: &Token) -> Self {
        StoredToken {
            token: t.value.expose_secret().clone(),
            expires_at: t.expires_at,
        }
    }
    fn into_token(self) -> Token {
        Token {
            value: SecretString::new(self.token),
            expires_at: self.expires_at,
        }
    }
}

/// A backend that persists auth tokens per profile.
pub trait CredentialStore {
    /// Persist `token` for `profile`, replacing any existing value.
    fn store_token(&self, profile: &str, token: &Token) -> crate::Result<()>;
    /// Load the token for `profile`, returning `None` if absent or expired.
    fn load_token(&self, profile: &str) -> crate::Result<Option<Token>>;
    /// Remove any stored token for `profile` (no error if absent).
    fn delete_token(&self, profile: &str) -> crate::Result<()>;
}

/// File-backed credential store (per-profile file, `0600` on Unix).
#[derive(Debug, Clone)]
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    /// Store under the OS data dir (`<data>/tokens`).
    pub fn new() -> crate::Result<Self> {
        let dirs = ProjectDirs::from("io.github", "ciroiriarte", "guacamole-cli")
            .ok_or_else(|| Error::Credential("cannot determine OS data directory".into()))?;
        Ok(Self::with_dir(dirs.data_dir().join("tokens")))
    }

    /// Store under an explicit directory (used in tests).
    pub fn with_dir(dir: PathBuf) -> Self {
        FileStore { dir }
    }

    fn path_for(&self, profile: &str) -> PathBuf {
        // Profile names are simple identifiers; sanitize path separators defensively.
        let safe = profile.replace(['/', '\\'], "_");
        self.dir.join(format!("{safe}.json"))
    }
}

impl CredentialStore for FileStore {
    fn store_token(&self, profile: &str, token: &Token) -> crate::Result<()> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| Error::Credential(format!("creating {}: {e}", self.dir.display())))?;
        let path = self.path_for(profile);
        let json = serde_json::to_vec_pretty(&StoredToken::from_token(token))
            .map_err(|e| Error::Credential(format!("serializing token: {e}")))?;
        std::fs::write(&path, json)
            .map_err(|e| Error::Credential(format!("writing {}: {e}", path.display())))?;
        restrict_permissions(&path)?;
        Ok(())
    }

    fn load_token(&self, profile: &str) -> crate::Result<Option<Token>> {
        let path = self.path_for(profile);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(Error::Credential(format!(
                    "reading {}: {e}",
                    path.display()
                )))
            }
        };
        let stored: StoredToken = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Credential(format!("parsing {}: {e}", path.display())))?;
        let token = stored.into_token();
        if token.is_expired() {
            // Proactively drop expired material.
            let _ = self.delete_token(profile);
            return Ok(None);
        }
        Ok(Some(token))
    }

    fn delete_token(&self, profile: &str) -> crate::Result<()> {
        let path = self.path_for(profile);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Credential(format!(
                "removing {}: {e}",
                path.display()
            ))),
        }
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) -> crate::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
        .map_err(|e| Error::Credential(format!("chmod {}: {e}", path.display())))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) -> crate::Result<()> {
    // Best-effort only on non-Unix; rely on the user profile directory ACLs.
    Ok(())
}

/// OS keyring-backed credential store.
#[cfg(feature = "keyring-store")]
#[derive(Debug, Clone)]
pub struct KeyringStore {
    service: String,
}

#[cfg(feature = "keyring-store")]
impl Default for KeyringStore {
    fn default() -> Self {
        KeyringStore {
            service: "guacamole-cli".to_string(),
        }
    }
}

#[cfg(feature = "keyring-store")]
impl CredentialStore for KeyringStore {
    fn store_token(&self, profile: &str, token: &Token) -> crate::Result<()> {
        let entry = keyring::Entry::new(&self.service, profile)
            .map_err(|e| Error::Credential(e.to_string()))?;
        let json = serde_json::to_string(&StoredToken::from_token(token))
            .map_err(|e| Error::Credential(format!("serializing token: {e}")))?;
        entry
            .set_password(&json)
            .map_err(|e| Error::Credential(e.to_string()))
    }

    fn load_token(&self, profile: &str) -> crate::Result<Option<Token>> {
        let entry = keyring::Entry::new(&self.service, profile)
            .map_err(|e| Error::Credential(e.to_string()))?;
        let json = match entry.get_password() {
            Ok(s) => s,
            Err(keyring::Error::NoEntry) => return Ok(None),
            Err(e) => return Err(Error::Credential(e.to_string())),
        };
        let stored: StoredToken =
            serde_json::from_str(&json).map_err(|e| Error::Credential(e.to_string()))?;
        let token = stored.into_token();
        if token.is_expired() {
            let _ = self.delete_token(profile);
            return Ok(None);
        }
        Ok(Some(token))
    }

    fn delete_token(&self, profile: &str) -> crate::Result<()> {
        let entry = keyring::Entry::new(&self.service, profile)
            .map_err(|e| Error::Credential(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::Credential(e.to_string())),
        }
    }
}

/// The default store: keyring when the `keyring-store` feature is on, else file.
pub fn default_store() -> crate::Result<Box<dyn CredentialStore>> {
    #[cfg(feature = "keyring-store")]
    {
        Ok(Box::new(KeyringStore::default()))
    }
    #[cfg(not(feature = "keyring-store"))]
    {
        Ok(Box::new(FileStore::new()?))
    }
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_round_trips_and_expires() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::with_dir(dir.path().join("tokens"));

        store
            .store_token("prod", &Token::new("SECRETTOKEN"))
            .unwrap();
        let got = store.load_token("prod").unwrap().unwrap();
        assert_eq!(got.value.expose_secret(), "SECRETTOKEN");

        // Absent profile is None, not an error.
        assert!(store.load_token("missing").unwrap().is_none());

        // An already-expired token loads as None and is removed.
        let expired = Token {
            value: SecretString::new("OLD".into()),
            expires_at: Some(0),
        };
        store.store_token("stale", &expired).unwrap();
        assert!(store.load_token("stale").unwrap().is_none());

        store.delete_token("prod").unwrap();
        assert!(store.load_token("prod").unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn file_store_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::with_dir(dir.path().to_path_buf());
        store.store_token("p", &Token::new("x")).unwrap();
        let meta = std::fs::metadata(store.path_for("p")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }
}
