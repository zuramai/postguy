//! The app's side of updates: release checks on a worker thread (the user's "Check for
//! updates", plus the daily automatic one Settings can turn off), the download and verification
//! on another, and the hand-off to `main`, which installs the update after the window closed and
//! the session was saved.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use postguy_engine::update::{RELEASES_PAGE_URL, Release, UpdateCheck};
use postguy_engine::{Cancel, Engine};
use postguy_platform::update::{Install, PreparedUpdate};
use postguy_ui_egui::update::{UpdatePhase, UpdateState};

/// How long an automatic check result stays fresh.
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// The verified update `main` installs once the event loop has returned.
pub type PendingRestart = Arc<Mutex<Option<PreparedUpdate>>>;

type CheckResult = Result<(UpdateCheck, Install), String>;

struct CheckJob {
    /// The user asked (the result reports failures in the dialog).
    manual: bool,
    receiver: Receiver<CheckResult>,
}

struct DownloadJob {
    cancel: Cancel,
    receiver: Receiver<Result<PreparedUpdate, String>>,
}

#[derive(Default)]
pub struct Updater {
    /// Automatic checks may run in this process. Only `main` sets it, so tests and the headless
    /// screenshot example never reach the network.
    pub automatic: bool,
    /// When the last check finished (Unix ms); the session file keeps it across launches.
    pub checked_at_ms: i64,
    check: Option<CheckJob>,
    download: Option<DownloadJob>,
    /// The newer release on offer and how this copy was installed.
    offer: Option<(Release, Install)>,
    ready: Option<PreparedUpdate>,
    pub restart: PendingRestart,
}

impl Updater {
    pub fn busy(&self) -> bool {
        self.check.is_some() || self.download.is_some()
    }

    /// An automatic check is due: allowed, nothing running or staged, and a day since the last.
    pub fn automatic_check_due(&self, enabled: bool, now_ms: i64) -> bool {
        self.automatic && enabled && !self.busy() && self.ready.is_none() && now_ms.saturating_sub(self.checked_at_ms) >= DAY_MS
    }

    /// Start a check. A manual check over a running automatic one adopts it instead.
    pub fn start_check(&mut self, engine: &Engine, manual: bool, state: &mut UpdateState) {
        if manual {
            state.phase = UpdatePhase::Checking;
        }
        if let Some(job) = &mut self.check {
            job.manual |= manual;
            return;
        }
        let (sender, receiver) = channel();
        let engine = engine.clone();
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let result = engine.check_for_update().map_err(|err| err.to_string()).map(|check| (check, Install::detect()));
            let _ = sender.send(result);
        });
        match spawned {
            Ok(_) => self.check = Some(CheckJob { manual, receiver }),
            Err(err) => state.phase = UpdatePhase::Failed { message: format!("could not start the update check: {err}"), page_url: None },
        }
    }

    /// Download, verify and unpack the offered release on a worker thread.
    pub fn start_download(&mut self, engine: &Engine, state: &mut UpdateState) {
        let Some((release, install)) = self.offer.clone() else { return };
        if self.download.is_some() {
            return;
        }
        let Some(asset) = install.asset_name(&release.version) else { return };
        let (sender, receiver) = channel();
        let cancel = Cancel::new();
        let worker_cancel = cancel.clone();
        let engine = engine.clone();
        let version = release.version.clone();
        let spawned = std::thread::Builder::new().name("update-download".into()).spawn(move || {
            let downloads = postguy_platform::update::staging_dir().join("downloads");
            let result = engine
                .download_update_asset(&release, &asset, &downloads, &worker_cancel)
                .map_err(|err| err.to_string())
                .and_then(|downloaded| install.prepare(&downloaded.path));
            let _ = sender.send(result);
        });
        match spawned {
            Ok(_) => {
                self.download = Some(DownloadJob { cancel, receiver });
                state.phase = UpdatePhase::Downloading { version };
            }
            Err(err) => {
                state.phase =
                    UpdatePhase::Failed { message: format!("could not start the download: {err}"), page_url: Some(RELEASES_PAGE_URL.into()) }
            }
        }
    }

    pub fn cancel_download(&mut self) {
        if let Some(job) = &self.download {
            job.cancel.cancel();
        }
    }

    /// Hand the staged update to `main`; false when nothing is staged.
    pub fn restart(&mut self) -> bool {
        let Some(prepared) = self.ready.take() else { return false };
        let mut pending = self.restart.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *pending = Some(prepared);
        true
    }

    /// Collect finished jobs into `state`. Returns whether the phase changed.
    pub fn poll(&mut self, state: &mut UpdateState, now_ms: i64) -> bool {
        let mut changed = false;
        if let Some(job) = self.check.take() {
            let result = match job.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err("the update check stopped unexpectedly".into())),
            };
            match result {
                None => self.check = Some(job),
                Some(result) => {
                    changed = true;
                    // Failures count too, so an offline machine is not asked again every frame.
                    self.checked_at_ms = now_ms;
                    self.apply_check(result, job.manual, state);
                }
            }
        }
        if let Some(job) = self.download.take() {
            let result = match job.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err("the download stopped unexpectedly".into())),
            };
            match result {
                None => self.download = Some(job),
                Some(Ok(prepared)) => {
                    changed = true;
                    let version = self.offer.as_ref().map(|(release, _)| release.version.clone()).unwrap_or_default();
                    state.phase = UpdatePhase::Ready { version, verification: prepared.signature.describe() };
                    self.ready = Some(prepared);
                }
                Some(Err(message)) => {
                    changed = true;
                    state.phase = if job.cancel.is_cancelled() {
                        self.offered_phase().unwrap_or_default()
                    } else {
                        let page_url = self.offer.as_ref().map(|(release, _)| release.page_url.clone()).filter(|url| !url.is_empty());
                        UpdatePhase::Failed { message, page_url: page_url.or_else(|| Some(RELEASES_PAGE_URL.into())) }
                    };
                }
            }
        }
        changed
    }

    fn apply_check(&mut self, result: CheckResult, manual: bool, state: &mut UpdateState) {
        match result {
            Ok((check, install)) if check.available => {
                self.offer = Some((check.latest, install));
                if self.download.is_none() && self.ready.is_none() {
                    state.phase = self.offered_phase().unwrap_or_default();
                }
            }
            Ok(_) => {
                self.offer = None;
                state.phase = UpdatePhase::UpToDate;
            }
            // A failed automatic check keeps an offer that is already showing.
            Err(_) if !manual && self.offer.is_some() => {}
            Err(message) => state.phase = UpdatePhase::Failed { message, page_url: Some(RELEASES_PAGE_URL.into()) },
        }
    }

    fn offered_phase(&self) -> Option<UpdatePhase> {
        let (release, install) = self.offer.as_ref()?;
        Some(UpdatePhase::Available {
            version: release.version.clone(),
            notes: release.notes.clone(),
            page_url: if release.page_url.is_empty() { RELEASES_PAGE_URL.into() } else { release.page_url.clone() },
            manual: manual_reason(install, release),
        })
    }
}

