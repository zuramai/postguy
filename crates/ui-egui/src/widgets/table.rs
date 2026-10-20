//! Key/value tables (PROMPT.md 6.3): enabled square checkbox, key, value, description, a
//! trailing placeholder row, per-row delete on hover, and variants for secret rows and
//! file-or-text form-data rows. Rows are virtualised above the visible-height threshold via
//! `ScrollArea::show_rows`; a 10 000-row table paints only its window.

use egui::{Response, Ui};
use postguy_model::{KeyValue, KeyValueKind};

use crate::widgets::compact_checkbox;

/// What a table is editing; the value cell changes shape for file rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TableVariant {
    #[default]
    Normal,
    /// Values are secrets: masked unless the user reveals them.
    Secret,
    /// Rows may switch between text and a file path (form-data).
    FileText,
}

/// The table's return value.
#[derive(Debug, Clone, Copy, Default)]
pub struct TableResponse {
    pub changed: bool,
    /// A row was deleted (the caller may need to mark the document dirty).
    pub removed: bool,
}

const ROW_HEIGHT: f32 = 28.0;

/// A key/value table bound to `rows`. `variant` picks the value cell; `max_height` enables
/// virtualised scrolling (0 = natural height, for small fixed tables).
pub fn key_value_table(ui: &mut Ui, rows: &mut Vec<KeyValue>, variant: TableVariant) -> TableResponse {
    let mut result = TableResponse::default();
    let theme = crate::theme::Theme::current(ui.ctx());
    let total = rows.len().saturating_add(1);
    let max_height = 360.0;
    let row_height = ROW_HEIGHT;
    let content_height = (total as f32) * row_height;
    let mut remove_index: Option<usize> = None;
    let mut add_row = false;

    let mut render_rows = |ui: &mut Ui, range: std::ops::Range<usize>| {
        for index in range {
            if index >= rows.len() {
                // The placeholder "add" row.
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    let response =
                        ui.add(egui::Label::new(egui::RichText::new("+ Add row").color(theme.palette.text_muted)).sense(egui::Sense::click()));
                    if response.clicked() {
                        add_row = true;
                    }
                });
                continue;
            }
            let row = &mut rows[index];
            ui.horizontal(|ui| {
                ui.set_height(row_height - 4.0);
                if compact_checkbox(ui, &mut row.enabled).changed() {
                    result.changed = true;
                }
                let key_width = (ui.available_width() * 0.26).max(80.0);
                let value_width = (ui.available_width() * 0.34).max(100.0);
                let key = ui.add(egui::TextEdit::singleline(&mut row.key).hint_text("Key").desired_width(key_width).frame(egui::Frame::NONE));
                if key.changed() {
                    result.changed = true;
                }
                match variant {
                    TableVariant::FileText => {
                        let mut kind = row.kind;
                        egui::ComboBox::from_id_salt(("kv-kind", index))
                            .width(64.0)
                            .selected_text(match kind {
                                KeyValueKind::Text => "Text",
                                KeyValueKind::File => "File",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut kind, KeyValueKind::Text, "Text");
                                ui.selectable_value(&mut kind, KeyValueKind::File, "File");
                            });
                        if kind != row.kind {
                            row.kind = kind;
                            result.changed = true;
                        }
                        let value = ui.add(
                            egui::TextEdit::singleline(&mut row.value).hint_text("Value or path").desired_width(value_width).frame(egui::Frame::NONE),
                        );
                        if value.changed() {
                            result.changed = true;
                        }
                    }
                    TableVariant::Secret => {
                        let value = ui.add(
                            egui::TextEdit::singleline(&mut row.value)
                                .password(true)
                                .hint_text("Secret value")
                                .desired_width(value_width)
                                .frame(egui::Frame::NONE),
                        );
                        if value.changed() {
                            result.changed = true;
                        }
                    }
                    TableVariant::Normal => {
                        let value =
                            ui.add(egui::TextEdit::singleline(&mut row.value).hint_text("Value").desired_width(value_width).frame(egui::Frame::NONE));
                        if value.changed() {
                            result.changed = true;
                        }
                    }
                }
                let remaining = (ui.available_width() - 32.0).max(60.0);
                let description = ui
                    .add(egui::TextEdit::singleline(&mut row.description).hint_text("Description").desired_width(remaining).frame(egui::Frame::NONE));
                if description.changed() {
                    result.changed = true;
                }
                let delete = ui.add(egui::Button::new("x").frame(false).small());
                if delete.hovered() {
                    ui.painter().rect_filled(delete.rect, 4.0, theme.palette.surface_sunken);
                }
                if delete.clicked() {
                    remove_index = Some(index);
                }
            });
            ui.separator();
        }
    };

    if content_height > max_height {
        egui::ScrollArea::vertical().max_height(max_height).id_salt("kv-scroll").show_rows(ui, row_height, total, |ui, range| {
            render_rows(ui, range);
        });
    } else {
        render_rows(ui, 0..total);
    }
    if let Some(index) = remove_index
        && index < rows.len()
    {
        rows.remove(index);
        result.removed = true;
        result.changed = true;
    }
    if add_row {
        rows.push(KeyValue::default());
        result.changed = true;
    }
    result
}

/// Read-only automatic rows (Host, User-Agent, …) with an info tooltip. Not editable and not
/// part of the file model.
pub fn automatic_rows(ui: &mut Ui, rows: &[(String, String)], tooltip: &str) -> Response {
    let theme = crate::theme::Theme::current(ui.ctx());
    let mut response = ui.label(egui::RichText::new("Auto-generated").small().color(theme.palette.text_muted));
    for (name, value) in rows {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label(egui::RichText::new(name).color(theme.palette.text_muted).monospace());
            ui.label(egui::RichText::new(value).color(theme.palette.text_muted));
            ui.label(egui::RichText::new("ⓘ").small().color(theme.palette.text_muted)).on_hover_text(tooltip);
        });
    }
    response = response.on_hover_text(tooltip);
    response
}

/// Stable helper for tests and the request builder's Params tab: one empty enabled row.
pub fn placeholder_row() -> KeyValue {
    KeyValue { key: String::new(), value: String::new(), enabled: true, ..KeyValue::default() }
}

/// Row size used by the virtualised table (tests use it to assert the window).
pub const ROW_SIZE: f32 = ROW_HEIGHT;

#[cfg(test)]
mod tests {
    use super::placeholder_row;
    use postguy_model::KeyValueKind;

    #[test]
    fn rows_start_enabled_and_empty() {
        let row = placeholder_row();
        assert!(row.enabled);
        assert!(row.key.is_empty());
        assert_eq!(row.kind, KeyValueKind::Text);
    }
}
