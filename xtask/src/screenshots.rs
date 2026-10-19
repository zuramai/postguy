//! `cargo xtask screenshots`: regenerate the documentation screenshots from Postguy's own UI
//! with demo data (AGENTS.md 1.2: screenshots are contributor-original; everything visible in
//! them must be allowed). Screenshot sets are declared below once the shell exists (M3.10);
//! the tool builds the `shot` example of `ui-egui`, runs it headlessly and copies the PNGs
//! into `docs/images/`, with `POSTGUY_SYSTEM_FONTS=0` so the runtime font fallback never
//! appears in a published image (AGENTS.md 1.4).

use anyhow::Result;

/// (example name, PNG files it must produce relative to `docs/images/`). Empty until M3.10.
const SETS: &[(&str, &[&str])] = &[];

pub fn run(_: &[String]) -> Result<()> {
    if SETS.is_empty() {
        println!("screenshots: none configured yet (the UI shell arrives in M3.10, PLAN.md); nothing to do");
        return Ok(());
    }
    unreachable!("screenshot sets exist only from M3.10 on; extend this module when the first set is added")
}
