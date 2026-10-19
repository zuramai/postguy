//! The workspace handle: open (or create) the folder, load and save everything it owns, and
//! keep the collection/environment ORDER tidy. Saves are atomic (postguy-util::atomic_write),
//! corrupt files are quarantined as `.corrupt-<time>` and REPORTED, never silently dropped.

use std::path::PathBuf;
use std::sync::Mutex;

use postguy_model::{Collection, Environment, HistoryEntry, Id};
use postguy_util::{Result, atomic_write, read_file_capped};
use serde::{Deserialize, Serialize};

use crate::paths::StorePaths;
use crate::secrets::SecretsStore;
use crate::{history, lock, trash};

const MAX_FILE_BYTES: usize = 100 << 20; // the import cap (PROMPT.md 5.7), model files included

/// A loaded (or freshly created) workspace. All mutations funnel through the interior MUTEX
/// so two saves cannot interleave inside one process; separate processes are fenced by the
/// lock file ([`lock::acquire`]), whose handle lives in the struct and releases on Drop.
pub struct Workspace {
    pub paths: StorePaths,
    pub meta: Mutex<WorkspaceMeta>,
    pub secrets: SecretsStore,
    /// Held while the workspace is open; `Drop` releases the on-disk lock.
    _lock: lock::LockHandle,
}

/// The workspace meta file (`postguy.json`): schema, order, the active environment id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMeta {
    #[serde(default = "schema_value")]
    pub schema: String,
    /// Collection tree order for the sidebar (files bear ids; order file keeps listing).
    #[serde(default)]
    pub collection_order: Vec<Id>,
    #[serde(default)]
    pub environment_order: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_environment_id: Option<Id>,
    #[serde(default, flatten)]
    pub extras: serde_json::Map<String, serde_json::Value>,
}

fn schema_value() -> String {
    "postguy/workspace/1".to_string()
}

impl Default for WorkspaceMeta {
    fn default() -> Self {
        WorkspaceMeta {
            schema: schema_value(),
            collection_order: Vec::new(),
            environment_order: Vec::new(),
            active_environment_id: None,
            extras: serde_json::Map::new(),
        }
    }
}

/// Store-level vocabulary error: id spellings, confinement — before any I/O happens.
#[derive(Debug)]
pub enum WorkspaceError {
    /// An id that is not UUID text was used as a file name (rejected before I/O).
    BadId { id: String },
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkspaceError::BadId { id } => {
                write!(f, "id `{id}` is not UUID text; file names come from generated ids only")
            }
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<WorkspaceError> for postguy_util::Error {
    fn from(err: WorkspaceError) -> postguy_util::Error {
        match err {
            WorkspaceError::BadId { id } => {
                postguy_util::Error::Unusable { what: format!("id `{id}` is not UUID text; file names come from generated ids only") }
            }
        }
    }
}

/// A load outcome keeps the corrupt-quarantine story honest: the caller tells the user.
#[derive(Debug)]
pub enum LoadOutcome<T> {
    Loaded(T),
    /// The file was missing (first run for this entity).
    Fresh(T),
    /// The file was corrupt; it was renamed `.corrupt-<time>` and the default stands.
    CorruptRecovered {
        recovered: T,
        quarantined_to: PathBuf,
    },
}

impl<T> LoadOutcome<T> {
    /// The value either way.
    pub fn into_value(self) -> T {
        match self {
            LoadOutcome::Loaded(value) | LoadOutcome::Fresh(value) | LoadOutcome::CorruptRecovered { recovered: value, .. } => value,
        }
    }

    /// Did loading leave a quarantined file the caller should report?
    pub fn quarantined_path(&self) -> Option<&PathBuf> {
        match self {
            LoadOutcome::CorruptRecovered { quarantined_to, .. } => Some(quarantined_to),
            _ => None,
        }
    }
}

impl Workspace {
    /// Open (or create) a workspace, taking the lock (stale locks are taken over with a
    /// note). Dropping the Workspace releases the lock.
    pub fn open(paths: StorePaths) -> Result<Workspace> {
        for dir in [
            paths.workspace().to_path_buf(),
            paths.collections_dir(),
            paths.environments_dir(),
            paths.protos_dir(),
            paths.files_dir(),
            paths.trash_dir(),
            paths.dot_dir(),
            paths.responses_dir(),
        ] {
            std::fs::create_dir_all(&dir).map_err(|source| postguy_util::Error::Io { operation: format!("creating {}", dir.display()), source })?;
        }

        let lock_handle = lock::acquire(&paths)?;
        let meta = load_or_create_meta(&paths)?;
        let secrets = SecretsStore::open(&paths)?;
        Ok(Workspace { paths, meta: Mutex::new(meta), secrets, _lock: lock_handle })
    }

