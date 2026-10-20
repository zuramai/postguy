//! Custom widgets (PROMPT.md 6.3): square checkbox, outlined radio, toggle switch, select with
//! groups and checks, split button, searchable popup, context menu with icons and shortcut
//! hints, modal and typed-confirm dialogs, inline rename. egui's stock checkbox/radio draw
//! poorly on light themes, so every control here paints with [`Theme`] tokens and reports
//! itself to AccessKit (kittest and real screen readers use the same labels).

pub mod editor;
pub mod field;
pub mod table;
pub mod timeline;

pub use editor::{CodeEditor, CodeLanguage, LargeTextView};
pub use field::{variable_field, variable_spans};
pub use table::{TableResponse, TableVariant, automatic_rows, key_value_table};
pub use timeline::{MessageDirection, Timeline, TimelineEntry};

use egui::{
    Align2, Color32, Context, CursorIcon, Id, Key, Popup, Rect, Response, RichText, Sense, Stroke, StrokeKind, TextEdit, TextStyle, Ui, UiKind, Vec2,
    WidgetInfo, WidgetType, pos2, vec2,
};

use crate::icons;

/// Vertical rhythm of one control row (checkbox/radio/switch).
const CONTROL_ROW: f32 = 22.0;
const BOX: f32 = 16.0;

/// A square checkbox: an outlined square that fills with the accent and draws a check when on.
/// The whole row is clickable; Space toggles when focused.
pub fn checkbox(ui: &mut Ui, checked: &mut bool, label: &str) -> Response {
    checkbox_impl(ui, checked, label, true)
}

/// The square checkbox box only (no label, 16 px slot), for dense tables.
pub fn compact_checkbox(ui: &mut Ui, checked: &mut bool) -> Response {
    checkbox_impl(ui, checked, "", false)
}

