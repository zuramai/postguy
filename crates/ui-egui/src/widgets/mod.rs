//! Custom widgets: square checkbox, outlined radio, toggle switch, select with
//! groups and checks, split button, searchable popup, context menu with icons and shortcut
//! hints, modal and typed-confirm dialogs, inline rename. egui's stock checkbox/radio draw
//! poorly on light themes, so every control here paints with [`Theme`] tokens and reports
//! itself to AccessKit (kittest and real screen readers use the same labels).

pub mod editor;
pub mod field;
pub mod table;
pub mod timeline;

pub use editor::{CodeEditor, CodeLanguage, LargeTextView};
pub use field::{variable_field, variable_field_frameless, variable_field_with, variable_spans};
pub use table::{TableResponse, TableVariant, VariableGrid, automatic_rows, key_value_table, variable_table};
pub use timeline::{MessageDirection, Timeline, TimelineEntry};

use egui::{
    Align2, Color32, Context, CursorIcon, Id, Key, Popup, Rect, Response, RichText, Sense, Stroke, StrokeKind, TextEdit, TextStyle, Ui, UiKind, Vec2,
    WidgetInfo, WidgetType, pos2, vec2,
};

use crate::icons;

/// Vertical rhythm of one control row (checkbox/radio/switch).
const CONTROL_ROW: f32 = 28.0;
const BOX: f32 = 16.0;
/// Height of a [`sub_tab`].
pub const SUB_TAB_HEIGHT: f32 = 30.0;

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
    // A labelled checkbox hugs its content so it can sit in a row beside other controls; the
    // box-only variant takes a fixed 16 px slot for dense tables.
    let width = if fill_width {
        BOX + 10.0
            + ui.fonts_mut(|fonts| fonts.layout_no_wrap(label.to_owned(), TextStyle::Body.resolve(ui.style()), theme.palette.text).size().x)
            + 4.0
    } else {
        BOX + 6.0
    };
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
    ui.painter().rect(box_rect, egui::CornerRadius::same(4), fill, Stroke::new(1.0, border), StrokeKind::Inside);
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
    let track = Rect::from_min_size(pos2(rect.left(), rect.center().y - 9.0), vec2(34.0, 18.0));
    let track_fill = if *on { theme.palette.accent } else { theme.palette.surface_sunken };
    ui.painter().rect(track, egui::CornerRadius::same(9), track_fill, Stroke::new(1.0, theme.palette.border), StrokeKind::Inside);
    let knob_x = if *on { track.right() - 9.0 } else { track.left() + 9.0 };
    ui.painter().circle_filled(pos2(knob_x, track.center().y), 7.0, contrast_on(track_fill));
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

/// A group with an optional muted header (grouped headers and separators).
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
    select_colored(ui, id, selected, groups, placeholder, None)
}

/// [`select`] with a custom trigger text colour (the raw-language select uses the accent).
pub fn select_colored(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    selected: &mut Option<String>,
    groups: &[SelectGroup],
    placeholder: &str,
    text_color: Option<Color32>,
) -> bool {
    let theme = crate::theme::Theme::current(ui.ctx());
    let display = selected
        .as_ref()
        .and_then(|value| groups.iter().flat_map(|group| group.items.iter()).find(|item| item.value == *value).map(|item| item.label.clone()))
        .unwrap_or_else(|| placeholder.to_string());
    let response = chevron_trigger(ui, &display, 100.0, text_color);
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
                let row = popup_row(ui, &item.label, is_selected, item.enabled);
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
    let palette = theme.palette;
    let mut clicked = false;
    let height = theme.metrics.url_height;
    let radius = theme.metrics.radius as u8;
    let label_width = ui.painter().layout_no_wrap(main_label.to_owned(), TextStyle::Button.resolve(ui.style()), palette.on_accent).size().x;
    let total_width = (label_width + 28.0).max(76.0) + 30.0;
    // Explicit direction: `ui.horizontal` would mirror inside a right-to-left parent.
    ui.allocate_ui_with_layout(vec2(total_width, height), egui::Layout::left_to_right(egui::Align::Min), |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let font = TextStyle::Button.resolve(ui.style());
        let galley = ui.painter().layout_no_wrap(main_label.to_owned(), font, palette.on_accent);
        let main_width = (galley.size().x + 28.0).max(76.0);
        let (main_rect, main) = ui.allocate_exact_size(vec2(main_width, height), Sense::click());
        let (caret_rect, caret) = ui.allocate_exact_size(vec2(30.0, height), Sense::click());
        let main_fill = if main.hovered() { palette.accent_fill_hover } else { palette.accent_fill };
        let caret_fill = if caret.hovered() { palette.accent_fill_hover } else { palette.accent_fill };
        ui.painter().rect_filled(main_rect, egui::CornerRadius { nw: radius, sw: radius, ne: 0, se: 0 }, main_fill);
        ui.painter().rect_filled(caret_rect, egui::CornerRadius { nw: 0, sw: 0, ne: radius, se: radius }, caret_fill);
        ui.painter().galley(main_rect.center() - galley.size() / 2.0, galley, palette.on_accent);
        ui.painter().line_segment(
            [pos2(caret_rect.left(), caret_rect.top() + 7.0), pos2(caret_rect.left(), caret_rect.bottom() - 7.0)],
            Stroke::new(1.0, palette.on_accent.gamma_multiply(0.35)),
        );
        icons::paint(ui.painter(), Rect::from_center_size(caret_rect.center(), Vec2::splat(14.0)), "chevron-down", palette.on_accent, 1.5);
        if main.hovered() || caret.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        main.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, main_label));
        caret.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Send options"));
        if main.clicked() {
            on_main();
            clicked = true;
        }
        Popup::menu(&caret).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
            ui.set_min_width(180.0);
            menu(ui);
        });
    });
    clicked
}