    /// Save (or replace) one collection file, and point the ORDER file at it atomically.
    pub fn save_collection(&self, collection: &Collection) -> Result<()> {
        let path = self.paths.collection_file(&collection.id.0)?;
        write_model(&path, collection)?;
        let mut meta = self.meta_handle();
        push_unique(&mut meta.collection_order, &collection.id);
        write_meta(&self.paths, &meta)?;
        self.store_meta(meta)
    }

    /// Load one collection by id; a corrupt file quarantines itself once.
    pub fn load_collection(&self, id: &Id) -> Result<LoadOutcome<Collection>> {
        read_model(&self.paths.collection_file(&id.0)?)
    }

    /// Delete one collection to `.trash`; the order file updates with it.
    pub fn delete_collection(&self, id: &Id) -> Result<()> {
        let path = self.paths.collection_file(&id.0)?;
        trash::move_to_trash(&self.paths, &path, "collection")?;
        let mut meta = self.meta_handle();
        meta.collection_order.retain(|other| other != id);
        write_meta(&self.paths, &meta)?;
        self.store_meta(meta)
    }

    /// The collection ids on disk, ORDER first, then unlisted files sorted by id.
    pub fn collection_ids(&self) -> Result<Vec<Id>> {
        ids_of(&self.paths.collections_dir(), &self.meta_lock_order(|m| m.collection_order.clone()))
    }

    /// Save one environment and point the ORDER file at it.
    pub fn save_environment(&self, environment: &Environment) -> Result<()> {
        let path = self.paths.environment_file(&environment.id.0)?;
        write_model(&path, environment)?;
        let mut meta = self.meta_handle();
        push_unique(&mut meta.environment_order, &environment.id);
        write_meta(&self.paths, &meta)?;
        self.store_meta(meta)
    }

    /// Load one environment (same corrupt-file recovery as collections).
    pub fn load_environment(&self, id: &Id) -> Result<LoadOutcome<Environment>> {
        read_model(&self.paths.environment_file(&id.0)?)
    }

    /// The environment ids on disk, ORDER first, then unlisted sorted by id.
    pub fn environment_ids(&self) -> Result<Vec<Id>> {
        ids_of(&self.paths.environments_dir(), &self.meta_handle().environment_order)
    }

    pub fn delete_environment(&self, id: &Id) -> Result<()> {
        let path = self.paths.environment_file(&id.0)?;
        trash::move_to_trash(&self.paths, &path, "environment")?;
        let mut meta = self.meta_handle();
        meta.environment_order.retain(|other| other != id);
        if meta.active_environment_id.as_ref() == Some(id) {
            meta.active_environment_id = None;
        }
        write_meta(&self.paths, &meta)?;
        self.store_meta(meta)
    }

    /// Globals live in ONE file.
    pub fn save_globals(&self, environment: &Environment) -> Result<()> {
        write_model(&self.paths.globals_file(), environment)
    }

    pub fn load_globals(&self) -> Result<LoadOutcome<Environment>> {
        read_model(&self.paths.globals_file())
    }

    /// Set the active environment id (workspace meta; the SESSION remembers too).
    pub fn set_active_environment(&self, id: Option<Id>) -> Result<()> {
        let mut meta = self.meta_handle();
        meta.active_environment_id = id.clone();
        write_meta(&self.paths, &meta)?;
        self.store_meta(meta)
    }

    /// The active environment id from the meta file.
    pub fn active_environment_id(&self) -> Result<Option<Id>> {
        Ok(self.meta_handle().active_environment_id)
    }

    /// History: one funnel (append with rotation, read latest).
    pub fn append_history(&self, entry: &HistoryEntry) -> Result<()> {
        history::append(&self.paths.history_file(), entry)
    }

    pub fn read_history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        history::latest(&self.paths.history_file(), limit)
    }

    /// Move an arbitrary file to trash (used by tools and the engine deletes).
    pub fn delete_to_trash(&self, file: &std::path::Path, kind: &str) -> Result<()> {
        trash::move_to_trash(&self.paths, file, kind)?;
        Ok(())
    }

    /// List the trash and restore or purge entries.
    pub fn trash_entries(&self) -> Result<Vec<trash::Entry>> {
        trash::entries(&self.paths)
    }

    /// Restore one trashed item (" Restore" suffix on name clash).
    pub fn restore_from_trash(&self, entry_id: &str) -> Result<Option<PathBuf>> {
        trash::restore(&self.paths, entry_id)
    }