fn checkbox_impl(ui: &mut Ui, checked: &mut bool, label: &str, fill_width: bool) -> Response {
    let theme = crate::theme::Theme::current(ui.ctx());
    let width = if fill_width { ui.available_width() } else { BOX + 6.0 };
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, CONTROL_ROW), Sense::click());
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    if response.has_focus() && ui.input(|input| input.key_pressed(Key::Space)) {
        *checked = !*checked;
        response.mark_changed();
    }
    let box_rect = Rect::from_min_size(pos2(rect.left(), rect.center().y - BOX / 2.0), Vec2::splat(BOX));
    let hovered = response.hovered() || response.has_focus();
    let border = if hovered { theme.palette.accent } else { theme.palette.border };
    let fill = if *checked { theme.palette.accent } else { theme.palette.surface };
    ui.painter().rect(box_rect, theme.metrics.radius - 2.0, fill, Stroke::new(1.0, border), StrokeKind::Inside);
    if *checked {
        let stroke = Stroke::new(2.0, contrast_on(theme.palette.accent));
        let painter = ui.painter();
        painter.line_segment([pos2(box_rect.left() + 4.0, box_rect.center().y), pos2(box_rect.left() + 7.0, box_rect.bottom() - 4.0)], stroke);
        painter.line_segment([pos2(box_rect.left() + 7.0, box_rect.bottom() - 4.0), pos2(box_rect.right() - 3.5, box_rect.top() + 4.5)], stroke);
    }
    ui.painter().text(
        pos2(box_rect.right() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        TextStyle::Body.resolve(ui.style()),
        theme.palette.text,
    );
    if label.is_empty() {
        response.clone().on_hover_text("Enable");
    }
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *checked, label));
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// An outlined radio: a ring with an accent dot when selected. Clicking any radio in the group
/// sets the shared value through the caller's `value`.
pub fn radio(ui: &mut Ui, selected: &mut String, value: &str, label: &str) -> Response {
    let theme = crate::theme::Theme::current(ui.ctx());
    let (rect, mut response) = ui.allocate_exact_size(vec2(ui.available_width(), CONTROL_ROW), Sense::click());
    let checked = selected == value;
    if response.clicked() && !checked {
        *selected = value.to_string();
        response.mark_changed();
    }
    let circle = Rect::from_center_size(pos2(rect.left() + BOX / 2.0, rect.center().y), Vec2::splat(BOX));
    let border = if response.hovered() || response.has_focus() { theme.palette.accent } else { theme.palette.border };
    ui.painter().circle_stroke(circle.center(), BOX / 2.0 - 0.5, Stroke::new(1.0, border));
    if checked {
        ui.painter().circle_filled(circle.center(), BOX / 2.0 - 4.0, theme.palette.accent);
    }
    ui.painter().text(
        pos2(circle.right() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        TextStyle::Body.resolve(ui.style()),
        theme.palette.text,
    );
    response.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, ui.is_enabled(), checked, label));
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// A toggle switch (on = accent track, knob right). AccessKit reports it as a checkbox.
pub fn switch(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let theme = crate::theme::Theme::current(ui.ctx());
    let (rect, mut response) = ui.allocate_exact_size(vec2(ui.available_width(), CONTROL_ROW), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if response.has_focus() && ui.input(|input| input.key_pressed(Key::Space)) {
        *on = !*on;
        response.mark_changed();
    }
    let track = Rect::from_min_size(pos2(rect.left(), rect.center().y - 8.0), vec2(30.0, 16.0));
    let track_fill = if *on { theme.palette.accent } else { theme.palette.surface_sunken };
    ui.painter().rect(track, 8.0, track_fill, Stroke::new(1.0, theme.palette.border), StrokeKind::Inside);
    let knob_x = if *on { track.right() - 8.0 } else { track.left() + 8.0 };
    ui.painter().circle_filled(pos2(knob_x, track.center().y), 6.0, contrast_on(track_fill));
    ui.painter().text(
        pos2(track.right() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        TextStyle::Body.resolve(ui.style()),
        theme.palette.text,
    );
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *on, label));
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// One entry of a grouped select.
#[derive(Debug, Clone)]
pub struct SelectItem {
    pub value: String,
    pub label: String,
    pub enabled: bool,
    /// Draw a separator above this item.
    pub separator_before: bool,
}

impl SelectItem {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> SelectItem {
        SelectItem { value: value.into(), label: label.into(), enabled: true, separator_before: false }
    }
}

/// A group with an optional muted header (PROMPT.md 6.3: grouped headers and separators).
#[derive(Debug, Clone, Default)]
pub struct SelectGroup {
    pub label: Option<String>,
    pub items: Vec<SelectItem>,
}

/// A select button with check marks, group headers, separators and disabled rows. Returns true
/// when the selection changed.
pub fn select(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    selected: &mut Option<String>,
    groups: &[SelectGroup],
    placeholder: &str,
) -> bool {
    let theme = crate::theme::Theme::current(ui.ctx());
    let display = selected
        .as_ref()
        .and_then(|value| groups.iter().flat_map(|group| group.items.iter()).find(|item| item.value == *value).map(|item| item.label.clone()))
        .unwrap_or_else(|| placeholder.to_string());
    let response = ui
        .add(egui::Button::new(RichText::new(format!("{display}  ▾")).color(theme.palette.text)).min_size(vec2(120.0, theme.metrics.control_height)));
    let mut changed = false;
    Popup::menu(&response).id(Id::new((id, "select"))).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
        ui.set_min_width(response.rect.width().max(180.0));
        for group in groups {
            if let Some(label) = &group.label {
                ui.label(RichText::new(label.to_ascii_uppercase()).small().color(theme.palette.text_muted));
            }
            for item in &group.items {
                if item.separator_before {
                    ui.separator();
                }
                let is_selected = selected.as_deref() == Some(item.value.as_str());
                let label = if is_selected { format!("✓  {}", item.label) } else { format!("    {}", item.label) };
                let row = ui.add_enabled(
                    item.enabled,
                    egui::Button::new(label).selected(is_selected).frame(false).min_size(vec2(ui.available_width(), 24.0)),
                );
                if row.clicked() && !is_selected {
                    *selected = Some(item.value.clone());
                    changed = true;
                    ui.close();
                }
            }
        }
    });
    changed
}

/// A primary action with a caret that opens more choices (`Send` / `Send and Download`).
pub fn split_button(ui: &mut Ui, main_label: &str, on_main: impl FnOnce(), menu: impl FnOnce(&mut Ui)) -> bool {
    let theme = crate::theme::Theme::current(ui.ctx());
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let main = ui.add(
            egui::Button::new(RichText::new(main_label).color(contrast_on(theme.palette.accent)).strong())
                .fill(theme.palette.accent)
                .corner_radius(egui::CornerRadius { nw: theme.metrics.radius as u8, sw: theme.metrics.radius as u8, ne: 0, se: 0 }),
        );
        if main.clicked() {
            on_main();
            clicked = true;
        }
        let caret = ui.add(
            egui::Button::new(RichText::new("▾").color(contrast_on(theme.palette.accent)))
                .fill(theme.palette.accent)
                .corner_radius(egui::CornerRadius { nw: 0, sw: 0, ne: theme.metrics.radius as u8, se: theme.metrics.radius as u8 }),
        );
        Popup::menu(&caret).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
            ui.set_min_width(180.0);
            menu(ui);
        });
    });
    clicked
}

