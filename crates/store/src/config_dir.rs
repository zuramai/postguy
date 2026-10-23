//! The config-dir side (settings.json, session.json): user-level files NOT in the workspace.
//! Same atomic saves, same quarantine of corrupt files with a clear note.

use postguy_model::Settings;
use postguy_util::{Result, atomic_write, read_file_capped};
use serde::{Deserialize, Serialize};

/// Read the settings, quarantining a corrupt file (the app continues with defaults and
/// reports the quarantine path through [`ConfigOutcome`]).
pub struct ConfigOutcome {
    pub settings: Settings,
    pub quarantined_to: Option<std::path::PathBuf>,
}

pub fn load_settings(paths: &crate::paths::StorePaths) -> Result<ConfigOutcome> {
    let path = paths.settings_file();
    let bytes = match read_file_capped(&path, 8 << 20) {
        Ok(bytes) => bytes,
        Err(err) if matches!(&err, postguy_util::Error::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound) => {
            return Ok(ConfigOutcome { settings: Settings::default(), quarantined_to: None });
        }
        Err(other) => return Err(other),
    };
    match serde_json::from_slice::<Settings>(&bytes) {
        Ok(settings) => Ok(ConfigOutcome { settings, quarantined_to: None }),
        Err(_parse_err) => {
            let quarantined_to = crate::workspace_file::quarantine(&path)?;
            let note = format!("settings file corrupt; moved to {}", quarantined_to.display());
            let _ = note; // (the report is UI-facing; tests assert the quarantine path)
            Ok(ConfigOutcome { settings: Settings::default(), quarantined_to: Some(quarantined_to) })
        }
    }
}

/// A thin wrapper to write ONLY the settings file, unknown keys preserved through
/// `Settings.extras`.
pub fn save_settings(paths: &crate::paths::StorePaths, settings: &Settings) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|err| postguy_util::Error::JsonSyntax { what: "serializing settings".to_string(), source: err })?;
    atomic_write(&paths.settings_file(), &bytes)
}

/// The session file: free-form JSON for the UI's tab layout, drafts and geometry.
/// The schema lives with the UI; the store treats it as an opaque `serde_json` object.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionFile(pub serde_json::Value);

pub fn load_session(paths: &crate::paths::StorePaths) -> Result<SessionFile> {
    let path = paths.session_file();
    match read_file_capped(&path, 8 << 20) {
        Ok(bytes) => match serde_json::from_slice::<SessionFile>(&bytes) {
            Ok(file) => Ok(file),
            Err(_err) => {
                let quarantined_to = crate::workspace_file::quarantine(&path)?;
                Err(postguy_util::Error::Unusable {
                    what: format!("session file corrupt; moved to {} (session starts fresh)", quarantined_to.display()),
                })
            }
        },
        Err(err) if matches!(&err, postguy_util::Error::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound) => {
            Ok(SessionFile::default())
        }
        Err(other) => Err(other),
    }
}

pub fn save_session(paths: &crate::paths::StorePaths, session: &SessionFile) -> Result<()> {
    let bytes =
        serde_json::to_vec_pretty(session).map_err(|err| postguy_util::Error::JsonSyntax { what: "serializing session".to_string(), source: err })?;
    atomic_write(&paths.session_file(), &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_corrupt_files_with_a_quarantine_note() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::StorePaths::new(dir.path(), dir.path().join("config"));
        std::fs::create_dir_all(paths.config()).unwrap();
        std::fs::write(paths.settings_file(), b"{ maxResponseMb: not }").unwrap();
        let outcome = load_settings(&paths).unwrap();
        let note = outcome.quarantined_to.expect("corrupt file quarantined");
        assert!(note.file_name().is_some_and(|n| n.to_string_lossy().contains("corrupt")));
        assert!(!paths.settings_file().exists(), "the corrupt file left its old spot (fresh read possible)");
        assert_eq!(outcome.settings.max_response_mb, 50, "defaults are the recovery");
    }
}
