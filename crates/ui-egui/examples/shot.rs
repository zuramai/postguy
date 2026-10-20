//! Headless screenshot of the real shell (`cargo run -p postguy-ui-egui --example shot -- out.png`).
//! This is the M3.10 `shot` example started early so the shell can be reviewed without a display:
//! it renders the SAME `PostguyShell` through egui's wgpu test renderer, not a mock-up.

use egui_kittest::Harness;
use postguy_ui_egui::PostguyShell;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "postguy-shell.png".to_string());
    let mode = std::env::args().nth(2).unwrap_or_else(|| "dark".to_string());
    let mut shell = PostguyShell::new("demo-workspace");
    shell.state.theme_mode = match mode.as_str() {
        "light" => postguy_ui_egui::ThemeMode::Light,
        "system" => postguy_ui_egui::ThemeMode::System,
        _ => postguy_ui_egui::ThemeMode::Dark,
    };
    let mut harness =
        Harness::builder().with_size(egui::vec2(1280.0, 800.0)).wgpu().build_ui_state(|ui, shell: &mut PostguyShell| shell.ui(ui), shell);
    harness.run();
    let image = harness.render().map_err(|err| format!("render: {err}"))?;
    image.save(&out)?;
    println!("shot: wrote {out}");
    Ok(())
}
