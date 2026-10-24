//! OS services: config/data dirs, file dialogs, reveal in file manager, open URL, clipboard,
//! system proxy, system font fallback.
//!
//! Status: the import file/folder chooser is implemented with `rfd`'s async dialog (M4.1), so
//! the UI polls the returned future from its own thread instead of blocking. The remaining
//! services arrive with M5/M10.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use rfd::AsyncFileDialog;

/// Pick one folder or multiple files for import. The future resolves to an empty list when
/// the user cancels. Native dialogs are parentless; the caller keeps the app responsive by
/// polling this from the UI thread (a wake requests a repaint).
/// Open a URL in the user's default browser. This is always an explicit user action (OAuth
/// authorization, docs links); the command is passed as an argument, never through a shell.
pub fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    command.arg(url).spawn().map(|_| ()).map_err(|err| format!("opening {url}: {err}"))
}

pub fn save_export(default_name: String, extension: &'static str) -> Pin<Box<dyn Future<Output = Option<PathBuf>>>> {
    Box::pin(async move {
        AsyncFileDialog::new()
            .set_title("Export")
            .set_file_name(default_name)
            .add_filter(extension.to_ascii_uppercase(), &[extension])
            .save_file()
            .await
            .map(|handle| handle.path().to_path_buf())
    })
}

/// Pick one CSV or JSON data file for the collection runner.
pub fn pick_data_file() -> Pin<Box<dyn Future<Output = Option<PathBuf>>>> {
    Box::pin(async move {
        AsyncFileDialog::new()
            .set_title("Choose data file")
            .add_filter("CSV", &["csv"])
            .add_filter("JSON", &["json"])
            .pick_file()
            .await
            .map(|handle| handle.path().to_path_buf())
    })
}

pub fn pick_import_paths(folder: bool) -> Pin<Box<dyn Future<Output = Vec<PathBuf>>>> {
    Box::pin(async move {
        let dialog = AsyncFileDialog::new().set_title(if folder { "Import folder" } else { "Import files" });
        if folder {
            dialog.pick_folder().await.map(|handle| vec![handle.path().to_path_buf()]).unwrap_or_default()
        } else {
            dialog.pick_files().await.map(|handles| handles.iter().map(|handle| handle.path().to_path_buf()).collect()).unwrap_or_default()
        }
    })
}