/// A searchable popup list (one line per item). Returns true when `selected` changed.
pub fn searchable_popup(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    button_label: &str,
    items: &[String],
    selected: &mut Option<String>,
) -> bool {
    let theme = crate::theme::Theme::current(ui.ctx());
    let base_id = Id::new(id);
    let response = ui.add(egui::Button::new(format!("{button_label}  ▾")));
    let mut filter = ui.data(|data| data.get_temp::<String>(Id::new((base_id, "filter")))).unwrap_or_default();
    let mut changed = false;
    Popup::menu(&response).id(Id::new((base_id, "popup"))).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(220.0);
        let search = ui.add(TextEdit::singleline(&mut filter).hint_text("Search…"));
        if search.changed() {
            ui.data_mut(|data| data.insert_temp(Id::new((base_id, "filter")), filter.clone()));
        }
        ui.separator();
        egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
            let needle = filter.to_ascii_lowercase();
            for item in items.iter().filter(|item| needle.is_empty() || item.to_ascii_lowercase().contains(&needle)) {
                let is_selected = selected.as_deref() == Some(item.as_str());
                let label = if is_selected { format!("✓  {item}") } else { format!("    {item}") };
                if ui.add(egui::Button::new(label).selected(is_selected).frame(false).min_size(vec2(ui.available_width(), 24.0))).clicked() {
                    *selected = Some(item.clone());
                    changed = true;
                    ui.close();
                }
            }
        });
    });
    let _ = theme;
    changed
}

/// A context menu on any response (right click / long press). Items are built with
/// [`menu_item`] so icons and shortcut hints line up.
pub fn context_menu(response: &Response, contents: impl FnOnce(&mut Ui)) {
    response.context_menu(|ui| {
        ui.set_min_width(200.0);
        contents(ui);
    });
}

/// One menu row: optional leading icon, label, right-aligned shortcut hint. Closes the menu
/// when clicked. Returns the row response (`enabled` false rows do nothing).
pub fn menu_item(ui: &mut Ui, icon: Option<&str>, label: &str, shortcut: Option<&str>, enabled: bool) -> Response {
    let theme = crate::theme::Theme::current(ui.ctx());
    let height = 24.0;
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && (response.hovered() || response.has_focus());
    if hovered {
        ui.painter().rect_filled(rect, 4.0, theme.palette.surface_sunken);
    }
    let color = if enabled { theme.palette.text } else { theme.palette.text_muted };
    let mut x = rect.left() + 8.0;
    if let Some(icon_name) = icon {
        let icon_rect = Rect::from_center_size(pos2(x + 7.0, rect.center().y), Vec2::splat(14.0));
        icons::paint(ui.painter(), icon_rect, icon_name, color, 1.5);
        x += 22.0;
    }
    ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, TextStyle::Body.resolve(ui.style()), color);
    if let Some(shortcut) = shortcut {
        ui.painter().text(
            pos2(rect.right() - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            shortcut,
            TextStyle::Small.resolve(ui.style()),
            theme.palette.text_muted,
        );
    }
    if enabled && response.clicked() {
        ui.close_kind(UiKind::Menu);
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    response
}

/// A modal dialog. Returns the content's value while open, `None` when the backdrop or the
/// close button dismissed it (the caller clears its `open` flag).
pub fn modal<T>(
    ctx: &Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    title: &str,
    open: &mut bool,
    contents: impl FnOnce(&mut Ui) -> T,
) -> Option<T> {
    if !*open {
        return None;
    }
    let mut close = false;
    let modal = egui::Modal::new(Id::new((id, "modal"))).show(ctx, |ui| {
        ui.set_min_width(360.0);
        ui.set_max_width(560.0);
        ui.horizontal(|ui| {
            ui.heading(title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new("✕").frame(false)).clicked() {
                    close = true;
                }
            });
        });
        ui.separator();
        contents(ui)
    });
    if modal.backdrop_response.clicked() {
        close = true;
    }
    if close {
        *open = false;
        return None;
    }
    Some(modal.inner)
}

/// What a typed confirmation decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmOutcome {
    Pending,
    Confirmed,
    Cancelled,
}

