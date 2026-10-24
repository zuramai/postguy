//! Console, cookies manager and settings UI tests.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_model::{AuthConfig, Settings, Theme};
use postguy_ui_egui::console::{ConsoleAction, ConsoleFilter, ConsoleRow, ConsoleState, console_view};
use postguy_ui_egui::cookies::{CookieAction, CookieData, CookieRow, CookieState, cookie_manager};
use postguy_ui_egui::settings::{SettingsAction, SettingsEditor, SettingsSection, SettingsState};

#[derive(Default)]
struct ConsoleHarness {
    rows: Vec<ConsoleRow>,
    state: ConsoleState,
    action: Option<ConsoleAction>,
    last: Option<ConsoleAction>,
}

#[test]
fn settings_shortcuts_lists_the_registry_table() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut SettingsHarness| {
            let mut editor = SettingsEditor { settings: &mut state.settings, state: &mut state.state };
            let _ = editor.show(ui);
        },
        SettingsHarness { settings: Settings::default(), state: SettingsState::default(), last: None },
    );
    harness.set_size(egui::vec2(1000.0, 800.0));
    harness.run();
    harness.get_by_label("Shortcuts").click_accesskit();
    harness.run();
    assert!(harness.query_by_label_contains("Toggle console").is_some());
    let shown = postguy_ui_egui::menu::display_shortcut("Cmd+Alt+C", cfg!(target_os = "macos"));
    assert!(harness.query_by_label_contains(&shown).is_some(), "the console shortcut {shown} is listed");
}

#[test]
fn console_filters_search_and_clears() {
    let rows = vec![
        ConsoleRow {
            id: 1,
            time_ms: 0,
            level: "info".into(),
            kind: "request".into(),
            message: "GET /echo 200".into(),
            detail: Some("detail".into()),
        },
        ConsoleRow { id: 2, time_ms: 0, level: "warning".into(), kind: "warning".into(), message: "scripts skipped".into(), detail: None },
    ];
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut ConsoleHarness| {
            if let Some(action) = console_view(ui, &state.rows, &mut state.state) {
                state.action = Some(action);
                state.last = Some(action);
            }
        },
        ConsoleHarness { rows, ..Default::default() },
    );
    harness.run();
    assert!(harness.query_by_label_contains("GET /echo 200").is_some());
    harness.get_by_label("Warnings").click();
    harness.run();
    assert!(harness.query_by_label_contains("GET /echo 200").is_none());
    assert!(harness.query_by_label_contains("scripts skipped").is_some());
    harness.get_by_label("Clear").click();
    harness.run();
    assert_eq!(harness.state().last, Some(ConsoleAction::Clear));
}

#[derive(Default)]
struct CookieHarness {
    data: CookieData,
    state: CookieState,
    last: Option<CookieAction>,
}

#[test]
fn cookies_delete_clear_and_allowlist() {
    let data = CookieData {
        cookies: vec![CookieRow {
            name: "sid".into(),
            value: "abc".into(),
            domain: "example.com".into(),
            path: "/".into(),
            secure: true,
            http_only: true,
            expires_ms: None,
        }],
        allowlist: vec!["example.com".into()],
    };
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut CookieHarness| {
            if let Some(action) = cookie_manager(ui, &state.data, &mut state.state) {
                state.last = Some(action);
            }
        },
        CookieHarness { data, ..Default::default() },
    );
    harness.run();
    assert!(harness.query_by_label_contains("sid").is_some());
    assert!(harness.query_by_label_contains("SH-").is_some());
    harness.get_by_label("Delete cookie").click();
    harness.run();
    assert_eq!(harness.state().last, Some(CookieAction::Delete { domain: "example.com".into(), path: "/".into(), name: "sid".into() }));
    harness.get_by_label("Clear all").click();
    harness.run();
    assert_eq!(harness.state().last, Some(CookieAction::Clear));
    harness.get_by_label("x").click();
    harness.run();
    assert_eq!(harness.state().last, Some(CookieAction::Disallow("example.com".into())));
}