/// Why the offered release cannot be installed from inside the app (`None`: it can).
fn manual_reason(install: &Install, release: &Release) -> Option<String> {
    if let Some(reason) = install.manual_reason() {
        return Some(reason);
    }
    let name = install.asset_name(&release.version)?;
    release.asset(&name).is_none().then(|| format!("Release {} has no {name} package. Download the update from the release page.", release.tag))
}

/// Where a failed install leaves its message for the relaunched app.
pub fn failure_note() -> PathBuf {
    postguy_platform::update::staging_root().join("failed.txt")
}

/// Install the staged update and start the new version, or, when installing fails, start this
/// version again with the reason waiting in [`failure_note`].
pub fn install_and_relaunch(prepared: PreparedUpdate) {
    let args = relaunch_args(std::env::args_os().skip(1).collect(), std::env::current_dir().ok());
    if let Err(message) = prepared.apply_and_relaunch(args.clone()) {
        eprintln!("update failed: {message}");
        let note = failure_note();
        if let Some(dir) = note.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&note, message.as_bytes());
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).args(args).spawn();
        }
    }
}

/// This process's arguments with relative `--workspace`/`--control` paths made absolute: the
/// relaunched app may start in another folder (macOS `open` starts it in `/`).
fn relaunch_args(args: Vec<std::ffi::OsString>, cwd: Option<PathBuf>) -> Vec<std::ffi::OsString> {
    let mut out = Vec::with_capacity(args.len());
    let mut path_next = false;
    for arg in args {
        let is_flag = arg == "--workspace" || arg == "--control";
        match &cwd {
            Some(cwd) if path_next && PathBuf::from(&arg).is_relative() => out.push(cwd.join(&arg).into_os_string()),
            _ => out.push(arg),
        }
        path_next = is_flag;
    }
    out
}

/// The message a failed install left behind (first 4 KiB), removed once read.
pub fn take_failure() -> Option<String> {
    use std::io::Read as _;
    let note = failure_note();
    let mut message = String::new();
    let read = std::fs::File::open(&note).and_then(|file| file.take(4096).read_to_string(&mut message));
    let _ = std::fs::remove_file(&note);
    let message = read.ok().map(|_| message.trim().to_string()).filter(|message| !message.is_empty())?;
    Some(format!("The update was not installed: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaunch_arguments_keep_the_same_workspace_from_any_folder() {
        let cwd = std::env::temp_dir().join("launch-folder");
        let control = std::env::temp_dir().join("control.json");
        let args = vec!["--workspace".into(), "work".into(), "--control".into(), control.clone().into_os_string(), "--other".into(), "keep".into()];
        let out = relaunch_args(args, Some(cwd.clone()));
        assert_eq!(out.get(1).map(PathBuf::from), Some(cwd.join("work")));
        assert_eq!(out.get(3).map(PathBuf::from), Some(control), "absolute paths stay as they are");
        assert_eq!(out.get(5).map(PathBuf::from), Some(PathBuf::from("keep")), "only path arguments change");
    }
}