/// The select-style trigger: `label` followed by a painted chevron. Height 26, radius 6,
/// `button_bg` fill. `text_color` overrides the label colour.
pub(crate) fn chevron_trigger(ui: &mut Ui, label: &str, min_width: f32, text_color: Option<Color32>) -> Response {
    let theme = crate::theme::Theme::current(ui.ctx());
    let palette = theme.palette;
    let font = TextStyle::Button.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let width = (galley.size().x + 12.0 + 6.0 + 12.0 + 10.0).max(min_width);
    let (rect, response) = ui.allocate_exact_size(vec2(width, 26.0), Sense::click());
    let fill = if response.hovered() { palette.button_hover } else { palette.button_bg };
    ui.painter().rect_filled(rect, egui::CornerRadius::same(6), fill);
    let text = text_color.unwrap_or(palette.text);
    ui.painter().galley(pos2(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0), galley, text);
    icons::paint(
        ui.painter(),
        Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), Vec2::splat(12.0)),
        "chevron-down",
        palette.text_muted,
        1.5,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// One popup list row: height 28, hover fill, a check icon when selected.
pub(crate) fn popup_row(ui: &mut Ui, label: &str, selected: bool, enabled: bool) -> Response {
    let theme = crate::theme::Theme::current(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), if enabled { Sense::click() } else { Sense::hover() });
    if enabled && response.hovered() {
        ui.painter().rect_filled(rect, egui::CornerRadius::same(theme.metrics.radius as u8), theme.palette.hover);
    }
    let color = if enabled { theme.palette.text } else { theme.palette.text_muted };
    if selected {
        icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), Vec2::splat(13.0)), "check", color, 1.6);
    }
    let galley = ellipsized_galley(ui, label, TextStyle::Body.resolve(ui.style()), color, (rect.width() - 38.0).max(10.0));
    ui.painter().galley(pos2(rect.left() + 28.0, rect.center().y - galley.size().y / 2.0), galley, color);
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, selected, label));
    response
}

