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

const ROW_HEIGHT: f32 = 32.0;
const HEADER_HEIGHT: f32 = 28.0;
const CHECK_WIDTH: f32 = 34.0;
const DELETE_WIDTH: f32 = 30.0;

/// Column x-ranges inside a row of `width` starting at `left`: check, key, value, description,
/// delete.
fn columns(left: f32, width: f32) -> [(f32, f32); 5] {
    let rest = (width - CHECK_WIDTH - DELETE_WIDTH).max(120.0);
    let (key, value) = (rest * 0.3, rest * 0.35);
    let description = rest - key - value;
    let mut x = left;
    let mut out = [(0.0, 0.0); 5];
    for (slot, w) in out.iter_mut().zip([CHECK_WIDTH, key, value, description, DELETE_WIDTH]) {
        *slot = (x, x + w);
        x += w;
    }
    out
}

/// One borderless cell edit filling `rect`, vertically centred. `id` keeps focus stable when
/// the trailing row turns into a real row (same index, same id).
fn cell_edit(ui: &mut Ui, id: egui::Id, rect: egui::Rect, text: &mut String, hint: &str, password: bool) -> Response {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let mut edit = egui::TextEdit::singleline(text)
        .id(id)
        .hint_text(hint)
        .font(egui::TextStyle::Body)
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(8, 4)))
        .vertical_align(egui::Align::Center)
        .min_size(rect.size())
        .desired_width(rect.width());
    if password {
        edit = edit.password(true);
    }
    child.add(edit)
}

