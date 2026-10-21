//! The Settings tab (PROMPT.md section 7, M3.9): General, Editor, Themes, Data, Certificates,
//! Proxy and About. Every change saves immediately through the app (unknown keys are preserved by
//! the model's `extras`). Network-touching/import-export buttons point at their milestones.

use egui::{RichText, ScrollArea, Ui};
use postguy_model::{HttpVersion, ProxyMode, ProxyProtocol, ResponseFormatDetection, Settings, Theme};

use crate::theme::Theme as UiTheme;
use crate::widgets;

/// Settings sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsSection {
    #[default]
    General,
    Editor,
    Themes,
    Shortcuts,
    Data,
    Certificates,
    Proxy,
    About,
}

/// State kept across frames.
#[derive(Debug, Clone, Default)]
pub struct SettingsState {
    pub section: SettingsSection,
}

/// What the app should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsAction {
    Save,
    /// The Data tab's reset action (settings only, never workspace data).
    ResetSettings,
}

/// The editor.
pub struct SettingsEditor<'a> {
    pub settings: &'a mut Settings,
    pub state: &'a mut SettingsState,
}

impl SettingsEditor<'_> {
    pub fn show(&mut self, ui: &mut Ui) -> Option<SettingsAction> {
        let theme = UiTheme::current(ui.ctx());
        let mut changed = false;
        let mut reset = false;
        ui.heading("Settings");
        ui.separator();
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_min_width(160.0);
                for (section, label) in [
                    (SettingsSection::General, "General"),
                    (SettingsSection::Editor, "Editor"),
                    (SettingsSection::Themes, "Themes"),
                    (SettingsSection::Shortcuts, "Shortcuts"),
                    (SettingsSection::Data, "Data"),
                    (SettingsSection::Certificates, "Certificates"),
                    (SettingsSection::Proxy, "Proxy"),
                    (SettingsSection::About, "About"),
                ] {
                    if ui
                        .add(egui::Button::new(label).selected(self.state.section == section).frame(false).min_size(egui::vec2(150.0, 24.0)))
                        .clicked()
                    {
                        self.state.section = section;
                    }
                }
            });
            ui.separator();
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.state.section {
                SettingsSection::General => changed |= general(ui, self.settings),
                SettingsSection::Editor => changed |= editor(ui, self.settings),
                SettingsSection::Themes => changed |= themes(ui, self.settings),
                SettingsSection::Shortcuts => shortcuts(ui),
                SettingsSection::Data => {
                    if ui.button("Reset settings (not workspace data)").clicked() {
                        reset = true;
                    }
                    ui.label(RichText::new("Import data (zip or folder) and Export all data arrive in M4.").color(theme.palette.text_muted));
                }
                SettingsSection::Certificates => changed |= certificates(ui, self.settings),
                SettingsSection::Proxy => changed |= proxy(ui, self.settings),
                SettingsSection::About => {
                    ui.label(RichText::new(format!("Postguy {}", env!("CARGO_PKG_VERSION"))).strong());
                    ui.label("MIT OR Apache-2.0 · offline, no telemetry, no update check.");
                    ui.label(
                        RichText::new("Third-party notices: ATTRIBUTION.md / NOTICE in the repository.").small().color(theme.palette.text_muted),
                    );
                }
            });
        });
        if reset {
            Some(SettingsAction::ResetSettings)
        } else if changed {
            Some(SettingsAction::Save)
        } else {
            None
        }
    }
}

fn certificates(ui: &mut Ui, settings: &mut Settings) -> bool {
    let theme = UiTheme::current(ui.ctx());
    let mut changed = false;
    ui.label(RichText::new("Server trust").strong());
    ui.label("Custom CA bundle (PEM)");
    changed |= ui.add(egui::TextEdit::singleline(&mut settings.ca_bundle_path).hint_text("/path/to/ca.pem").desired_width(360.0)).changed();
    ui.label(
        RichText::new("Added to the OS trust store for every request; leave empty to use the system roots only.")
            .small()
            .color(theme.palette.text_muted),
    );
    ui.add_space(12.0);
    ui.label(RichText::new("Client certificate").strong());
    ui.label("Certificate chain (PEM)");
    changed |= ui.add(egui::TextEdit::singleline(&mut settings.client_cert_path).hint_text("/path/to/client.pem").desired_width(360.0)).changed();
    ui.label("Private key (PEM, PKCS#8/PKCS#1/SEC1)");
    changed |= ui.add(egui::TextEdit::singleline(&mut settings.client_key_path).hint_text("/path/to/client.key").desired_width(360.0)).changed();
    ui.label(
        RichText::new("Used for HTTPS requests that ask for a client certificate. Encrypted keys and PFX files are not supported yet.")
            .small()
            .color(theme.palette.text_muted),
    );
    ui.add_space(12.0);
    ui.label(RichText::new("SSL verification").strong());
    changed |= widgets::switch(ui, &mut settings.verify_ssl, "Enable SSL certificate verification").changed();
    if !settings.verify_ssl {
        ui.label(RichText::new("Verification is off: responses are encrypted but unauthenticated.").color(theme.palette.warning));
    }
    changed
}

