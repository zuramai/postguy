//! The workspace handle: open (or create) the folder, load and save everything it owns, and
//! keep the collection/environment ORDER tidy. Saves are atomic (postguy-util::atomic_write),
//! corrupt files are quarantined as `.corrupt-<time>` and REPORTED, never silently dropped.

use std::path::PathBuf;
use std::sync::Mutex;

use postguy_model::{AuthConfig, Collection, Environment, Folder, HistoryEntry, Id, Item, RequestKind, Variable};
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
    /// Entries currently in `history.jsonl`, so appending stays O(1) until the cap needs a
    /// rotation (one startup read instead of one read per send).
    history_count: std::sync::Mutex<usize>,
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
        let history_count = history::latest(&paths.history_file(), usize::MAX).map(|entries| entries.len().min(history::DEFAULT_CAP)).unwrap_or(0);
        Ok(Workspace { paths, meta: Mutex::new(meta), secrets, _lock: lock_handle, history_count: Mutex::new(history_count) })
    }

    /// Save (or replace) one collection file, and point the ORDER file at it atomically.
    /// Secret variables at collection AND folder level move into the secrets store (file
    /// values blanked); everything else is written as-is.
    pub fn save_collection(&self, collection: &Collection) -> Result<()> {
        let mut stored = collection.clone();
        split_collection_variables(&mut stored, &self.secrets)?;
        let path = self.paths.collection_file(&collection.id.0)?;
        write_model(&path, &stored)?;
        let mut meta = self.meta_handle();
        push_unique(&mut meta.collection_order, &collection.id);
        write_meta(&self.paths, &meta)?;
        self.store_meta(meta)
    }

    /// Load one collection by id; a corrupt file quarantines itself once. Secret values
    /// rehydrate from the secrets store per collection/folder owner.
    pub fn load_collection(&self, id: &Id) -> Result<LoadOutcome<Collection>> {
        let outcome = read_model::<Collection>(&self.paths.collection_file(&id.0)?)?;
        Ok(match outcome {
            LoadOutcome::Loaded(mut collection) => {
                rehydrate_collection_variables(&mut collection, &self.secrets)?;
                LoadOutcome::Loaded(collection)
            }
            LoadOutcome::Fresh(mut collection) => {
                rehydrate_collection_variables(&mut collection, &self.secrets)?;
                LoadOutcome::Fresh(collection)
            }
            LoadOutcome::CorruptRecovered { recovered, quarantined_to } => LoadOutcome::CorruptRecovered { recovered, quarantined_to },
        })
    }

    /// Delete one collection to `.trash`; the order file and its secrets go with it.
    pub fn delete_collection(&self, id: &Id) -> Result<()> {
        let path = self.paths.collection_file(&id.0)?;
        trash::move_to_trash(&self.paths, &path, "collection")?;
        let _ = self.secrets.clear_owner(&id.0);
        for folder_id in folder_ids_of_file(&path) {
            let _ = self.secrets.clear_owner(&folder_id);
        }
        let mut meta = self.meta_handle();
        meta.collection_order.retain(|other| other != id);
        write_meta(&self.paths, &meta)?;
        self.store_meta(meta)
    }

    /// The collection ids on disk, ORDER first, then unlisted files sorted by id.
    pub fn collection_ids(&self) -> Result<Vec<Id>> {
        ids_of(&self.paths.collections_dir(), &self.meta_lock_order(|m| m.collection_order.clone()))
    }

    /// Save one environment and point the ORDER file at it. Secret variable values are moved
    /// into the secrets store and BLANKED in the file (PROMPT.md 4.4: secret values never live
    /// in environment files).
    pub fn save_environment(&self, environment: &Environment) -> Result<()> {
        let owner = environment.id.0.clone();
        let mut stored = environment.clone();
        split_variables(&mut stored.variables, &owner, &self.secrets)?;
        let path = self.paths.environment_file(&environment.id.0)?;
        write_model(&path, &stored)?;
        let mut meta = self.meta_handle();
        push_unique(&mut meta.environment_order, &environment.id);
        write_meta(&self.paths, &meta)?;
        self.store_meta(meta)
    }

    /// Load one environment (same corrupt-file recovery as collections), with secret values
    /// rehydrated from the secrets store.
    pub fn load_environment(&self, id: &Id) -> Result<LoadOutcome<Environment>> {
        let outcome = read_model::<Environment>(&self.paths.environment_file(&id.0)?)?;
        self.rehydrate_environment(outcome)
    }

    /// The environment ids on disk, ORDER first, then unlisted sorted by id.
    pub fn environment_ids(&self) -> Result<Vec<Id>> {
        ids_of(&self.paths.environments_dir(), &self.meta_handle().environment_order)
    }

    pub fn delete_environment(&self, id: &Id) -> Result<()> {
        let path = self.paths.environment_file(&id.0)?;
        trash::move_to_trash(&self.paths, &path, "environment")?;
        let _ = self.secrets.clear_owner(&id.0);
        let mut meta = self.meta_handle();
        meta.environment_order.retain(|other| other != id);
        if meta.active_environment_id.as_ref() == Some(id) {
            meta.active_environment_id = None;
        }
        write_meta(&self.paths, &meta)?;
        self.store_meta(meta)
    }

    /// Globals live in ONE file; secrets split like an environment under owner `"globals"`.
    pub fn save_globals(&self, environment: &Environment) -> Result<()> {
        let mut stored = environment.clone();
        split_variables(&mut stored.variables, GLOBALS_OWNER, &self.secrets)?;
        write_model(&self.paths.globals_file(), &stored)
    }

    pub fn load_globals(&self) -> Result<LoadOutcome<Environment>> {
        let outcome = read_model::<Environment>(&self.paths.globals_file())?;
        self.rehydrate_environment(outcome)
    }

    fn rehydrate_environment(&self, outcome: LoadOutcome<Environment>) -> Result<LoadOutcome<Environment>> {
        Ok(match outcome {
            LoadOutcome::Loaded(mut environment) => {
                rehydrate_variables(&mut environment.variables, &environment.id.0, &self.secrets)?;
                LoadOutcome::Loaded(environment)
            }
            LoadOutcome::Fresh(mut environment) => {
                rehydrate_variables(&mut environment.variables, &environment.id.0, &self.secrets)?;
                LoadOutcome::Fresh(environment)
            }
            LoadOutcome::CorruptRecovered { recovered: mut environment, quarantined_to } => {
                rehydrate_variables(&mut environment.variables, GLOBALS_OWNER, &self.secrets)?;
                LoadOutcome::CorruptRecovered { recovered: environment, quarantined_to }
            }
        })
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
        let path = self.paths.history_file();
        let mut count = self.history_count.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if *count < history::DEFAULT_CAP {
            history::append_line(&path, entry)?;
            *count = count.saturating_add(1);
            Ok(())
        } else {
            // At the cap: rewrite keeping the newest entries (one rotation per new send).
            history::append(&path, entry)?;
            *count = history::DEFAULT_CAP;
            Ok(())
        }
    }

    pub fn read_history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        history::latest(&self.paths.history_file(), limit)
    }

    /// Remove one history entry by id.
    pub fn delete_history(&self, id: &str) -> Result<bool> {
        let removed = history::remove(&self.paths.history_file(), id)?;
        if removed && let Ok(mut count) = self.history_count.lock() {
            *count = count.saturating_sub(1);
        }
        Ok(removed)
    }

    /// Empty the history file atomically (the History panel's Clear).
    pub fn clear_history(&self) -> Result<()> {
        history::clear(&self.paths.history_file())?;
        if let Ok(mut count) = self.history_count.lock() {
            *count = 0;
        }
        Ok(())
    }

    /// Load `.postguy/cookies.json`: a missing or corrupt file yields an empty object; a
    /// corrupt one is quarantined and reported (never blocks startup).
    pub fn load_cookies(&self) -> Result<(serde_json::Value, Option<PathBuf>)> {
        let path = self.paths.cookies_file();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok((serde_json::Value::Object(serde_json::Map::new()), None));
            }
            Err(source) => return Err(postguy_util::Error::Io { operation: format!("reading {}", path.display()), source }),
        };
        match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(value @ serde_json::Value::Object(_)) => Ok((value, None)),
            _ => {
                let quarantined = quarantine(&path)?;
                Ok((serde_json::Value::Object(serde_json::Map::new()), Some(quarantined)))
            }
        }
    }

    /// Save `.postguy/cookies.json` atomically (the jar owns its own shape).
    pub fn save_cookies(&self, value: &serde_json::Value) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(value)
            .map_err(|err| postguy_util::Error::JsonSyntax { what: "serializing cookies".to_string(), source: err })?;
        atomic_write(&self.paths.cookies_file(), &bytes)
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

