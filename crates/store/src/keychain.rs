//! Secrets that never belong in a file: the AI Agent API keys live in the OS credential store
//! (macOS Keychain, Windows Credential Manager, Secret Service on Linux/FreeBSD).
//!
//! [`KeyStore`] is the seam: the engine uses [`OsKeyStore`] by default and tests inject
//! [`MemoryKeyStore`]. Secrets never appear in an error or log string.

use std::collections::HashMap;
use std::sync::Mutex;

use postguy_model::AiProvider;
use postguy_util::{Error, Result};

/// The service name every Postguy credential is stored under.
pub const SERVICE: &str = "postguy";

/// The keychain account that holds the API key of one provider.
pub fn ai_account(provider: AiProvider) -> String {
    format!("ai-agent/{}", provider.slug())
}

/// A small credential store keyed by account name.
pub trait KeyStore: Send + Sync {
    /// The stored secret, or `None` when the account has none.
    fn get(&self, account: &str) -> Result<Option<String>>;
    fn set(&self, account: &str, secret: &str) -> Result<()>;
    /// Remove the secret; removing a missing secret succeeds.
    fn delete(&self, account: &str) -> Result<()>;
}

/// The OS credential store.
///
/// Call it only from worker threads: never from the UI thread (macOS may show an access
/// prompt and block) and never from inside a tokio runtime (the Secret Service client runs
/// its own).
#[derive(Debug, Default, Clone, Copy)]
pub struct OsKeyStore;

fn unavailable(err: &keyring::Error) -> Error {
    Error::Unusable {
        what: format!("the system keychain is unavailable ({err}); unlock or install a keychain (Secret Service on Linux/FreeBSD) and try again"),
    }
}

impl KeyStore for OsKeyStore {
    fn get(&self, account: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(SERVICE, account).map_err(|err| unavailable(&err))?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(unavailable(&err)),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<()> {
        let entry = keyring::Entry::new(SERVICE, account).map_err(|err| unavailable(&err))?;
        entry.set_password(secret).map_err(|err| unavailable(&err))
    }

    fn delete(&self, account: &str) -> Result<()> {
        let entry = keyring::Entry::new(SERVICE, account).map_err(|err| unavailable(&err))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(unavailable(&err)),
        }
    }
}

/// An in-process store for tests and embedders that must not touch the OS keychain.
#[derive(Debug, Default)]
pub struct MemoryKeyStore(Mutex<HashMap<String, String>>);

impl MemoryKeyStore {
    fn with<T>(&self, f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
        // A poisoned lock still holds a consistent map: every operation is a single call.
        let mut map = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut map)
    }
}

impl KeyStore for MemoryKeyStore {
    fn get(&self, account: &str) -> Result<Option<String>> {
        Ok(self.with(|map| map.get(account).cloned()))
    }

    fn set(&self, account: &str, secret: &str) -> Result<()> {
        self.with(|map| map.insert(account.to_string(), secret.to_string()));
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<()> {
        self.with(|map| map.remove(account));
        Ok(())
    }
}