fn shortcuts(ui: &mut Ui) {
    ui.label(RichText::new("Shortcuts").strong());
    ui.separator();
    let registry = crate::commands::CommandRegistry::default();
    for command in registry.commands() {
        if let Some(shortcut) = command.shortcut {
            ui.horizontal(|ui| {
                ui.add_sized([240.0, 26.0], egui::Label::new(RichText::new(command.label)));
                ui.add_sized([130.0, 26.0], egui::Label::new(RichText::new(shortcut).strong().color(UiTheme::current(ui.ctx()).palette.text_muted)));
                ui.label(RichText::new(command.group).small().color(UiTheme::current(ui.ctx()).palette.text_muted));
            });
        }
    }
}

fn general(ui: &mut Ui, settings: &mut Settings) -> bool {
    let mut changed = false;
    ui.label(RichText::new("General").strong());
    ui.separator();
    changed |= http_version(ui, &mut settings.http_version);
    ui.add_space(6.0);
    changed |= widgets::switch(ui, &mut settings.verify_ssl, "Enable SSL certificate verification").changed();
    changed |= widgets::switch(ui, &mut settings.disable_cookies, "Disable cookies").changed();
    changed |= widgets::switch(ui, &mut settings.follow_redirects, "Automatically follow redirects").changed();
    changed |= widgets::switch(ui, &mut settings.send_no_cache_header, "Send no-cache header").changed();
    changed |= widgets::switch(ui, &mut settings.trim_keys_and_values, "Trim keys and values in tables").changed();
    changed |=
        widgets::switch(ui, &mut settings.allow_files_outside_working_directory, "Allow reading files outside the working directory").changed();
    changed |= widgets::switch(ui, &mut settings.always_open_in_new_tab, "Always open sidebar items in a new tab").changed();
    changed |= widgets::switch(ui, &mut settings.always_ask_on_close, "Always ask when closing unsaved tabs").changed();
    changed |= widgets::switch(ui, &mut settings.show_icons_with_tab_names, "Show icons with tab names").changed();
    changed |= widgets::switch(ui, &mut settings.two_pane_layout, "Two-pane layout").changed();
    changed |= widgets::switch(ui, &mut settings.variable_autocomplete, "Variable autocomplete").changed();
    changed |= widgets::switch(ui, &mut settings.autosave, "Autosave").changed();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("Request timeout (ms; 0 = none)");
        changed |= ui.add(egui::DragValue::new(&mut settings.request_timeout_ms).range(0..=3_600_000)).changed();
    });
    ui.horizontal(|ui| {
        ui.label("Max response size (MB; 0 = unlimited)");
        changed |= ui.add(egui::DragValue::new(&mut settings.max_response_mb).range(0..=1024)).changed();
    });
    ui.horizontal(|ui| {
        ui.label("Working directory");
        changed |= ui.add(egui::TextEdit::singleline(&mut settings.working_directory).hint_text("<workspace>/files").desired_width(360.0)).changed();
    });
    ui.horizontal(|ui| {
        ui.label("Response format detection");
        let mut chosen = Some(match settings.response_format_detection {
            ResponseFormatDetection::Auto => "auto".to_string(),
            ResponseFormatDetection::ForceJson => "json".to_string(),
        });
        let groups = [widgets::SelectGroup {
            label: None,
            items: vec![widgets::SelectItem::new("auto", "Auto"), widgets::SelectItem::new("json", "Force JSON")],
        }];
        if widgets::select(ui, "settings-response-format", &mut chosen, &groups, "Auto") {
            settings.response_format_detection =
                if chosen.as_deref() == Some("json") { ResponseFormatDetection::ForceJson } else { ResponseFormatDetection::Auto };
            changed = true;
        }
    });
    ui.add_space(8.0);
    ui.label(RichText::new("History").strong());
    ui.horizontal(|ui| {
        ui.label("History entries cap");
        changed |= ui.add(egui::DragValue::new(&mut settings.history_cap).range(100..=100_000)).changed();
    });
    changed |= widgets::switch(ui, &mut settings.save_history_responses, "Save response bodies in history").changed();
    changed
}

fn editor(ui: &mut Ui, settings: &mut Settings) -> bool {
    let mut changed = false;
    ui.label(RichText::new("Editor").strong());
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Font size");
        changed |= ui.add(egui::DragValue::new(&mut settings.editor_font_size).range(9.0..=24.0).speed(0.1)).changed();
    });
    ui.horizontal(|ui| {
        ui.label("Indent size (JSON)");
        changed |= ui.add(egui::DragValue::new(&mut settings.editor_indent_json).range(1..=8)).changed();
    });
    ui.horizontal(|ui| {
        ui.label("Indent size (other)");
        changed |= ui.add(egui::DragValue::new(&mut settings.editor_indent_other).range(1..=8)).changed();
    });
    changed |= widgets::switch(ui, &mut settings.editor_spaces, "Indent with spaces").changed();
    changed |= widgets::switch(ui, &mut settings.editor_auto_close, "Auto-close brackets and quotes").changed();
    changed |= widgets::switch(ui, &mut settings.editor_word_wrap, "Word wrap").changed();
    changed
}

