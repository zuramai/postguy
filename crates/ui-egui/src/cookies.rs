//! Cookies manager (PROMPT.md 5.5, M3.9): every jar cookie with flags and expiry, search, delete
//! and clear-all, plus the domain allowlist that scripts may touch. The jar itself lives in the
//! engine; this widget is data-in/actions-out.

use egui::{Align, Layout, RichText, ScrollArea, Ui};

use crate::theme::Theme;
use crate::widgets;

/// One cookie row as the app maps it from the jar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieRow {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires_ms: Option<i64>,
}

/// The manager's data.
#[derive(Debug, Clone, Default)]
pub struct CookieData {
    pub cookies: Vec<CookieRow>,
    pub allowlist: Vec<String>,
}

/// Actions the app applies through the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CookieAction {
    Delete { domain: String, path: String, name: String },
    Clear,
    Allow(String),
    Disallow(String),
}

/// State kept per frame/tab.
#[derive(Debug, Clone, Default)]
pub struct CookieState {
    pub search: String,
    pub new_domain: String,
}

/// Draw the manager.
pub fn cookie_manager(ui: &mut Ui, data: &CookieData, state: &mut CookieState) -> Option<CookieAction> {
    let theme = Theme::current(ui.ctx());
    let mut action = None;
    ui.horizontal(|ui| {
        ui.heading("Cookies");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add(egui::Button::new("Clear all")).clicked() {
                action = Some(CookieAction::Clear);
            }
            ui.add(egui::TextEdit::singleline(&mut state.search).hint_text("Search cookies…").desired_width(200.0));
        });
    });
    ui.separator();
    ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add_sized([150.0, 18.0], egui::Label::new(RichText::new("DOMAIN").small().color(theme.palette.text_muted)));
            ui.add_sized([150.0, 18.0], egui::Label::new(RichText::new("NAME").small().color(theme.palette.text_muted)));
            ui.add_sized([220.0, 18.0], egui::Label::new(RichText::new("VALUE").small().color(theme.palette.text_muted)));
            ui.add_sized([90.0, 18.0], egui::Label::new(RichText::new("FLAGS").small().color(theme.palette.text_muted)));
        });
        ui.separator();
        let needle = state.search.trim().to_ascii_lowercase();
        let mut remove = None;
        for cookie in data.cookies.iter().filter(|cookie| {
            needle.is_empty() || cookie.domain.to_ascii_lowercase().contains(&needle) || cookie.name.to_ascii_lowercase().contains(&needle)
        }) {
            ui.horizontal(|ui| {
                ui.add_sized([150.0, 20.0], egui::Label::new(RichText::new(&cookie.domain).monospace().color(theme.palette.text)));
                ui.add_sized([150.0, 20.0], egui::Label::new(RichText::new(&cookie.name).monospace().color(theme.palette.text)));
                ui.add_sized(
                    [220.0, 20.0],
                    egui::Label::new(RichText::new(&cookie.value).monospace().small().color(theme.palette.text_muted)).truncate(),
                );
                let flags = format!(
                    "{}{}{}",
                    if cookie.secure { "S" } else { "-" },
                    if cookie.http_only { "H" } else { "-" },
                    match cookie.expires_ms {
                        Some(_) => "T",
                        None => "-",
                    }
                );
                ui.add_sized([90.0, 20.0], egui::Label::new(RichText::new(flags).monospace().small().color(theme.palette.text_muted)))
                    .on_hover_text("S = Secure · H = HttpOnly · T = expires · S(session)");
                ui.label(RichText::new(&cookie.path).small().color(theme.palette.text_muted));
                if widgets::icon_button(ui, "x", "Delete cookie").clicked() {
                    remove = Some(cookie.clone());
                }
            });
            ui.separator();
        }
        if let Some(cookie) = remove {
            action = Some(CookieAction::Delete { domain: cookie.domain, path: cookie.path, name: cookie.name });
        }
        if data.cookies.is_empty() {
            ui.label(RichText::new("No cookies in the jar.").color(theme.palette.text_muted));
        }
    });
    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new("Script domain allowlist").strong());
    ui.label(RichText::new("Scripts may read and write jar cookies only for these domains.").small().color(theme.palette.text_muted));
    ui.horizontal(|ui| {
        let response = ui.add(egui::TextEdit::singleline(&mut state.new_domain).hint_text("example.com").desired_width(220.0));
        if ui.add(egui::Button::new("Allow")).clicked() && !state.new_domain.trim().is_empty() {
            action = Some(CookieAction::Allow(state.new_domain.trim().to_string()));
            state.new_domain.clear();
        }
        let _ = response;
    });
    for domain in &data.allowlist {
        ui.horizontal(|ui| {
            ui.label(RichText::new(domain).monospace());
            if ui.add(egui::Button::new("x").frame(false).small()).clicked() {
                action = Some(CookieAction::Disallow(domain.clone()));
            }
        });
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_data_defaults_are_empty() {
        let data = CookieData::default();
        assert!(data.cookies.is_empty() && data.allowlist.is_empty());
    }
}
