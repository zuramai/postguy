//! The Settings tab: a left navigation (General, Themes, Shortcuts, Data, Certificates, Proxy,
//! About) and, per page, headed groups of rows with the label and description on the left and
//! the control on the right. Every change saves immediately through the app (unknown keys are
//! preserved by the model's `extras`). Fields that nothing reads yet are not shown; they stay in
//! the model so older and newer files round-trip.

use egui::{Align, Layout, Margin, Rect, ScrollArea, Sense, Stroke, Ui, UiBuilder, WidgetInfo, WidgetType, pos2, vec2};
use postguy_model::{HttpVersion, ProxyMode, ProxyProtocol, Settings, Theme};

use crate::theme::Theme as UiTheme;
use crate::widgets;

/// Width of the navigation column.
const NAV_WIDTH: f32 = 220.0;
/// Width of the control column in a row.
const CONTROL_WIDTH: f32 = 220.0;
/// Widest the page content grows.
const CONTENT_MAX_WIDTH: f32 = 760.0;

/// Settings sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsSection {
    #[default]
    General,
    Themes,
    Shortcuts,
    Data,
    Certificates,
    Proxy,
    About,
}

impl SettingsSection {
    const ALL: [(SettingsSection, &'static str, &'static str); 7] = [
        (SettingsSection::General, "settings", "General"),
        (SettingsSection::Themes, "palette", "Themes"),
        (SettingsSection::Shortcuts, "keyboard", "Shortcuts"),
        (SettingsSection::Data, "hard-drive", "Data"),
        (SettingsSection::Certificates, "shield-check", "Certificates"),
        (SettingsSection::Proxy, "network", "Proxy"),
        (SettingsSection::About, "info", "About"),
    ];

    fn label(self) -> &'static str {
        Self::ALL.iter().find(|(section, _, _)| *section == self).map_or("Settings", |(_, _, label)| label)
    }
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
        let full = ui.available_rect_before_wrap();

        let nav_rect = Rect::from_min_size(full.min, vec2(NAV_WIDTH.min(full.width()), full.height()));
        ui.painter().rect_filled(nav_rect, 0.0, theme.palette.surface_alt);
        ui.painter().vline(nav_rect.right(), nav_rect.y_range(), Stroke::new(1.0, theme.palette.border));
        let mut nav = ui.new_child(UiBuilder::new().max_rect(nav_rect.shrink(12.0)).layout(Layout::top_down(Align::Min)));
        nav.spacing_mut().item_spacing.y = 2.0;
        for (section, icon, label) in SettingsSection::ALL {
            if nav_item(&mut nav, icon, label, self.state.section == section).clicked() {
                self.state.section = section;
            }
        }

        let content_rect = Rect::from_min_max(pos2(nav_rect.right() + 1.0, full.top()), full.max);
        let mut content = ui.new_child(UiBuilder::new().max_rect(content_rect).layout(Layout::top_down(Align::Min)));
        ScrollArea::vertical().auto_shrink([false, false]).show(&mut content, |ui| {
            egui::Frame::NONE.inner_margin(Margin { left: 32, right: 32, top: 24, bottom: 32 }).show(ui, |ui| {
                ui.set_max_width(ui.available_width().min(CONTENT_MAX_WIDTH));
                ui.spacing_mut().item_spacing.y = 0.0;
                widgets::plain_text(ui, self.state.section.label(), theme.semibold_font(20.0), theme.palette.text);
                ui.add_space(8.0);
                match self.state.section {
                    SettingsSection::General => changed |= general(ui, self.settings),
                    SettingsSection::Themes => changed |= themes(ui, self.settings),
                    SettingsSection::Shortcuts => shortcuts(ui),
                    SettingsSection::Data => reset = data(ui),
                    SettingsSection::Certificates => changed |= certificates(ui, self.settings),
                    SettingsSection::Proxy => changed |= proxy(ui, self.settings),
                    SettingsSection::About => about(ui),
                }
            });
        });
        ui.allocate_rect(full, Sense::hover());

        if reset {
            Some(SettingsAction::ResetSettings)
        } else if changed {
            Some(SettingsAction::Save)
        } else {
            None
        }
    }
}