fn themes(ui: &mut Ui, settings: &mut Settings) -> bool {
    let mut changed = false;
    ui.label(RichText::new("Theme").strong());
    ui.separator();
    let mut choice = match settings.theme {
        Theme::System => "system".to_string(),
        Theme::Light => "light".to_string(),
        Theme::Dark => "dark".to_string(),
    };
    widgets::radio(ui, &mut choice, "system", "System (follow the OS)");
    widgets::radio(ui, &mut choice, "light", "Light");
    widgets::radio(ui, &mut choice, "dark", "Dark");
    let theme = match choice.as_str() {
        "light" => Theme::Light,
        "dark" => Theme::Dark,
        _ => Theme::System,
    };
    if theme != settings.theme {
        settings.theme = theme;
        changed = true;
    }
    changed
}

fn proxy(ui: &mut Ui, settings: &mut Settings) -> bool {
    let mut changed = false;
    ui.label(RichText::new("Proxy").strong());
    ui.separator();
    let proxy = &mut settings.proxy;
    let mut mode = if proxy.mode == ProxyMode::Custom { "custom".to_string() } else { "system".to_string() };
    widgets::radio(ui, &mut mode, "system", "Use system proxy (environment)");
    widgets::radio(ui, &mut mode, "custom", "Custom proxy");
    let new_mode = if mode == "custom" { ProxyMode::Custom } else { ProxyMode::System };
    if new_mode != proxy.mode {
        proxy.mode = new_mode;
        changed = true;
    }
    ui.add_space(6.0);
    if proxy.mode == ProxyMode::Custom {
        ui.horizontal(|ui| {
            ui.label("Protocol");
            let mut chosen = Some(match proxy.protocol {
                ProxyProtocol::Http => "http".to_string(),
                ProxyProtocol::Socks5 => "socks5".to_string(),
                ProxyProtocol::Socks5h => "socks5h".to_string(),
            });
            let groups = [widgets::SelectGroup {
                label: None,
                items: vec![
                    widgets::SelectItem::new("http", "HTTP"),
                    widgets::SelectItem::new("socks5", "SOCKS5"),
                    widgets::SelectItem::new("socks5h", "SOCKS5h"),
                ],
            }];
            if widgets::select(ui, "proxy-protocol", &mut chosen, &groups, "HTTP") {
                proxy.protocol = match chosen.as_deref() {
                    Some("socks5") => ProxyProtocol::Socks5,
                    Some("socks5h") => ProxyProtocol::Socks5h,
                    _ => ProxyProtocol::Http,
                };
                changed = true;
            }
            ui.label("Host");
            changed |= ui.add(egui::TextEdit::singleline(&mut proxy.host).desired_width(180.0)).changed();
            ui.label("Port");
            changed |= ui.add(egui::DragValue::new(&mut proxy.port).range(1..=65_535)).changed();
        });
        ui.horizontal(|ui| {
            ui.label("Username");
            changed |= ui.add(egui::TextEdit::singleline(&mut proxy.username).desired_width(160.0)).changed();
            ui.label("Password");
            changed |= ui.add(egui::TextEdit::singleline(&mut proxy.password).password(true).desired_width(160.0)).changed();
        });
        ui.horizontal(|ui| {
            changed |= widgets::switch(ui, &mut proxy.apply_to_http, "Apply to HTTP").changed();
            changed |= widgets::switch(ui, &mut proxy.apply_to_https, "Apply to HTTPS").changed();
        });
        ui.horizontal(|ui| {
            ui.label("Bypass list");
            changed |= ui.add(egui::TextEdit::singleline(&mut proxy.bypass).hint_text("localhost, *.internal").desired_width(320.0)).changed();
        });
        ui.label(
            RichText::new("The effective proxy is logged in the Console; custom wins over system.")
                .small()
                .color(UiTheme::current(ui.ctx()).palette.text_muted),
        );
    }
    changed
}

fn http_version(ui: &mut Ui, version: &mut HttpVersion) -> bool {
    let mut chosen = Some(match version {
        HttpVersion::Auto => "auto".to_string(),
        HttpVersion::Http1 => "http1".to_string(),
        HttpVersion::Http2 => "http2".to_string(),
    });
    let groups = [widgets::SelectGroup {
        label: None,
        items: vec![
            widgets::SelectItem::new("auto", "Auto"),
            widgets::SelectItem::new("http1", "HTTP/1.x"),
            widgets::SelectItem::new("http2", "HTTP/2"),
        ],
    }];
    if widgets::select(ui, "general-http", &mut chosen, &groups, "Auto") {
        *version = match chosen.as_deref() {
            Some("http1") => HttpVersion::Http1,
            Some("http2") => HttpVersion::Http2,
            _ => HttpVersion::Auto,
        };
        true
    } else {
        false
    }
}