/// The secrets owner for the globals file.
pub const GLOBALS_OWNER: &str = "globals";

/// Move secret variable values into the secrets store, blanking them in the model copy that is
/// about to be written (PROMPT.md 4.4). A cleared secret row removes the stored value.
fn split_variables(variables: &mut [Variable], owner: &str, secrets: &SecretsStore) -> Result<()> {
    for variable in variables.iter_mut().filter(|v| v.secret) {
        if variable.value.is_empty() {
            secrets.set(owner, &variable.key, None)?;
        } else {
            secrets.set(owner, &variable.key, Some(&variable.value))?;
            variable.value.clear();
        }
    }
    Ok(())
}

/// Put stored secret values back for the UI/engine. A blank file value stays blank when the
/// secrets store has nothing (a genuinely empty secret).
fn rehydrate_variables(variables: &mut [Variable], owner: &str, secrets: &SecretsStore) -> Result<()> {
    let stored = secrets.values_for(owner)?;
    for variable in variables.iter_mut().filter(|v| v.secret) {
        if let Some(value) = stored.get(&variable.key) {
            variable.value.clone_from(value);
        }
    }
    Ok(())
}

/// Collection- and folder-level secret splitting, walking the item tree iteratively.
fn split_collection_variables(collection: &mut Collection, secrets: &SecretsStore) -> Result<()> {
    let owner = collection.id.0.clone();
    split_variables(&mut collection.variables, &owner, secrets)?;
    split_auth_tokens(&mut collection.auth, &owner, secrets)?;
    let mut stack: Vec<&mut Item> = collection.items.iter_mut().collect();
    while let Some(item) = stack.pop() {
        match item {
            Item::Folder(folder) => {
                let owner = folder.id.0.clone();
                split_variables(&mut folder.variables, &owner, secrets)?;
                split_auth_tokens(&mut folder.auth, &owner, secrets)?;
                stack.extend(folder.items.iter_mut());
            }
            Item::Request(request) => {
                if let RequestKind::Http(http) = &mut request.kind {
                    split_oauth2(&mut http.auth, &request.id.0, secrets)?;
                }
            }
        }
    }
    Ok(())
}