/// One navigation entry: icon and label, a filled row when selected or hovered.
fn nav_item(ui: &mut Ui, icon: &str, label: &str, selected: bool) -> egui::Response {
    let theme = UiTheme::current(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    let rounding = egui::CornerRadius::same(6);
    if selected {
        ui.painter().rect_filled(rect, rounding, theme.palette.selected);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, rounding, theme.palette.hover);
    }
    let (color, font) = if selected {
        (theme.palette.text, theme.semibold_font(13.0))
    } else {
        (theme.palette.text_muted, egui::TextStyle::Body.resolve(ui.style()))
    };
    crate::icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(16.0, 16.0)), icon, color, 1.5);
    ui.painter().text(pos2(rect.left() + 34.0, rect.center().y), egui::Align2::LEFT_CENTER, label, font, color);
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, selected, label));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A group heading: a rule under a semibold title.
fn group(ui: &mut Ui, title: &str) {
    let theme = UiTheme::current(ui.ctx());
    ui.add_space(12.0);
    widgets::plain_text(ui, title, theme.semibold_font(15.0), theme.palette.text);
    ui.add_space(8.0);
    rule(ui);
}

fn rule(ui: &mut Ui) {
    let theme = UiTheme::current(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme.palette.border);
}

/// A row: label and description on the left, the control on the right. `control` returns
/// whether it changed the value.
fn setting_row(ui: &mut Ui, label: &str, description: Option<&str>, control: impl FnOnce(&mut Ui) -> bool) -> bool {
    setting_row_sized(ui, label, description, false, 30.0, control)
}

/// [`setting_row`] with the description drawn as a warning.
fn setting_row_with(ui: &mut Ui, label: &str, description: Option<&str>, warn: bool, control: impl FnOnce(&mut Ui) -> bool) -> bool {
    setting_row_sized(ui, label, description, warn, 30.0, control)
}

/// The row implementation; `min_height` is the control column's height when the label is shorter.
fn setting_row_sized(
    ui: &mut Ui,
    label: &str,
    description: Option<&str>,
    warn: bool,
    min_height: f32,
    control: impl FnOnce(&mut Ui) -> bool,
) -> bool {
    let theme = UiTheme::current(ui.ctx());
    let width = ui.available_width();
    let text_width = (width - CONTROL_WIDTH - 16.0).max(120.0);
    ui.add_space(12.0);
    let top = ui.cursor().min;
    let left = ui
        .allocate_ui_with_layout(vec2(text_width, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(text_width);
            widgets::plain_text(ui, label, egui::TextStyle::Body.resolve(ui.style()), theme.palette.text);
            if let Some(description) = description {
                ui.add_space(2.0);
                let color = if warn { theme.palette.warning } else { theme.palette.text_muted };
                widgets::plain_text(ui, description, egui::TextStyle::Small.resolve(ui.style()), color);
            }
        })
        .response
        .rect;
    let row_height = left.height().max(min_height);
    let control_rect = Rect::from_min_size(pos2(top.x + width - CONTROL_WIDTH, top.y), vec2(CONTROL_WIDTH, row_height));
    let mut child = ui.new_child(UiBuilder::new().max_rect(control_rect).layout(Layout::right_to_left(Align::Center)));
    let changed = control(&mut child);
    ui.allocate_space(vec2(0.0, (row_height - left.height()).max(0.0)));
    ui.add_space(12.0);
    rule(ui);
    changed
}

/// A row whose value is a full-width single-line field under the label.
fn path_row(ui: &mut Ui, label: &str, description: Option<&str>, value: &mut String, hint: &str) -> bool {
    let theme = UiTheme::current(ui.ctx());
    ui.add_space(12.0);
    widgets::plain_text(ui, label, egui::TextStyle::Body.resolve(ui.style()), theme.palette.text);
    if let Some(description) = description {
        ui.add_space(2.0);
        widgets::plain_text(ui, description, egui::TextStyle::Small.resolve(ui.style()), theme.palette.text_muted);
    }
    ui.add_space(6.0);
    let changed = ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(f32::INFINITY).margin(Margin::symmetric(8, 6))).changed();
    ui.add_space(12.0);
    rule(ui);
    changed
}

fn toggle_row(ui: &mut Ui, label: &str, description: Option<&str>, value: &mut bool) -> bool {
    setting_row(ui, label, description, |ui| widgets::toggle(ui, value, label).changed())
}