    // ---- helpers ---------------------------------------------------------------

    /// The meta guard, recovering from poison instead of panicking (AGENTS.md 4): a panic in
    /// another thread leaves possibly-inconsistent state, and the workspace CONTINUES with
    /// what is there; the consistency rules guard the file writes, not the memory lock.
    fn meta_guard(&self) -> std::sync::MutexGuard<'_, WorkspaceMeta> {
        self.meta.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Clone the meta under the mutex (immutably).
    fn meta_handle(&self) -> WorkspaceMeta {
        self.meta_guard().clone()
    }

    /// Store (mutate in place) a fresh meta back into the mutex.
    fn store_meta(&self, meta: WorkspaceMeta) -> Result<()> {
        *self.meta_guard() = meta;
        Ok(())
    }

    /// Read the collection order under the mutex (kept separate for a lock-safe closure).
    fn meta_lock_order(&self, pick: impl FnOnce(&WorkspaceMeta) -> Vec<Id>) -> Vec<Id> {
        pick(&self.meta_guard())
    }
}

/// Append `id` if not present.
fn push_unique(order: &mut Vec<Id>, id: &Id) {
    if !order.iter().any(|other| other == id) {
        order.push(id.clone());
    }
}

fn ids_of(dir: &std::path::Path, order: &[Id]) -> Result<Vec<Id>> {
    let mut listed: Vec<Id> = order.to_vec();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(listed),
        Err(source) => return Err(postguy_util::Error::Io { operation: format!("reading {}", dir.display()), source }),
    };
    let mut unlisted: Vec<Id> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| postguy_util::Error::Io { operation: format!("listing {}", dir.display()), source })?;
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(id) = name.strip_suffix(".json") {
            let id = Id(id.to_string());
            if !listed.contains(&id) {
                unlisted.push(id);
            }
        }
    }
    listed.extend(unlisted);
    Ok(listed)
}

/// Read the meta, creating a default file when missing.
pub fn load_or_create_meta(paths: &StorePaths) -> Result<WorkspaceMeta> {
    match read_model::<WorkspaceMeta>(&paths.workspace_meta_file())? {
        outcome @ (LoadOutcome::Loaded(_) | LoadOutcome::CorruptRecovered { .. }) => Ok(outcome.into_value()),
        LoadOutcome::Fresh(meta) => {
            write_meta(paths, &meta)?;
            Ok(meta)
        }
    }
}

/// Write the meta file atomically with the batch variant's discipline.
pub fn write_meta(paths: &StorePaths, meta: &WorkspaceMeta) -> Result<()> {
    write_model(&paths.workspace_meta_file(), meta)
}

/// Serialize a MODEL (camelCase) to `path` atomically, pretty-printed for git-friendly diffs.
pub fn write_model<T: Serialize>(path: &std::path::Path, model: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(model)
        .map_err(|err| postguy_util::Error::JsonSyntax { what: format!("serializing {}", path.display()), source: err })?;
    atomic_write(path, text.as_bytes())
}

/// Read a model file; missing -> `Fresh(default)`; corrupt -> rename `.corrupt-<time>` and
/// continue with `CorruptRecovered(default)` so the load is RECOVERABLE and the user hears
/// about it (PROMPT.md 4.4: "renamed `.corrupt-<time>`; the app starts without it and says so").
pub fn read_model<T: for<'de> Deserialize<'de> + Default>(path: &std::path::Path) -> Result<LoadOutcome<T>> {
    let bytes = match read_file_capped(path, MAX_FILE_BYTES) {
        Ok(bytes) => bytes,
        Err(postguy_util::Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LoadOutcome::Fresh(T::default()));
        }
        Err(other) => return Err(other),
    };
    match serde_json::from_slice::<T>(&bytes) {
        Ok(model) => Ok(LoadOutcome::Loaded(model)),
        Err(_parse_err) => {
            let quarantined_to = quarantine(path)?;
            Ok(LoadOutcome::CorruptRecovered { recovered: T::default(), quarantined_to })
        }
    }
}

/// Rename a corrupt file aside as `.corrupt-<nanos>`; the old name is now FREE for recovery
/// writes. Renaming never deletes: the bytes stay for manual salvage.
pub fn quarantine(path: &std::path::Path) -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let target = path.to_path_buf().with_file_name(format!("{}.corrupt-{}", file_name_of(path), nanos));
    std::fs::rename(path, &target).map_err(|source| postguy_util::Error::Io { operation: format!("quarantining {}", path.display()), source })?;
    Ok(target)
}

fn file_name_of(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".to_string())
}