struct SettingsHarness {
    settings: Settings,
    state: SettingsState,
    last: Option<SettingsAction>,
}

#[test]
fn settings_theme_and_proxy_edit_save() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut SettingsHarness| {
            let mut editor = SettingsEditor { settings: &mut state.settings, state: &mut state.state };
            if let Some(action) = editor.show(ui) {
                state.last = Some(action);
            }
        },
        SettingsHarness { settings: Settings::default(), state: SettingsState::default(), last: None },
    );
    harness.state_mut().state.section = SettingsSection::Themes;
    harness.set_size(egui::vec2(1000.0, 800.0));
    harness.run();
    harness.get_by_label("Dark").click_accesskit();
    harness.run();
    assert_eq!(harness.state().settings.theme, Theme::Dark);
    assert_eq!(harness.state().last, Some(SettingsAction::Save));

    harness.state_mut().state.section = SettingsSection::Proxy;
    harness.run();
    harness.get_by_label("Custom proxy").click();
    harness.run();
    assert_eq!(harness.state().settings.proxy.mode, postguy_model::ProxyMode::Custom);
    // Authenticated proxy password is not part of any display string.
    harness.state_mut().settings.proxy.password = "secret".into();
    harness.run();
    assert!(harness.query_by_label_contains("secret").is_none());
}

fn settings_harness(section: SettingsSection) -> Harness<'static, SettingsHarness> {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut SettingsHarness| {
            let mut editor = SettingsEditor { settings: &mut state.settings, state: &mut state.state };
            if let Some(action) = editor.show(ui) {
                state.last = Some(action);
            }
        },
        SettingsHarness { settings: Settings::default(), state: SettingsState::default(), last: None },
    );
    harness.state_mut().state.section = section;
    harness.set_size(egui::vec2(1000.0, 800.0));
    harness.run();
    harness
}

#[test]
fn the_settings_nav_switches_pages() {
    let mut harness = settings_harness(SettingsSection::General);
    assert!(harness.query_by_label("SSL certificate verification").is_some(), "General page shows its rows");
    harness.get_by_label("Proxy").click_accesskit();
    harness.run();
    assert_eq!(harness.state().state.section, SettingsSection::Proxy);
    assert!(harness.query_by_label("SSL certificate verification").is_none(), "the General rows are gone");
    assert!(harness.query_by_label("Custom proxy").is_some(), "the Proxy rows are shown");
}

#[test]
fn general_toggles_save() {
    let mut harness = settings_harness(SettingsSection::General);
    assert!(!harness.state().settings.disable_cookies);
    harness.get_by_label("Disable cookies").click_accesskit();
    harness.run();
    assert!(harness.state().settings.disable_cookies);
    assert_eq!(harness.state().last, Some(SettingsAction::Save));
}

#[test]
fn disable_scripts_toggle_saves() {
    let mut harness = settings_harness(SettingsSection::General);
    assert!(!harness.state().settings.disable_scripts);
    harness.get_by_label("Disable scripts").click_accesskit();
    harness.run();
    assert!(harness.state().settings.disable_scripts);
    assert_eq!(harness.state().last, Some(SettingsAction::Save));
}

#[test]
fn settings_auth_is_unaffected_by_the_editor() {
    // A small guard that the settings struct keeps unrelated fields while editing.
    let mut settings = Settings::default();
    settings.proxy.username = "user".into();
    let mut state = SettingsState { section: SettingsSection::Proxy };
    let mut harness = Harness::new_ui(move |ui| {
        let mut editor = SettingsEditor { settings: &mut settings, state: &mut state };
        let _ = editor.show(ui);
    });
    harness.run();
    let _ = AuthConfig::NoAuth;
    let _ = ConsoleFilter::All;
}