/// A Postman-style grid bound to `rows`: a header (Key / Value / Description) and one editable
/// row per entry, followed by an always-present empty row. Typing into the empty row turns it
/// into a real row and a fresh empty row appears below it. `variant` picks the value cell;
/// many rows scroll virtualised.
pub fn key_value_table(ui: &mut Ui, rows: &mut Vec<KeyValue>, variant: TableVariant) -> TableResponse {
    let mut result = TableResponse::default();
    let theme = crate::theme::Theme::current(ui.ctx());
    let palette = theme.palette;
    let base_id = ui.next_auto_id();
    ui.skip_ahead_auto_ids(1);
    let total = rows.len().saturating_add(1);
    let max_height = 360.0;
    let content_height = (total as f32) * ROW_HEIGHT;
    let mut remove_index: Option<usize> = None;
    let mut materialized: Option<KeyValue> = None;
    let line = egui::Stroke::new(1.0, palette.border);

    let mut render_rows = |ui: &mut Ui, range: std::ops::Range<usize>| {
        for index in range {
            let (row_rect, row_response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), egui::Sense::hover());
            let cols = columns(row_rect.left(), row_rect.width());
            let cell =
                |column: usize| egui::Rect::from_min_max(egui::pos2(cols[column].0, row_rect.top()), egui::pos2(cols[column].1, row_rect.bottom()));
            let is_ghost = index >= rows.len();
            let mut ghost = placeholder_row();
            let row = if is_ghost { &mut ghost } else { &mut rows[index] };
            let id = |column: usize| base_id.with((index, column));
            let mut changed = false;

            if !is_ghost {
                let mut check = ui.new_child(egui::UiBuilder::new().max_rect(cell(0)).layout(egui::Layout::left_to_right(egui::Align::Center)));
                check.add_space(((CHECK_WIDTH - 16.0) / 2.0).max(0.0));
                changed |= compact_checkbox(&mut check, &mut row.enabled).changed();
            }
            changed |= cell_edit(ui, id(1), cell(1), &mut row.key, "Key", false).changed();
            let value_rect = cell(2);
            let (value_rect, kind_changed) = if variant == TableVariant::FileText {
                let combo_rect = egui::Rect::from_min_size(value_rect.min + egui::vec2(4.0, 0.0), egui::vec2(64.0, ROW_HEIGHT));
                let mut combo_ui = ui.new_child(egui::UiBuilder::new().max_rect(combo_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
                let mut kind = row.kind;
                egui::ComboBox::from_id_salt(("kv-kind", base_id, index))
                    .width(52.0)
                    .selected_text(match kind {
                        KeyValueKind::Text => "Text",
                        KeyValueKind::File => "File",
                    })
                    .show_ui(&mut combo_ui, |ui| {
                        ui.selectable_value(&mut kind, KeyValueKind::Text, "Text");
                        ui.selectable_value(&mut kind, KeyValueKind::File, "File");
                    });
                let changed_kind = kind != row.kind;
                row.kind = kind;
                (egui::Rect::from_min_max(egui::pos2(combo_rect.right(), value_rect.top()), value_rect.max), changed_kind)
            } else {
                (value_rect, false)
            };
            changed |= kind_changed && !is_ghost;
            let (hint, secret) = match variant {
                TableVariant::FileText => ("Value or path", false),
                TableVariant::Secret => ("Secret value", true),
                TableVariant::Normal => ("Value", false),
            };
            changed |= cell_edit(ui, id(3), value_rect, &mut row.value, hint, secret).changed();
            changed |= cell_edit(ui, id(4), cell(3), &mut row.description, "Description", false).changed();

            if is_ghost {
                if !row.key.is_empty() || !row.value.is_empty() || !row.description.is_empty() {
                    materialized = Some(std::mem::take(row));
                }
            } else {
                result.changed |= changed;
                let delete_rect = cell(4);
                let delete = ui.interact(delete_rect, base_id.with((index, "delete")), egui::Sense::click());
                let hot = row_response.hovered() || delete.hovered() || ui.rect_contains_pointer(row_rect);
                if hot {
                    let pill = egui::Rect::from_center_size(delete_rect.center(), egui::vec2(24.0, 24.0));
                    if delete.hovered() {
                        ui.painter().rect_filled(pill, egui::CornerRadius::same(theme.metrics.radius as u8), palette.hover);
                    }
                    crate::icons::paint(ui.painter(), pill.shrink(6.0), "x", if delete.hovered() { palette.text } else { palette.text_muted }, 1.5);
                }
                delete.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "x"));
                if delete.clicked() {
                    remove_index = Some(index);
                }
            }
            // Grid lines: row bottom and the column dividers.
            ui.painter().hline(row_rect.x_range(), row_rect.bottom() - 0.5, line);
            for column in [1usize, 2, 3] {
                ui.painter().vline(cols[column].0, row_rect.y_range(), line);
            }
        }
    };

    egui::Frame::new()
        .fill(palette.input_bg)
        .stroke(line)
        .corner_radius(egui::CornerRadius::same(theme.metrics.radius as u8))
        .inner_margin(egui::Margin::ZERO)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            // Header.
            let (header, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), HEADER_HEIGHT), egui::Sense::hover());
            let cols = columns(header.left(), header.width());
            ui.painter().rect_filled(
                header,
                egui::CornerRadius { nw: theme.metrics.radius as u8, ne: theme.metrics.radius as u8, sw: 0, se: 0 },
                palette.surface_alt,
            );
            for (column, label) in [(1usize, "Key"), (2, "Value"), (3, "Description")] {
                ui.painter().text(
                    egui::pos2(cols[column].0 + 8.0, header.center().y),
                    egui::Align2::LEFT_CENTER,
                    label,
                    theme.semibold_font(12.0),
                    palette.text_muted,
                );
            }
            ui.painter().hline(header.x_range(), header.bottom() - 0.5, line);
            for column in [1usize, 2, 3] {
                ui.painter().vline(cols[column].0, header.y_range(), line);
            }
            if content_height > max_height {
                egui::ScrollArea::vertical().max_height(max_height).id_salt((base_id, "scroll")).show_rows(ui, ROW_HEIGHT, total, |ui, range| {
                    render_rows(ui, range);
                });
            } else {
                render_rows(ui, 0..total);
            }
        });
    if let Some(index) = remove_index
        && index < rows.len()
    {
        rows.remove(index);
        result.removed = true;
        result.changed = true;
    }
    if let Some(row) = materialized {
        rows.push(row);
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
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_space(24.0);
            ui.label(egui::RichText::new(name).color(theme.palette.text_muted).strong());
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