fn general(ui: &mut Ui, settings: &mut Settings) -> bool {
    let mut changed = false;
    group(ui, "Request");
    changed |= setting_row(ui, "HTTP version", Some("The HTTP version used when a request does not choose one."), |ui| {
        http_version(ui, &mut settings.http_version)
    });
    changed |= setting_row(ui, "Request timeout", Some("How long a request waits for a response. Set to 0 to never time out."), |ui| {
        widgets::number_with_unit(ui, "settings-timeout", &mut settings.request_timeout_ms, 0..=3_600_000, "ms", "Request timeout").changed()
    });
    changed |= setting_row(ui, "Max response size", Some("Responses larger than this are cut off. Set to 0 for no limit."), |ui| {
        widgets::number_with_unit(ui, "settings-max-response", &mut settings.max_response_mb, 0..=1024, "MB", "Max response size").changed()
    });
    let verify_description = if settings.verify_ssl {
        "Check server certificates for every request."
    } else {
        "Verification is off: responses are encrypted but unauthenticated."
    };
    let verify_on = settings.verify_ssl;
    changed |= setting_row_with(ui, "SSL certificate verification", Some(verify_description), !verify_on, |ui| {
        widgets::toggle(ui, &mut settings.verify_ssl, "SSL certificate verification").changed()
    });
    changed |= toggle_row(ui, "Automatically follow redirects", Some("Follow 3xx responses to their new location."), &mut settings.follow_redirects);
    changed |= toggle_row(ui, "Disable cookies", Some("Do not send or store cookies for any request."), &mut settings.disable_cookies);
    changed |= toggle_row(ui, "Send no-cache header", Some("Add Cache-Control: no-cache to every request."), &mut settings.send_no_cache_header);

    group(ui, "Working directory");
    changed |=
        path_row(ui, "Location", Some("Files used in request bodies are resolved from here."), &mut settings.working_directory, "<workspace>/files");
    let outside = settings.allow_files_outside_working_directory;
    changed |= setting_row_with(
        ui,
        "Read files outside the working directory",
        Some("Any collection could then read any file on this computer."),
        outside,
        |ui| widgets::toggle(ui, &mut settings.allow_files_outside_working_directory, "Read files outside the working directory").changed(),
    );

    group(ui, "History");
    changed |= toggle_row(ui, "Save response bodies in history", None, &mut settings.save_history_responses);
    changed
}

const ZOOM_STEPS: [(&str, &str); 8] =
    [("0.8", "80%"), ("0.9", "90%"), ("1.0", "100%"), ("1.1", "110%"), ("1.25", "125%"), ("1.5", "150%"), ("1.75", "175%"), ("2.0", "200%")];

fn themes(ui: &mut Ui, settings: &mut Settings) -> bool {
    let mut changed = false;
    group(ui, "Appearance");
    changed |= setting_row(ui, "Theme", Some("System follows the operating system."), |ui| {
        let mut choice = match settings.theme {
            Theme::System => "system".to_string(),
            Theme::Light => "light".to_string(),
            Theme::Dark => "dark".to_string(),
        };
        ui.horizontal(|ui| {
            for (value, label) in [("dark", "Dark"), ("light", "Light"), ("system", "System")] {
                ui.allocate_ui(vec2(72.0, 28.0), |ui| widgets::radio(ui, &mut choice, value, label));
            }
        });
        let new_theme = match choice.as_str() {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::System,
        };
        let differs = new_theme != settings.theme;
        settings.theme = new_theme;
        differs
    });
    changed |= setting_row(ui, "Interface zoom", Some("Scales the whole interface. Cmd/Ctrl + and - change it too."), |ui| {
        let current = crate::commands::sanitize_zoom(settings.ui_zoom);
        let nearest = ZOOM_STEPS
            .iter()
            .min_by(|a, b| {
                let distance = |step: &(&str, &str)| (step.0.parse::<f32>().unwrap_or(1.0) - current).abs();
                distance(a).total_cmp(&distance(b))
            })
            .map_or("1.0", |step| step.0);
        let mut chosen = Some(nearest.to_string());
        let groups =
            [widgets::SelectGroup { label: None, items: ZOOM_STEPS.iter().map(|(value, label)| widgets::SelectItem::new(*value, *label)).collect() }];
        if widgets::select(ui, "settings-zoom", &mut chosen, &groups, "100%") {
            let zoom = chosen.as_deref().and_then(|value| value.parse::<f32>().ok()).unwrap_or(1.0);
            settings.ui_zoom = crate::commands::sanitize_zoom(zoom);
            true
        } else {
            false
        }
    });
    group(ui, "Text");
    changed |= setting_row(ui, "Code font size", Some("Size of text in the request and response editors."), |ui| {
        ui.add(egui::DragValue::new(&mut settings.editor_font_size).range(9.0..=24.0).speed(0.1).suffix(" px")).changed()
    });
    changed
}