/// `text` laid out on one line no wider than `max_width`, ending in `…` when it had to be cut.
pub(crate) fn ellipsized_galley(ui: &Ui, text: &str, font: egui::FontId, color: Color32, max_width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap.max_width = max_width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

/// A searchable popup list (one line per item). Returns true when `selected` changed.
pub fn searchable_popup(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    button_label: &str,
    items: &[String],
    selected: &mut Option<String>,
) -> bool {
    let base_id = Id::new(id);
    let response = chevron_trigger(ui, button_label, 0.0, None);
    let mut filter = ui.data(|data| data.get_temp::<String>(Id::new((base_id, "filter")))).unwrap_or_default();
    let mut changed = false;
    Popup::menu(&response).id(Id::new((base_id, "popup"))).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(240.0);
        let search = ui.add(TextEdit::singleline(&mut filter).hint_text("Search…"));
        if search.changed() {
            ui.data_mut(|data| data.insert_temp(Id::new((base_id, "filter")), filter.clone()));
        }
        ui.separator();
        egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
            let needle = filter.to_ascii_lowercase();
            for item in items.iter().filter(|item| needle.is_empty() || item.to_ascii_lowercase().contains(&needle)) {
                let is_selected = selected.as_deref() == Some(item.as_str());
                if popup_row(ui, item, is_selected, true).clicked() {
                    *selected = Some(item.clone());
                    changed = true;
                    ui.close();
                }
            }
        });
    });
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
    let height = 28.0;
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && (response.hovered() || response.has_focus());
    if hovered {
        ui.painter().rect_filled(rect, egui::CornerRadius::same(theme.metrics.radius as u8), theme.palette.hover);
    }
    let color = if enabled { theme.palette.text } else { theme.palette.text_muted };
    let mut x = rect.left() + 10.0;
    if let Some(icon_name) = icon {
        let icon_rect = Rect::from_center_size(pos2(x + 7.5, rect.center().y), Vec2::splat(15.0));
        icons::paint(ui.painter(), icon_rect, icon_name, color, 1.5);
        x += 24.0;
    }
    ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, TextStyle::Body.resolve(ui.style()), color);
    if let Some(shortcut) = shortcut {
        ui.painter().text(
            pos2(rect.right() - 10.0, rect.center().y),
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

/// A rounded pill for a short label: HTTP status, method tag, protocol or count chip. `fg` is
/// the text colour; the caller supplies the (usually tinted) `bg`.
pub fn badge(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) -> Response {
    let font = TextStyle::Small.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), fg);
    let padding = Vec2::new(16.0, 6.0);
    let size = galley.size() + padding;
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, egui::CornerRadius::same(6), bg);
    ui.painter().galley(rect.min + padding * 0.5, galley, fg);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), text));
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
                if ui.add(egui::Button::new("x").frame(false)).clicked() {
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

/// A destructive-action confirmation that requires typing the exact name. The
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
        let response = ui.add(egui::Label::new(text.as_str()).sense(Sense::click())).on_hover_cursor(CursorIcon::PointingHand);
        if response.double_clicked() {
            state.editing = true;
            state.buffer = text.clone();
            ui.memory_mut(|memory| memory.request_focus(Id::new((id, "rename"))));
        }
        return RenameAction::Idle;
    }
    let edit = ui.add(TextEdit::singleline(&mut state.buffer).id(Id::new((id, "rename"))).hint_text("Rename").desired_width(180.0));
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
    icon_button_with(ui, icon_name, tooltip, tooltip)
}

/// An icon button whose accessible `label` differs from the hover `tooltip`.
pub fn icon_button_with(ui: &mut Ui, icon_name: &str, label: &str, tooltip: &str) -> Response {
    icon_button_impl(ui, icon_name, label, tooltip, false)
}

/// An icon button that stays highlighted while `on` and flips it on click.
pub fn icon_toggle(ui: &mut Ui, icon_name: &str, label: &str, tooltip: &str, on: &mut bool) -> Response {
    let mut response = icon_button_impl(ui, icon_name, label, tooltip, *on);
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let enabled = ui.is_enabled();
    let checked = *on;
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, checked, label));
    response
}

fn icon_button_impl(ui: &mut Ui, icon_name: &str, label: &str, tooltip: &str, on: bool) -> Response {
    let theme = crate::theme::Theme::current(ui.ctx());
    let enabled = ui.is_enabled();
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(theme.metrics.control_height), if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && response.hovered();
    let rounding = egui::CornerRadius::same(theme.metrics.radius as u8);
    if on {
        ui.painter().rect_filled(rect, rounding, theme.palette.selected);
    } else if hovered {
        ui.painter().rect_filled(rect, rounding, theme.palette.hover);
    }
    let mut color = if hovered || on { theme.palette.text } else { theme.palette.text_muted };
    if !enabled {
        color = color.gamma_multiply(0.5);
    }
    icons::paint(ui.painter(), rect.shrink(7.0), icon_name, color, 1.5);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    response.on_hover_text(tooltip).on_hover_cursor(CursorIcon::PointingHand)
}

/// Look of a [`text_button`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    /// Destructive secondary action (Cancel): tinted danger fill and text.
    Danger,
}

/// A compact text button with an optional leading icon. The accessible label is `label`.
pub fn text_button(ui: &mut Ui, kind: ButtonKind, icon_name: Option<&str>, label: &str, enabled: bool) -> Response {
    text_button_with(ui, kind, icon_name, label, enabled, ButtonOptions::default())
}

/// Tweaks for [`text_button_with`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ButtonOptions {
    /// Height override (default: the theme's control height).
    pub height: Option<f32>,
    /// Use the small text style (bottom bar).
    pub small: bool,
    /// Paint the `selected` fill (a toggled panel).
    pub selected: bool,
    /// Text/icon colour override for the resting state.
    pub color: Option<Color32>,
}

