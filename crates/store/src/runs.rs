//! Stored collection runs: `.postguy/runs/<collection-id>/<run-id>.json`, newest 20 per
//! collection. Run ids sort by time, so "newest" is "last in name order".

use std::path::PathBuf;

use postguy_util::{Error, Result, atomic_write, read_file_capped};

use crate::workspace_file::Workspace;

/// Runs kept per collection.
pub const RUNS_PER_COLLECTION: usize = 20;
/// Largest stored run read back.
pub const RUN_FILE_CAP: usize = 32 << 20;

fn safe(name: &str, what: &str) -> Result<()> {
    if name.is_empty() || name.len() > 128 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(Error::Unusable { what: format!("{what} `{name}` may only contain letters, digits, `-` and `_`") });
    }
    Ok(())
}

impl Workspace {
    fn runs_dir_of(&self, collection: &str) -> Result<PathBuf> {
        safe(collection, "collection id")?;
        Ok(self.paths.runs_dir().join(collection))
    }

    /// Write one run atomically and prune the collection's folder to the newest 20 runs.
    pub fn save_run(&self, collection: &str, run_id: &str, bytes: &[u8]) -> Result<()> {
        safe(run_id, "run id")?;
        let dir = self.runs_dir_of(collection)?;
        atomic_write(&dir.join(format!("{run_id}.json")), bytes)?;
        let mut ids = self.run_ids(collection)?;
        // Newest first: everything past the cap goes.
        for old in ids.split_off(RUNS_PER_COLLECTION.min(ids.len())) {
            let _ = std::fs::remove_file(dir.join(format!("{old}.json")));
        }
        Ok(())
    }

    /// The stored run ids of a collection, newest first. A missing folder is an empty list.
    pub fn run_ids(&self, collection: &str) -> Result<Vec<String>> {
        let dir = self.runs_dir_of(collection)?;
        let Ok(entries) = std::fs::read_dir(&dir) else { return Ok(Vec::new()) };
        let mut ids: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().and_then(|name| name.strip_suffix(".json")).map(str::to_string))
            .filter(|id| safe(id, "run id").is_ok())
            .collect();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        Ok(ids)
    }

    /// One stored run's JSON bytes.
    pub fn load_run(&self, collection: &str, run_id: &str) -> Result<Vec<u8>> {
        safe(run_id, "run id")?;
        read_file_capped(&self.runs_dir_of(collection)?.join(format!("{run_id}.json")), RUN_FILE_CAP)
    }
}
