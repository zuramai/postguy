//! The engine's single tokio runtime. Worker threads are tokio's own; blocking
//! frontends (`postguy-cli`) call [`RuntimeHandle::block_on`], the UI uses `spawn`.

use std::future::Future;

/// Owns the runtime for the whole session.
pub struct RuntimeHandle {
    runtime: tokio::runtime::Runtime,
}

impl RuntimeHandle {
    /// Two worker threads and every driver (net, time) enabled.
    pub fn new() -> std::io::Result<RuntimeHandle> {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
        Ok(RuntimeHandle { runtime })
    }

    /// Run a future to completion on the caller's thread, driving the shared runtime. Only
    /// blocking frontends call this; the UI must use `spawn` and poll its channel.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    /// Run a future on the runtime and get a handle for its result.
    pub fn spawn<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime.spawn(future)
    }

    /// The runtime itself, for libraries that need an executor handle.
    pub fn handle(&self) -> tokio::runtime::Handle {
        self.runtime.handle().clone()
    }
}

impl std::fmt::Debug for RuntimeHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeHandle").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn futures_run_and_spawned_tasks_complete() {
        let runtime = RuntimeHandle::new().expect("runtime");
        assert_eq!(runtime.block_on(async { 2 + 2 }), 4);
        let task = runtime.spawn(async { "done" });
        assert_eq!(runtime.block_on(task).expect("join"), "done");
    }
}
