//! Per-collection asset cache for the UI: the Assets tab rows and the index the body editors
//! use to colour `{{% file('…') %}}` shortcodes. Every filesystem touch (listing, import,
//! rename, delete) runs on a `postguy-assets` thread and reports through a channel that the
//! frame loop polls; nothing here blocks the UI thread.

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use postguy_engine::{AssetInfo, Engine};
use postguy_model::Id;
use postguy_ui_egui::AssetIndex;
use postguy_ui_egui::collection::AssetRow;

type Listing = Result<Vec<AssetInfo>, String>;

/// What one collection's assets look like right now.
#[derive(Default)]
struct AssetCache {
    rows: Vec<AssetRow>,
    index: AssetIndex,
    loading: bool,
    receiver: Option<Receiver<Listing>>,
}

/// A mutation running on a worker thread; `Ok` carries the status line.
struct AssetJob {
    collection: String,
    receiver: Receiver<Result<String, String>>,
}

type Picker = Pin<Box<dyn Future<Output = Vec<PathBuf>>>>;

#[derive(Default)]
pub struct AssetState {
    caches: HashMap<String, AssetCache>,
    jobs: Vec<AssetJob>,
    picker: Option<(String, Picker)>,
}

impl AssetState {
    /// The Assets tab rows; `None` while the first listing is still loading.
    pub fn rows(&self, collection: &str) -> Option<&[AssetRow]> {
        let cache = self.caches.get(collection)?;
        cache.index.loaded.then_some(cache.rows.as_slice())
    }

    /// The path index for shortcode colouring, once any listing exists (even a stale one).
    pub fn index(&self, collection: &str) -> Option<&AssetIndex> {
        self.caches.get(collection).map(|cache| &cache.index)
    }

    /// True while something needs the frame loop to keep polling.
    pub fn busy(&self) -> bool {
        self.picker.is_some() || !self.jobs.is_empty() || self.caches.values().any(|cache| cache.loading)
    }

    /// Start the first listing of `collection` when there is no cache entry yet.
    pub fn ensure(&mut self, engine: &Engine, collection: &str) {
        if collection.is_empty() || self.caches.contains_key(collection) {
            return;
        }
        self.caches.insert(collection.to_string(), AssetCache::default());
        self.reload(engine, collection);
    }

    /// List the collection again on a worker thread.
    pub fn reload(&mut self, engine: &Engine, collection: &str) {
        let cache = self.caches.entry(collection.to_string()).or_default();
        let (sender, receiver) = channel();
        cache.loading = true;
        cache.receiver = Some(receiver);
        let engine = engine.clone();
        let id = Id(collection.to_string());
        let spawned = std::thread::Builder::new().name("postguy-assets".into()).spawn(move || {
            let _ = sender.send(engine.assets(&id).map_err(|err| err.to_string()));
        });
        if spawned.is_err() {
            cache.loading = false;
            cache.receiver = None;
        }
    }

    /// Open the native picker for files or a folder to import into `collection`.
    pub fn pick(&mut self, collection: &str, folder: bool) {
        if self.picker.is_none() {
            self.picker = Some((collection.to_string(), postguy_platform::pick_import_paths(folder)));
        }
    }

    pub fn delete(&mut self, engine: &Engine, collection: &str, path: String) {
        self.spawn_job(engine, collection, move |engine, id| {
            engine.delete_asset(id, &path).map(|()| format!("Deleted asset {path}")).map_err(|err| err.to_string())
        });
    }

    pub fn rename(&mut self, engine: &Engine, collection: &str, from: String, to: String) {
        self.spawn_job(engine, collection, move |engine, id| {
            engine.rename_asset(id, &from, &to).map(|info| format!("Renamed to {}", info.path)).map_err(|err| err.to_string())
        });
    }

    fn import(&mut self, engine: &Engine, collection: &str, paths: Vec<PathBuf>) {
        self.spawn_job(engine, collection, move |engine, id| {
            let outcome = engine.import_assets(id, &paths, "", false).map_err(|err| err.to_string())?;
            Ok(format!("Imported {} assets ({} skipped)", outcome.imported.len(), outcome.skipped.len()))
        });
    }

    fn spawn_job(&mut self, engine: &Engine, collection: &str, work: impl FnOnce(&Engine, &Id) -> Result<String, String> + Send + 'static) {
        let (sender, receiver) = channel();
        let engine = engine.clone();
        let id = Id(collection.to_string());
        let spawned = std::thread::Builder::new().name("postguy-assets".into()).spawn(move || {
            let _ = sender.send(work(&engine, &id));
        });
        if spawned.is_ok() {
            self.jobs.push(AssetJob { collection: collection.to_string(), receiver });
        }
    }

    /// Advance the picker, the listings and the mutations. Returns a status line when one of
    /// them finished.
    pub fn poll(&mut self, ctx: &egui::Context, engine: Option<&Engine>) -> Option<String> {
        let mut status = None;
        if let Some((collection, mut picker)) = self.picker.take() {
            let waker = std::task::Waker::from(std::sync::Arc::new(crate::RepaintWake(ctx.clone())));
            let mut task = std::task::Context::from_waker(&waker);
            match picker.as_mut().poll(&mut task) {
                std::task::Poll::Pending => self.picker = Some((collection, picker)),
                std::task::Poll::Ready(paths) => {
                    if let (false, Some(engine)) = (paths.is_empty(), engine) {
                        self.import(engine, &collection, paths);
                    }
                }
            }
        }
        for cache in self.caches.values_mut() {
            let Some(receiver) = cache.receiver.take() else { continue };
            match receiver.try_recv() {
                Ok(Ok(infos)) => {
                    cache.rows = infos.into_iter().map(|info| AssetRow { path: info.path, size: info.size, modified_ms: info.modified_ms }).collect();
                    cache.index = AssetIndex { paths: cache.rows.iter().map(|row| row.path.clone()).collect(), loaded: true };
                    cache.loading = false;
                }
                Ok(Err(err)) => {
                    cache.loading = false;
                    status = Some(format!("Assets: {err}"));
                }
                Err(TryRecvError::Empty) => cache.receiver = Some(receiver),
                Err(TryRecvError::Disconnected) => {
                    cache.loading = false;
                    status = Some("Assets: the listing worker stopped".into());
                }
            }
        }
        let mut refresh = Vec::new();
        let mut index = 0;
        while index < self.jobs.len() {
            let result = self.jobs.get(index).map(|job| job.receiver.try_recv());
            match result {
                Some(Err(TryRecvError::Empty)) => index += 1,
                Some(done) => {
                    let job = self.jobs.remove(index);
                    status = Some(match done {
                        Ok(Ok(message)) => message,
                        Ok(Err(err)) => format!("Assets: {err}"),
                        _ => "Assets: the worker stopped".to_string(),
                    });
                    refresh.push(job.collection);
                }
                None => break,
            }
        }
        if let Some(engine) = engine {
            for collection in refresh {
                self.reload(engine, &collection);
            }
        }
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        status
    }
}