/// A destructive-action confirmation that requires typing the exact name (PROMPT.md 6.3). The
/// destructive button stays disabled until the text matches; Escape cancels.
pub fn typed_confirm(
    ctx: &Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    open: &mut bool,
    title: &str,
    message: &str,
    confirm_word: &str,
    confirm_label: &str,
) -> ConfirmOutcome {
    if !*open {
        return ConfirmOutcome::Pending;
    }
    let theme = crate::theme::Theme::current(ctx);
    let mut typed = ctx.data(|data| data.get_temp::<String>(Id::new((&id, "typed"))).unwrap_or_default());
    let mut outcome = ConfirmOutcome::Pending;
    egui::Modal::new(Id::new((&id, "typed-modal"))).show(ctx, |ui| {
        ui.set_min_width(380.0);
        ui.heading(title);
        ui.separator();
        ui.label(message);
        ui.add_space(8.0);
        ui.label(RichText::new(format!("Type `{confirm_word}` to confirm:")).small().color(theme.palette.text_muted));
        let edit = ui.add(TextEdit::singleline(&mut typed).desired_width(f32::INFINITY));
        if ui.input(|input| input.key_pressed(Key::Enter)) && typed.trim() == confirm_word {
            outcome = ConfirmOutcome::Confirmed;
            let _ = edit;
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() || ui.input(|input| input.key_pressed(Key::Escape)) {
                outcome = ConfirmOutcome::Cancelled;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let matches = typed.trim() == confirm_word;
                let button = ui.add_enabled(
                    matches,
                    egui::Button::new(RichText::new(confirm_label).color(contrast_on(theme.palette.danger))).fill(theme.palette.danger),
                );
                if button.clicked() {
                    outcome = ConfirmOutcome::Confirmed;
                }
            });
        });
    });
    ctx.data_mut(|data| data.insert_temp(Id::new((&id, "typed")), typed));
    if outcome != ConfirmOutcome::Pending {
        *open = false;
        ctx.data_mut(|data| data.insert_temp(Id::new((&id, "typed")), String::new()));
    }
    outcome
}
/// Inline rename state, kept by the caller (a tree row, a tab, a header).
#[derive(Debug, Clone, Default)]
pub struct InlineRename {
    pub editing: bool,
    buffer: String,
}

impl InlineRename {
    /// Start editing from a menu command (the double-click path does this itself).
    pub fn begin(&mut self, current: &str) {
        self.editing = true;
        self.buffer = current.to_string();
    }
}

/// What an inline rename did this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameAction {
    Idle,
    Committed,
    Cancelled,
}

/// Double-click the label to edit in place; Enter commits, Escape cancels. While editing, the
/// caller should stop drawing conflicting interactions for that row.
pub fn inline_rename(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, text: &mut String, state: &mut InlineRename) -> RenameAction {
    if !state.editing {
        let response = ui.add(egui::Label::new(text.as_str()).sense(Sense::click()));
        if response.double_clicked() {
            state.editing = true;
            state.buffer = text.clone();
            ui.memory_mut(|memory| memory.request_focus(Id::new((id, "rename"))));
        }
        return RenameAction::Idle;
    }
    let edit = ui.add(TextEdit::singleline(&mut state.buffer).id(Id::new((id, "rename"))).desired_width(180.0));
    if ui.input(|input| input.key_pressed(Key::Enter)) {
        let trimmed = state.buffer.trim();
        if !trimmed.is_empty() {
            *text = trimmed.to_string();
            state.editing = false;
            return RenameAction::Committed;
        }
    }
    if ui.input(|input| input.key_pressed(Key::Escape)) {
        state.editing = false;
        return RenameAction::Cancelled;
    }
    if edit.lost_focus() && !ui.input(|input| input.key_pressed(Key::Enter)) {
        // Clicking elsewhere commits the typed name (the common rename gesture).
        let trimmed = state.buffer.trim();
        if !trimmed.is_empty() && trimmed != text {
            *text = trimmed.to_string();
            state.editing = false;
            return RenameAction::Committed;
        }
        state.editing = false;
        return RenameAction::Cancelled;
    }
    RenameAction::Idle
}

/// A borderless toolbar button showing a Lucide glyph; AccessKit gets the tooltip as its label.
pub fn icon_button(ui: &mut Ui, icon_name: &str, tooltip: &str) -> Response {
    let theme = crate::theme::Theme::current(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(theme.metrics.control_height), Sense::click());
    let color = if response.hovered() { theme.palette.accent } else { theme.palette.text_muted };
    if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, theme.palette.surface_sunken);
    }
    icons::paint(ui.painter(), rect.shrink(4.0), icon_name, color, 1.5);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), tooltip));
    response.on_hover_text(tooltip).on_hover_cursor(CursorIcon::PointingHand)
}

/// Readable text on a filled colour (white on accent/danger, dark on light fills).
pub(crate) fn contrast_on(background: Color32) -> Color32 {
    let luma = 0.299 * f32::from(background.r()) + 0.587 * f32::from(background.g()) + 0.114 * f32::from(background.b());
    if luma > 150.0 { Color32::from_rgb(0x1F, 0x23, 0x28) } else { Color32::WHITE }
}
