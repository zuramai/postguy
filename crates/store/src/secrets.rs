//! The secrets store: `(owner id, key) -> value` for every variable
//! marked secret, in `.postguy/secrets.json` (0600 on unix). Secret values NEVER live inside
//! collection or environment files — this file is the only on-disk place, and v1 does not
//! encrypt at rest (documented; the OS keychain is a P2 decision for the owner).

use std::collections::BTreeMap;
use std::path::Path;

use postguy_util::{Result, atomic_write, read_file_capped};
use serde::{Deserialize, Serialize};

/// Namespaced by owner (an environment id or a collection id), keys plain.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretFile(pub BTreeMap<String, BTreeMap<String, String>>);

/// The open store; writes go through the workspace handle discipline (atomic + one `.bak`).
#[derive(Debug)]
pub struct SecretsStore {
    path: std::path::PathBuf,
    file: std::sync::Mutex<SecretFile>,
}

impl SecretsStore {
    /// Open (or create) the secrets file. Missing -> empty store; corrupt -> quarantined by
    /// the caller path in `workspace_file::read_model` conventions (here: same quarantine).
    pub fn open(paths: &crate::paths::StorePaths) -> Result<SecretsStore> {
        let path = paths.secrets_file();
        let file = match read_file_capped(&path, 8 << 20) {
            Ok(bytes) => match serde_json::from_slice::<SecretFile>(&bytes) {
                Ok(file) => file,
                Err(_parse_err) => {
                    let quarantined = crate::workspace_file::quarantine(&path)?;
                    return Err(postguy_util::Error::Unusable { what: format!("secrets file was corrupt; moved to {}", quarantined.display()) });
                }
            },
            Err(err) if matches!(&err, postguy_util::Error::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound) => {
                SecretFile::default()
            }
            Err(other) => return Err(other),
        };
        if path.exists() {
            let _ = lockdown(&path);
        }
        Ok(SecretsStore { path, file: std::sync::Mutex::new(file) })
    }

    /// Read every value of `owner`.
    pub fn values_for(&self, owner: &str) -> Result<BTreeMap<String, String>> {
        Ok(self.file_guard().0.get(owner).cloned().unwrap_or_default())
    }

    /// Upsert one (owner, key, value); pass-through empty value deletes (fidelity rule: an
    /// empty secret carries nothing to keep).
    pub fn set(&self, owner: &str, key: &str, value: Option<&str>) -> Result<()> {
        let mut guard = self.file_guard();
        let owner_entry = guard.0.entry(owner.to_string()).or_default();
        let action = match value {
            Some(value) if !value.is_empty() => owner_entry.insert(key.to_string(), value.to_string()),
            _ => owner_entry.remove(key),
        };
        let _ = action;
        drop(guard);
        self.flush()
    }

    /// Remove `owner` entirely (deleting an environment / collection purges its secrets).
    pub fn clear_owner(&self, owner: &str) -> Result<()> {
        let mut guard = self.file_guard();
        guard.0.remove(owner);
        drop(guard);
        self.flush()
    }

    /// A guard that recovers from poison instead of panicking (AGENTS.md 4).
    fn file_guard(&self) -> std::sync::MutexGuard<'_, SecretFile> {
        self.file.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn flush(&self) -> Result<()> {
        let guard = self.file_guard();
        let bytes = serde_json::to_vec_pretty(&*guard)
            .map_err(|err| postguy_util::Error::JsonSyntax { what: format!("serializing {}", self.path.display()), source: err })?;
        drop(guard);
        atomic_write(&self.path, &bytes)?;
        let _ = lockdown(&self.path);
        Ok(())
    }
}

/// 0600 on unix (owner-only); a no-op on Windows where the ACLs are the OS's business.
pub fn lockdown(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .map_err(|source| postguy_util::Error::Io { operation: format!("stating {}", path.display()), source })?
            .permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms)
            .map_err(|source| postguy_util::Error::Io { operation: format!("locking down {}", path.display()), source })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_round_trip_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::StorePaths::new(dir.path(), dir.path().join("config"));
        let store = SecretsStore::open(&paths).unwrap();
        store.set("env-1", "apiKey", Some("v3ry-s3cret")).unwrap();
        store.set("env-1", "blank-rule", Some("")).unwrap();
        assert_eq!(store.values_for("env-1").unwrap().get("apiKey"), Some(&"v3ry-s3cret".to_string()));
        // The empty value deleted the key instead of storing junk.
        assert!(!store.values_for("env-1").unwrap().contains_key("blank-rule"));
        // On disk: 0600, and the value is not in the tree outside .postguy.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(paths.secrets_file()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "secrets file stays owner-only");
        }
    }
}