/// Rehydrate both collection and folder secret variables.
fn rehydrate_collection_variables(collection: &mut Collection, secrets: &SecretsStore) -> Result<()> {
    let owner = collection.id.0.clone();
    rehydrate_variables(&mut collection.variables, &owner, secrets)?;
    rehydrate_auth_tokens(&mut collection.auth, &owner, secrets)?;
    let mut stack: Vec<&mut Item> = collection.items.iter_mut().collect();
    while let Some(item) = stack.pop() {
        match item {
            Item::Folder(folder) => {
                let owner = folder.id.0.clone();
                rehydrate_variables(&mut folder.variables, &owner, secrets)?;
                rehydrate_auth_tokens(&mut folder.auth, &owner, secrets)?;
                stack.extend(folder.items.iter_mut());
            }
            Item::Request(request) => {
                if let RequestKind::Http(http) = &mut request.kind {
                    rehydrate_oauth2(&mut http.auth, &request.id.0, secrets)?;
                }
            }
        }
    }
    Ok(())
}

/// OAuth 2.0 access/refresh tokens are credentials: the collection file keeps only the rest of
/// the auth block, while the values live in the owner-only secrets store.
fn split_auth_tokens(auth: &mut Option<AuthConfig>, owner: &str, secrets: &SecretsStore) -> Result<()> {
    if let Some(auth) = auth {
        split_oauth2(auth, owner, secrets)?;
    }
    Ok(())
}