/// [`text_button`] with [`ButtonOptions`].
pub fn text_button_with(ui: &mut Ui, kind: ButtonKind, icon_name: Option<&str>, label: &str, enabled: bool, options: ButtonOptions) -> Response {
    let theme = crate::theme::Theme::current(ui.ctx());
    let font = if options.small { TextStyle::Small } else { TextStyle::Button }.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let width = galley.size().x + 20.0 + if icon_name.is_some() { 20.0 } else { 0.0 };
    let height = options.height.unwrap_or(theme.metrics.control_height);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && response.hovered();
    let palette = theme.palette;
    let rounding = egui::CornerRadius::same(theme.metrics.radius as u8);
    let (fill, stroke, text) = match kind {
        ButtonKind::Primary => (if hovered { palette.accent_fill_hover } else { palette.accent_fill }, Stroke::NONE, palette.on_accent),
        ButtonKind::Secondary => {
            (if hovered { palette.button_hover } else { palette.button_bg }, Stroke::new(1.0, palette.border), options.color.unwrap_or(palette.text))
        }
        ButtonKind::Danger => (palette.danger.gamma_multiply(if hovered { 0.2 } else { 0.12 }), Stroke::NONE, palette.danger),
        ButtonKind::Ghost => {
            let fill = if hovered {
                palette.hover
            } else if options.selected {
                palette.selected
            } else {
                Color32::TRANSPARENT
            };
            let text = if hovered || options.selected { palette.text } else { options.color.unwrap_or(palette.text_muted) };
            (fill, Stroke::NONE, text)
        }
    };
    let text = if enabled { text } else { palette.text_muted.gamma_multiply(0.6) };
    let fill = if !enabled && kind != ButtonKind::Ghost { palette.button_bg } else { fill };
    ui.painter().rect(rect, rounding, fill, stroke, StrokeKind::Inside);
    let mut x = rect.left() + 10.0;
    if let Some(name) = icon_name {
        let icon_rect = Rect::from_center_size(pos2(rect.left() + 8.0 + 7.0, rect.center().y), Vec2::splat(14.0));
        icons::paint(ui.painter(), icon_rect, name, text, 1.5);
        x += 20.0;
    }
    ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, text);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    if enabled { response.on_hover_cursor(CursorIcon::PointingHand) } else { response }
}

/// A pill-style sub tab (request builder and response viewer): selected tabs fill with
/// `selected`; `count` and `dot` annotate the label. The accessible label is `label`.
pub fn sub_tab(ui: &mut Ui, label: &str, selected: bool, count: Option<usize>, dot: bool) -> Response {
    sub_tab_sized(ui, label, selected, count, dot, SUB_TAB_HEIGHT)
}

/// [`sub_tab`] with an explicit height (the body toolbar uses a shorter one).
pub fn sub_tab_sized(ui: &mut Ui, label: &str, selected: bool, count: Option<usize>, dot: bool, height: f32) -> Response {
    let theme = crate::theme::Theme::current(ui.ctx());
    let palette = theme.palette;
    let font = TextStyle::Body.resolve(ui.style());
    let small = TextStyle::Small.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let count_galley = count.filter(|count| *count > 0).map(|count| ui.painter().layout_no_wrap(count.to_string(), small, Color32::PLACEHOLDER));
    let mut width = galley.size().x + 20.0;
    if let Some(count) = &count_galley {
        width += count.size().x + 5.0;
    }
    if dot {
        width += 11.0;
    }
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let hovered = response.hovered();
    let rounding = egui::CornerRadius::same(theme.metrics.radius as u8);
    if selected {
        ui.painter().rect_filled(rect, rounding, palette.selected);
    } else if hovered {
        ui.painter().rect_filled(rect, rounding, palette.hover);
    }
    let text = if selected { palette.text } else { palette.text_muted };
    let mut x = rect.left() + 10.0;
    let label_width = galley.size().x;
    ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, text);
    x += label_width;
    if let Some(count) = count_galley {
        x += 5.0;
        let count_width = count.size().x;
        ui.painter().galley(pos2(x, rect.center().y - count.size().y / 2.0), count, palette.text_muted);
        x += count_width;
    }
    if dot {
        ui.painter().circle_filled(pos2(x + 8.0, rect.center().y), 3.0, palette.success);
    }
    let accessible = match count.filter(|count| *count > 0) {
        Some(count) => format!("{label} {count}"),
        None => label.to_string(),
    };
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), selected, &accessible));
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// Readable text on a filled colour (white on accent/danger, dark on light fills).
pub(crate) fn contrast_on(background: Color32) -> Color32 {
    let luma = 0.299 * f32::from(background.r()) + 0.587 * f32::from(background.g()) + 0.114 * f32::from(background.b());
    if luma > 150.0 { Color32::from_rgb(0x1F, 0x23, 0x28) } else { Color32::WHITE }
}