fn shortcuts(ui: &mut Ui) {
    let theme = UiTheme::current(ui.ctx());
    let registry = crate::commands::CommandRegistry::default();
    let mut groups: Vec<&str> = Vec::new();
    for command in registry.commands() {
        if command.shortcut.is_some() && !groups.contains(&command.group) {
            groups.push(command.group);
        }
    }
    for name in groups {
        group(ui, name);
        for command in registry.commands().iter().filter(|command| command.group == name) {
            let Some(shortcut) = command.shortcut else { continue };
            let shown = crate::menu::display_shortcut(shortcut, cfg!(target_os = "macos"));
            setting_row(ui, command.label, None, |ui| {
                let keys = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for part in shown.split('+').rev() {
                            widgets::badge(ui, part, theme.palette.text, theme.palette.surface_alt);
                        }
                    })
                    .response;
                keys.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, format!("{}: {shown}", command.label)));
                false
            });
        }
    }
}

/// The Data page; true when the user asked to reset the settings.
fn data(ui: &mut Ui) -> bool {
    let mut reset = false;
    group(ui, "Settings");
    setting_row(ui, "Reset settings", Some("Restore every setting to its default. Collections, environments and history are not touched."), |ui| {
        reset = widgets::text_button(ui, widgets::ButtonKind::Danger, None, "Reset", true).clicked();
        false
    });
    reset
}

fn certificates(ui: &mut Ui, settings: &mut Settings) -> bool {
    let mut changed = false;
    group(ui, "CA certificates");
    changed |= path_row(
        ui,
        "Custom CA bundle (PEM)",
        Some("Added to the system roots for every request; leave empty to use the system roots only."),
        &mut settings.ca_bundle_path,
        "/path/to/ca.pem",
    );
    group(ui, "Client certificate");
    changed |= path_row(ui, "Certificate chain (PEM)", None, &mut settings.client_cert_path, "/path/to/client.pem");
    changed |= path_row(
        ui,
        "Private key (PEM)",
        Some("PKCS#8, PKCS#1 or SEC1. Encrypted keys and PFX files are not supported yet."),
        &mut settings.client_key_path,
        "/path/to/client.key",
    );
    changed
}

fn proxy(ui: &mut Ui, settings: &mut Settings) -> bool {
    let mut changed = false;
    let proxy = &mut settings.proxy;
    group(ui, "Proxy server");
    changed |= setting_row_sized(
        ui,
        "Proxy mode",
        Some("The system proxy comes from the HTTP_PROXY environment variables. A custom proxy wins. The effective proxy is logged in the Console."),
        false,
        60.0,
        |ui| {
            let mut mode = if proxy.mode == ProxyMode::Custom { "custom".to_string() } else { "system".to_string() };
            ui.vertical(|ui| {
                widgets::radio(ui, &mut mode, "system", "System proxy");
                widgets::radio(ui, &mut mode, "custom", "Custom proxy");
            });
            let new_mode = if mode == "custom" { ProxyMode::Custom } else { ProxyMode::System };
            let differs = new_mode != proxy.mode;
            proxy.mode = new_mode;
            differs
        },
    );
    if proxy.mode == ProxyMode::Custom {
        group(ui, "Custom proxy settings");
        changed |= setting_row(ui, "Protocol", None, |ui| {
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
                true
            } else {
                false
            }
        });
        changed |= setting_row(ui, "Host", None, |ui| ui.add(egui::TextEdit::singleline(&mut proxy.host).desired_width(200.0)).changed());
        changed |= setting_row(ui, "Port", None, |ui| ui.add(egui::DragValue::new(&mut proxy.port).range(1..=65_535)).changed());
        changed |= setting_row(ui, "Username", None, |ui| ui.add(egui::TextEdit::singleline(&mut proxy.username).desired_width(200.0)).changed());
        changed |= setting_row(ui, "Password", None, |ui| {
            ui.add(egui::TextEdit::singleline(&mut proxy.password).password(true).desired_width(200.0)).changed()
        });
        changed |= toggle_row(ui, "Apply to HTTP", None, &mut proxy.apply_to_http);
        changed |= toggle_row(ui, "Apply to HTTPS", None, &mut proxy.apply_to_https);
        changed |= path_row(ui, "Bypass list", Some("Hosts that skip the proxy, separated by commas."), &mut proxy.bypass, "localhost, *.internal");
    }
    changed
}

fn about(ui: &mut Ui) {
    let theme = UiTheme::current(ui.ctx());
    group(ui, &format!("Postguy {}", env!("CARGO_PKG_VERSION")));
    for (label, value) in [
        ("Licence", "MIT OR Apache-2.0"),
        ("Privacy", "Offline. No telemetry, no update check"),
        ("Third-party notices", "ATTRIBUTION.md and NOTICE in the repository"),
    ] {
        setting_row(ui, label, None, |ui| {
            ui.label(egui::RichText::new(value).color(theme.palette.text_muted));
            false
        });
    }
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