fn split_oauth2(auth: &mut AuthConfig, owner: &str, secrets: &SecretsStore) -> Result<()> {
    let AuthConfig::OAuth2(oauth2) = auth else { return Ok(()) };
    for (field, value) in [("oauth2.access", &mut oauth2.access_token), ("oauth2.refresh", &mut oauth2.refresh_token)] {
        if value.is_empty() {
            secrets.set(owner, field, None)?;
        } else {
            secrets.set(owner, field, Some(value))?;
            value.clear();
        }
    }
    Ok(())
}

fn rehydrate_auth_tokens(auth: &mut Option<AuthConfig>, owner: &str, secrets: &SecretsStore) -> Result<()> {
    if let Some(auth) = auth {
        rehydrate_oauth2(auth, owner, secrets)?;
    }
    Ok(())
}

fn rehydrate_oauth2(auth: &mut AuthConfig, owner: &str, secrets: &SecretsStore) -> Result<()> {
    let AuthConfig::OAuth2(oauth2) = auth else { return Ok(()) };
    let stored = secrets.values_for(owner)?;
    if oauth2.access_token.is_empty()
        && let Some(value) = stored.get("oauth2.access")
    {
        oauth2.access_token.clone_from(value);
    }
    if oauth2.refresh_token.is_empty()
        && let Some(value) = stored.get("oauth2.refresh")
    {
        oauth2.refresh_token.clone_from(value);
    }
    Ok(())
}

/// Folder ids from a collection FILE on disk (for clearing secrets on delete); a corrupt or
/// missing file yields none.
fn folder_ids_of_file(path: &std::path::Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    let Ok(collection) = serde_json::from_slice::<Collection>(&bytes) else { return Vec::new() };
    let mut ids = Vec::new();
    let mut stack: Vec<&Folder> = collection.items.iter().filter_map(|item| item.folders()).collect();
    while let Some(folder) = stack.pop() {
        ids.push(folder.id.0.clone());
        stack.extend(folder.items.iter().filter_map(|item| item.folders()));
    }
    ids
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_append_lines_reach_the_cap_and_rotate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = Workspace::open(StorePaths::new(dir.path(), dir.path().join("config"))).expect("open");
        for index in 0..(history::DEFAULT_CAP + 3) {
            let entry = HistoryEntry { id: postguy_model::Id(format!("h{index}")), ..HistoryEntry::default() };
            workspace.append_history(&entry).expect("append");
        }
        let entries = workspace.read_history(usize::MAX).expect("read");
        assert_eq!(entries.len(), history::DEFAULT_CAP);
        assert_eq!(entries.first().map(|entry| entry.id.0.as_str()), Some(format!("h{}", 3).as_str()));
        assert!(entries.iter().all(|entry| entry.status.is_none()));
        workspace.clear_history().expect("clear");
        assert!(workspace.read_history(usize::MAX).expect("read").is_empty());
        workspace.append_history(&HistoryEntry { id: postguy_model::Id("after".into()), ..HistoryEntry::default() }).expect("append after clear");
        assert_eq!(workspace.read_history(usize::MAX).expect("read").len(), 1);
    }

    #[test]
    fn cookies_round_trip_and_a_corrupt_file_is_quarantined() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = Workspace::open(StorePaths::new(dir.path(), dir.path().join("config"))).expect("open");
        let (empty, quarantine) = workspace.load_cookies().expect("load missing");
        assert_eq!(empty, serde_json::json!({}));
        assert!(quarantine.is_none(), "a missing file is not a repair");

        let value = serde_json::json!({"cookies": [{"name": "a"}], "domainAllowlist": ["example.com"]});
        workspace.save_cookies(&value).expect("save");
        let (loaded, quarantine) = workspace.load_cookies().expect("load");
        assert_eq!(loaded, value);
        assert!(quarantine.is_none());

        std::fs::write(workspace.paths.cookies_file(), b"{not json").expect("corrupt it");
        let (repaired, quarantine) = workspace.load_cookies().expect("load corrupt");
        assert_eq!(repaired, serde_json::json!({}), "an empty jar, never a crash");
        let quarantined = quarantine.expect("quarantined path");
        assert!(quarantined.exists(), "the bad file is kept for the user");
        assert!(!workspace.paths.cookies_file().exists(), "the bad file is moved aside");
    }
}
