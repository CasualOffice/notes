//! OS-keystore key management for the SQLCipher master key. Implements the key
//! custody rule of Data Model §13.1 / Architecture §7: *the DB key lives only in
//! the OS keystore (Keychain / Credential Manager / Secret Service), never in the
//! DB or logs.*
//!
//! Two backends:
//! - [`KeyringKeyStore`] — the real OS keystore via the `keyring` crate
//!   (feature `os-keystore`, on by default).
//! - [`DevFileKeyStore`] — a file-backed fallback for headless CI / dev boxes
//!   with no Secret Service. It is **not secret** and logs a warning; it exists
//!   only so the core builds and tests offline (CLAUDE.md local-first invariant).
//!
//! The 32-byte key is generated with the OS CSPRNG ([`getrandom`]) and stored as
//! a 64-char hex string.

use std::path::PathBuf;

use crate::db::KeyMaterial;
use crate::error::{StorageError, StorageResult};

/// Default keystore service/account labels.
pub const DEFAULT_SERVICE: &str = "app.casualnote";
pub const DEFAULT_ACCOUNT: &str = "db-master-key";

/// A backend that persists the SQLCipher master key.
pub trait KeyStore {
    /// Fetch the stored key, or `None` if none has been provisioned yet.
    fn get_db_key(&self) -> StorageResult<Option<KeyMaterial>>;
    /// Persist `key`, replacing any existing value.
    fn set_db_key(&self, key: &KeyMaterial) -> StorageResult<()>;
}

/// Generate a fresh 256-bit key from the OS CSPRNG.
pub fn generate_key() -> StorageResult<KeyMaterial> {
    let mut key = [0u8; 32];
    getrandom::getrandom(&mut key)
        .map_err(|e| StorageError::Keystore(format!("OS CSPRNG unavailable: {e}")))?;
    Ok(key)
}

/// Return the existing key, or mint + store one if none exists yet.
pub fn provision_db_key(store: &dyn KeyStore) -> StorageResult<KeyMaterial> {
    if let Some(k) = store.get_db_key()? {
        return Ok(k);
    }
    let key = generate_key()?;
    store.set_db_key(&key)?;
    Ok(key)
}

fn key_to_hex(key: &KeyMaterial) -> String {
    let mut s = String::with_capacity(64);
    for b in key {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn key_from_hex(s: &str) -> StorageResult<KeyMaterial> {
    let s = s.trim();
    if s.len() != 64 {
        return Err(StorageError::Keystore(format!(
            "stored key must be 64 hex chars, got {}",
            s.len()
        )));
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = hex_val(chunk[0])?;
        let lo = hex_val(chunk[1])?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_val(c: u8) -> StorageResult<u8> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(StorageError::Keystore(
            "non-hex character in stored key".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// OS-native backend (keyring)
// ---------------------------------------------------------------------------

/// The OS-native keystore backend.
#[cfg(feature = "os-keystore")]
#[derive(Clone, Debug)]
pub struct KeyringKeyStore {
    service: String,
    account: String,
}

#[cfg(feature = "os-keystore")]
impl KeyringKeyStore {
    /// A keystore entry under the default service/account labels.
    #[must_use]
    pub fn new() -> Self {
        Self {
            service: DEFAULT_SERVICE.to_string(),
            account: DEFAULT_ACCOUNT.to_string(),
        }
    }

    /// A keystore entry under custom labels.
    #[must_use]
    pub fn with_labels(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }

    fn entry(&self) -> StorageResult<keyring::Entry> {
        keyring::Entry::new(&self.service, &self.account)
            .map_err(|e| StorageError::Keystore(e.to_string()))
    }
}

#[cfg(feature = "os-keystore")]
impl Default for KeyringKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "os-keystore")]
impl KeyStore for KeyringKeyStore {
    fn get_db_key(&self) -> StorageResult<Option<KeyMaterial>> {
        match self.entry()?.get_password() {
            Ok(hex) => Ok(Some(key_from_hex(&hex)?)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(StorageError::Keystore(e.to_string())),
        }
    }

    fn set_db_key(&self, key: &KeyMaterial) -> StorageResult<()> {
        self.entry()?
            .set_password(&key_to_hex(key))
            .map_err(|e| StorageError::Keystore(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Dev fallback backend (file)
// ---------------------------------------------------------------------------

/// A file-backed dev fallback. **Not secret** — the key sits beside the DB. Use
/// only where an OS keystore is unavailable (headless CI, some Linux dev boxes).
#[derive(Clone, Debug)]
pub struct DevFileKeyStore {
    path: PathBuf,
}

impl DevFileKeyStore {
    /// Store the key hex at `path` (e.g. `<app_data>/.dev-db-key`).
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl KeyStore for DevFileKeyStore {
    fn get_db_key(&self) -> StorageResult<Option<KeyMaterial>> {
        match std::fs::read_to_string(&self.path) {
            Ok(hex) => Ok(Some(key_from_hex(&hex)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set_db_key(&self, key: &KeyMaterial) -> StorageResult<()> {
        tracing::warn!(
            path = %self.path.display(),
            "using the FILE-backed dev key fallback — the DB key is NOT protected by an OS keystore"
        );
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, key_to_hex(key))?;
        restrict_permissions(&self.path)?;
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) -> StorageResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) -> StorageResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_key_roundtrips() {
        let key = generate_key().unwrap();
        let hex = key_to_hex(&key);
        assert_eq!(hex.len(), 64);
        assert_eq!(key_from_hex(&hex).unwrap(), key);
    }

    #[test]
    fn dev_file_store_provisions_and_persists() {
        let mut b = [0u8; 8];
        getrandom::getrandom(&mut b).unwrap();
        let name: String = b.iter().map(|x| format!("{x:02x}")).collect();
        let path = std::env::temp_dir().join(format!("cn-key-{name}"));

        let store = DevFileKeyStore::new(&path);
        assert!(store.get_db_key().unwrap().is_none());
        let k1 = provision_db_key(&store).unwrap();
        let k2 = provision_db_key(&store).unwrap();
        assert_eq!(k1, k2, "second provision returns the stored key");

        std::fs::remove_file(&path).ok();
    }
}
