//! Postguy desktop app (eframe + glow). The shell is `postguy-ui-egui`; this binary owns the
//! window, the OS theme signal and (from M3.2) the engine session.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use postguy_ui_egui::PostguyShell;

struct App {
    shell: PostguyShell,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shell.ui(ui);
    }
}

fn main() -> eframe::Result<()> {
    // MIN 900x560, default 1280x800 (PROMPT.md 6.2); window position is remembered by eframe
    // once the session store lands in M3.3.
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]).with_min_inner_size([900.0, 560.0]).with_title("Postguy"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native("Postguy", options, Box::new(|_cc| Ok(Box::new(App { shell: PostguyShell::new("No workspace") }))))
}
