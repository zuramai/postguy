//! Environment and Globals editor: editable title and colour dot, search,
//! a variables table with enabled/name/value/description, a secret flag with masked values
//! (hold Cmd/Ctrl or the reveal toggle to see them), per-row delete, drag reordering and a
//! trailing add row. Changes autosave: the widget returns [`EnvironmentAction::Save`] whenever a
//! field changed and the app writes atomically through the engine (secret values are split into
//! the secrets file by the store).

use egui::{Align, Color32, Layout, RichText, ScrollArea, Sense, Ui, vec2};
use postguy_model::Environment;

use crate::theme::Theme;
use crate::widgets::{self, menu_item};

/// What the editor asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentAction {
    Save,
    Duplicate,
    Export,
    Delete,
    CreateEnvironment,
    Close,
}

/// Everything the editor needs for one frame.
pub struct EnvironmentEditor<'a> {
    pub environment: &'a mut Environment,
    /// Globals use a single fixed file: no duplicate/export/delete menu.
    pub globals: bool,
    pub search: &'a mut String,
    pub show_description: &'a mut bool,
    pub reveal: &'a mut bool,
}

impl EnvironmentEditor<'_> {
    pub fn show(&mut self, ui: &mut Ui) -> Option<EnvironmentAction> {
        let theme = Theme::current(ui.ctx());
        let mut action = None;
        let mut changed = false;
        ui.horizontal(|ui| {
            self.color_dot(ui, &theme, &mut changed);
            let name = ui.add(
                egui::TextEdit::singleline(&mut self.environment.name).frame(egui::Frame::NONE).font(egui::TextStyle::Heading).desired_width(240.0),
            );
            name.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Environment name"));
            if name.changed() {
                changed = true;
            }
        });
        // The search and the view toggles get their own row so neither can overlap the name when
        // the panel is narrow (a single row overlapped at the default width).
        ui.horizontal(|ui| {
            ui.add_sized([190.0, 26.0], egui::TextEdit::singleline(self.search).hint_text("Search variables…"));
            let mut show = *self.show_description;
            if widgets::checkbox(ui, &mut show, "Description").changed() {
                *self.show_description = show;
            }
            if ui.add(egui::Button::new("Reveal secrets").selected(*self.reveal)).clicked() {
                *self.reveal = !*self.reveal;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.globals {
                    ui.label(RichText::new("Globals").small().color(theme.palette.text_muted));
                } else {
                    let menu = ui.add(egui::Button::new("...").frame(false));
                    egui::Popup::menu(&menu).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
                        ui.set_min_width(180.0);
                        if menu_item(ui, None, "Duplicate", None, true).clicked() {
                            action = Some(EnvironmentAction::Duplicate);
                        }
                        if menu_item(ui, None, "Export", None, true).clicked() {
                            action = Some(EnvironmentAction::Export);
                        }
                        ui.separator();
                        if menu_item(ui, None, "Delete", None, true).clicked() {
                            action = Some(EnvironmentAction::Delete);
                        }
                    });
                }
            });
        });
        ui.separator();
        let search = self.search.trim().to_ascii_lowercase();
        let reveal_all = *self.reveal || ui.input(|input| input.modifiers.command);
        let show_description = *self.show_description;

        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let grid = widgets::VariableGrid { salt: self.environment.id.0.as_str(), show_description, reveal: reveal_all, search: &search };
            if widgets::variable_table(ui, &mut self.environment.variables, &grid) {
                changed = true;
            }
        });
        if changed {
            action = Some(EnvironmentAction::Save);
        }
        action
    }

    fn color_dot(&mut self, ui: &mut Ui, theme: &Theme, changed: &mut bool) {
        let color = self.environment.color.as_deref().and_then(parse_hex).unwrap_or(theme.palette.accent);
        let (rect, response) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
        ui.painter().circle_filled(rect.center(), 6.0, color);
        response.clone().on_hover_text("Environment colour").on_hover_cursor(egui::CursorIcon::PointingHand);
        egui::Popup::menu(&response).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
            ui.set_min_width(150.0);
            ui.horizontal_wrapped(|ui| {
                if ui.add(egui::Button::new("None").frame(false)).clicked() {
                    self.environment.color = None;
                    *changed = true;
                    ui.close();
                }
                for candidate in ["#2F6FEB", "#1F9D55", "#B7791F", "#D64545", "#7C5CC4", "#2B9C8F", "#C2549B"] {
                    let (dot, response) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                    response.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
                    ui.painter().circle_filled(dot.center(), 7.0, parse_hex(candidate).unwrap_or(theme.palette.accent));
                    if response.clicked() {
                        self.environment.color = Some(candidate.to_string());
                        *changed = true;
                        ui.close();
                    }
                }
            });
        });
    }
}

fn parse_hex(value: &str) -> Option<Color32> {
    let value = value.trim().trim_start_matches('#');
    if value.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(value.get(0..2)?, 16).ok()?;
    let g = u8::from_str_radix(value.get(2..4)?, 16).ok()?;
    let b = u8::from_str_radix(value.get(4..6)?, 16).ok()?;
    Some(Color32::from_rgb(r, g, b))
}

/// A grouped, read-only quick look at variables (active environment, globals, collection).
#[derive(Debug, Clone, Default)]
pub struct VariableGroup {
    pub title: String,
    pub variables: Vec<(String, String, bool)>,
}

/// The selector's quick-look popover: groups, secret masking and a reveal toggle. Clicking a
/// row copies its value; editing in place arrives with the field popovers.
pub fn variable_quick_look(ui: &mut Ui, groups: &[VariableGroup]) {
    let theme = Theme::current(ui.ctx());
    let mut reveal = ui.data(|data| data.get_temp::<bool>(egui::Id::new("quicklook-reveal"))).unwrap_or(false);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Variables").strong());
        if ui.add(egui::Button::new("Reveal secrets").selected(reveal).small()).clicked() {
            reveal = !reveal;
            ui.data_mut(|data| data.insert_temp(egui::Id::new("quicklook-reveal"), reveal));
        }
    });
    ui.separator();
    ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
        for group in groups {
            if group.variables.is_empty() {
                continue;
            }
            ui.label(RichText::new(&group.title).small().color(theme.palette.text_muted));
            for (name, value, secret) in &group.variables {
                ui.horizontal(|ui| {
                    ui.set_height(32.0);
                    ui.add_sized([140.0, 24.0], egui::Label::new(RichText::new(name).strong().color(theme.palette.text)));
                    let shown = if *secret && !reveal { "••••••".to_string() } else { value.clone() };
                    let label = ui.add(egui::Label::new(RichText::new(&shown).small().color(theme.palette.text_muted)).truncate());
                    label.clone().on_hover_text("Click to copy");
                    if label.clicked() {
                        ui.ctx().copy_text(value.clone());
                    }
                });
            }
            ui.add_space(4.0);
        }
        if groups.iter().all(|group| group.variables.is_empty()) {
            ui.label(RichText::new("No variables in this scope.").color(theme.palette.text_muted));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colours_parse_and_reject_bad_input() {
        assert_eq!(parse_hex("#2F6FEB"), Some(Color32::from_rgb(0x2f, 0x6f, 0xeb)));
        assert_eq!(parse_hex("2f6feb"), Some(Color32::from_rgb(0x2f, 0x6f, 0xeb)));
        assert!(parse_hex("#xyz").is_none());
        assert!(parse_hex("").is_none());
    }
}
