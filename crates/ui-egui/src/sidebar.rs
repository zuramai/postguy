//! The sidebar (PROMPT.md 5.1, 5.2, 6.2): filter, COLLECTIONS / ENVIRONMENTS / HISTORY sections,
//! a flattened tree with caret toggles, preview-open clicks, keyboard navigation, multi-select,
//! inline rename, typed delete confirmation and drag-and-drop (move, Cmd/Ctrl-drag copies).
//! The widget is data-in/actions-out: the app owns the engine and applies the returned
//! [`SidebarAction`]s, so the sidebar stays testable without a workspace.
//!
//! Row flattening is cached against `SidebarData::revision` and the filter text, so a 10 000-item
//! tree is walked only when the workspace or the filter changes; every frame paints the cached
//! row list and virtualises it with `ScrollArea::show_rows`.

use std::collections::{BTreeSet, HashMap};

use egui::{Align, Color32, Id, Key, Layout, RichText, ScrollArea, Sense, Stroke, Ui, vec2};
use postguy_model::{Collection, Environment, HistoryEntry, Id as ModelId, Item, RequestKind};

use crate::icons;
use crate::theme::Theme;
use crate::widgets::{self, InlineRename, RenameAction, context_menu, menu_item};

/// Which section a row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Collection,
    Folder,
    Request,
    Example,
    Environment,
    History,
}

impl RowKind {
    fn icon(self) -> &'static str {
        match self {
            RowKind::Collection | RowKind::Folder => "folder",
            RowKind::Request | RowKind::Example => "file-text",
            RowKind::History => "clock",
            RowKind::Environment => "settings",
        }
    }
}

/// A request protocol choice for "Add request".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestProtocol {
    Http,
    Grpc,
    WebSocket,
}

/// One flattened row, rebuilt only when the data revision or filter changes.
#[derive(Debug, Clone)]
pub struct SidebarRow {
    pub id: String,
    /// Parent folder/collection id (None for collections, environments and history).
    pub parent: Option<String>,
    pub depth: usize,
    pub kind: RowKind,
    pub label: String,
    /// URL for requests (filter matches it too).
    pub url: Option<String>,
    /// "GET", "POST", …, "gRPC", "WS" for requests; None otherwise.
    pub method: Option<String>,
    pub has_children: bool,
    pub expanded: bool,
    /// The row is the active environment.
    pub active: bool,
    /// Index among its parent's children (drag/drop).
    pub index: usize,
    pub collection: Option<String>,
    /// Ancestor folder/collection ids, for collapse filtering.
    pub ancestors: Vec<String>,
}

/// The data the sidebar paints. `revision` invalidates the cached flattening; the app bumps it
/// whenever the workspace changes.
#[derive(Debug, Clone, Default)]
pub struct SidebarData {
    pub revision: u64,
    pub collections: Vec<Collection>,
    pub environments: Vec<Environment>,
    pub history: Vec<HistoryEntry>,
    pub active_environment: Option<ModelId>,
}

/// An action the app must apply to the engine. The sidebar never mutates the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarAction {
    Open { id: String, kind: RowKind, preview: bool },
    NewRequest { collection: String, parent: Option<String>, protocol: RequestProtocol },
    NewFolder { collection: String, parent: Option<String> },
    NewCollection,
    NewEnvironment,
    Import,
    Rename { id: String, name: String },
    Duplicate { id: String, kind: RowKind },
    Delete { id: String, kind: RowKind },
    Move { id: String, parent: Option<String>, index: usize, copy: bool },
    Run { id: String },
    Export { id: String },
    SetActiveEnvironment(Option<String>),
    CopyAsCurl { id: String },
    OpenInNewTab { id: String },
    CreateExample { id: String },
    OpenHistory { id: String },
    DeleteHistory { id: String },
}

/// Stable sidebar state across frames (panels, selection, expansion, rename).
#[derive(Debug, Clone)]
pub struct SidebarState {
    pub filter: String,
    /// Folders/collections the user collapsed (everything starts expanded).
    pub collapsed: BTreeSet<String>,
    pub selected: BTreeSet<String>,
    pub anchor: Option<String>,
    pub focused: Option<String>,
    pub rename: Option<(String, InlineRename)>,
    /// The text being edited while a rename is active.
    pub rename_text: String,
    pub sections: [bool; 3],
    /// Typed confirmation for deleting a collection or folder.
    pub delete_confirm: Option<String>,
    drag: Option<String>,
    drop_target: Option<(String, usize)>,
    pub last_click_frame: u64,
}

impl Default for SidebarState {
    fn default() -> Self {
        SidebarState {
            filter: String::new(),
            collapsed: BTreeSet::new(),
            selected: BTreeSet::new(),
            anchor: None,
            focused: None,
            rename: None,
            rename_text: String::new(),
            sections: [true, true, true],
            delete_confirm: None,
            drag: None,
            drop_target: None,
            last_click_frame: 0,
        }
    }
}

impl SidebarState {
    pub fn is_expanded(&self, id: &str) -> bool {
        !self.collapsed.contains(id)
    }

    pub fn toggle_expanded(&mut self, id: &str) {
        if !self.collapsed.remove(id) {
            self.collapsed.insert(id.to_string());
        }
    }

    fn select(&mut self, row: &SidebarRow, range_rows: &[SidebarRow], modifiers: egui::Modifiers) {
        if modifiers.command {
            if !self.selected.remove(&row.id) {
                self.selected.insert(row.id.clone());
            }
        } else if modifiers.shift {
            let anchor = self.anchor.as_deref().and_then(|anchor| range_rows.iter().position(|candidate| candidate.id == anchor));
            let target = range_rows.iter().position(|candidate| candidate.id == row.id);
            if let (Some(anchor), Some(target)) = (anchor, target) {
                let (start, end) = if anchor <= target { (anchor, target) } else { (target, anchor) };
                self.selected = range_rows.get(start..=end).map(|rows| rows.iter().map(|row| row.id.clone()).collect()).unwrap_or_default();
            }
        } else {
            self.selected.clear();
            self.selected.insert(row.id.clone());
            self.anchor = Some(row.id.clone());
        }
        self.focused = Some(row.id.clone());
    }
}

/// The sidebar widget. Holds only the flattening cache; all interaction state lives in
/// [`SidebarState`].
#[derive(Debug, Default)]
pub struct Sidebar {
    cached_revision: u64,
    cached_filter: String,
    rows: Vec<SidebarRow>,
    /// True when the cache has been built at least once.
    built: bool,
}

impl Sidebar {
    pub fn new() -> Sidebar {
        Sidebar::default()
    }

    /// Build (or reuse) the flattened rows. Exposed for tests.
    pub fn rows(&mut self, data: &SidebarData, filter: &str) -> &[SidebarRow] {
        if !self.built || self.cached_revision != data.revision || self.cached_filter != filter {
            self.rows = flatten(data, filter);
            self.cached_revision = data.revision;
            self.cached_filter = filter.to_string();
            self.built = true;
        }
        &self.rows
    }

    /// Draw the sidebar sections and return at most one action (menus/keys/buttons).
    pub fn show(&mut self, ui: &mut Ui, state: &mut SidebarState, data: &SidebarData) -> Option<SidebarAction> {
        let theme = Theme::current(ui.ctx());
        let mut action: Option<SidebarAction> = None;

        self.toolbar(ui, state, &theme, &mut action);
        ui.add_space(2.0);
        let all_rows = self.rows(data, &state.filter).to_vec();
        let rows: Vec<SidebarRow> = all_rows
            .iter()
            .filter(|row| row.ancestors.iter().all(|ancestor| state.is_expanded(ancestor)))
            .cloned()
            .collect();

        // COLLECTIONS
        self.section_header(ui, state, 0, "COLLECTIONS", &theme);
        if state.sections[0] {
            let height = (ui.available_height() * 0.62).max(120.0);
            ScrollArea::vertical().id_salt("sidebar-collections").max_height(height).auto_shrink([false, false]).show_rows(
                ui,
                ROW_HEIGHT,
                rows.iter().filter(|row| matches!(row.kind, RowKind::Collection | RowKind::Folder | RowKind::Request | RowKind::Example)).count(),
                |ui, range| {
                    let section_rows: Vec<&SidebarRow> = rows
                        .iter()
                        .filter(|row| matches!(row.kind, RowKind::Collection | RowKind::Folder | RowKind::Request | RowKind::Example))
                        .collect();
                    self.paint_rows(ui, state, data, &rows, &section_rows, range, &theme, &mut action);
                },
            );
        }

        // ENVIRONMENTS
        self.section_header(ui, state, 1, "ENVIRONMENTS", &theme);
        if state.sections[1] {
            let env_rows: Vec<SidebarRow> = rows.iter().filter(|row| row.kind == RowKind::Environment).cloned().collect();
            ScrollArea::vertical().id_salt("sidebar-environments").max_height(140.0).auto_shrink([false, false]).show_rows(
                ui,
                ROW_HEIGHT,
                env_rows.len(),
                |ui, range| {
                    for index in range {
                        if let Some(row) = env_rows.get(index) {
                            self.paint_row(ui, state, data, &rows, row, &theme, &mut action);
                        }
                    }
                },
            );
        }

        // HISTORY
        self.section_header(ui, state, 2, "HISTORY", &theme);
        if state.sections[2] {
            let history_rows: Vec<SidebarRow> = rows.iter().filter(|row| row.kind == RowKind::History).cloned().collect();
            ScrollArea::vertical().id_salt("sidebar-history").max_height(160.0).auto_shrink([false, false]).show_rows(
                ui,
                ROW_HEIGHT,
                history_rows.len(),
                |ui, range| {
                    for index in range {
                        if let Some(row) = history_rows.get(index) {
                            self.paint_row(ui, state, data, &rows, row, &theme, &mut action);
                        }
                    }
                },
            );
        }

        // Typed confirmation for destructive deletes (PROMPT.md 5.1).
        if let Some(id) = state.delete_confirm.clone() {
            let label = rows.iter().find(|row| row.id == id).map(|row| row.label.clone()).unwrap_or_default();
            let mut open = true;
            let outcome = widgets::typed_confirm(
                ui.ctx(),
                ("sidebar-delete", &id),
                &mut open,
                "Delete item",
                &format!("Deleting `{label}` moves it to the trash."),
                "delete",
                "Delete",
            );
            match outcome {
                widgets::ConfirmOutcome::Confirmed => {
                    let kind = rows.iter().find(|row| row.id == id).map(|row| row.kind).unwrap_or(RowKind::Request);
                    state.delete_confirm = None;
                    action = Some(SidebarAction::Delete { id, kind });
                }
                widgets::ConfirmOutcome::Cancelled => state.delete_confirm = None,
                widgets::ConfirmOutcome::Pending => {}
            }
            if !open && state.delete_confirm.is_some() {
                state.delete_confirm = None;
            }
        }

        self.keyboard(ui, state, &rows, &mut action);
        action
    }

    fn toolbar(&mut self, ui: &mut Ui, state: &mut SidebarState, theme: &Theme, action: &mut Option<SidebarAction>) {
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut state.filter)
                    .hint_text("Filter…")
                    .desired_width((ui.available_width() - 62.0).max(80.0))
                    .frame(egui::Frame::NONE),
            );
            if state.filter.is_empty() {
                icons::icon(ui, "plus", theme.palette.text_muted);
                let add = ui.add(egui::Button::new("+").frame(false)).on_hover_text("New…");
                if add.clicked() {
                    *action = Some(SidebarAction::NewCollection);
                }
                let import = widgets::icon_button(ui, "arrow-right", "Import");
                if import.clicked() {
                    *action = Some(SidebarAction::Import);
                }
            }
            if response.changed() {
                state.selected.clear();
            }
        });
    }

    fn section_header(&self, ui: &mut Ui, state: &mut SidebarState, index: usize, label: &str, theme: &Theme) {
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
        if response.clicked() {
            state.sections[index] = !state.sections[index];
        }
        let caret = if state.sections[index] { "▾" } else { "▸" };
        ui.painter().text(
            rect.left_center() + vec2(6.0, 0.0),
            egui::Align2::LEFT_CENTER,
            caret,
            egui::TextStyle::Small.resolve(ui.style()),
            theme.palette.text_muted,
        );
        ui.painter().text(
            rect.left_center() + vec2(20.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::TextStyle::Small.resolve(ui.style()),
            theme.palette.text_muted,
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_rows(
        &mut self,
        ui: &mut Ui,
        state: &mut SidebarState,
        data: &SidebarData,
        all_rows: &[SidebarRow],
        section_rows: &[&SidebarRow],
        range: std::ops::Range<usize>,
        theme: &Theme,
        action: &mut Option<SidebarAction>,
    ) {
        for index in range {
            if let Some(row) = section_rows.get(index) {
                self.paint_row(ui, state, data, all_rows, row, theme, action);
            }
        }
    }

    fn paint_row(
        &mut self,
        ui: &mut Ui,
        state: &mut SidebarState,
        data: &SidebarData,
        all_rows: &[SidebarRow],
        row: &SidebarRow,
        theme: &Theme,
        action: &mut Option<SidebarAction>,
    ) {
        let indented = ui.available_width().max(220.0);
        let (rect, response) = ui.allocate_exact_size(vec2(indented, ROW_HEIGHT), Sense::click_and_drag());
        let hovered = response.hovered();
        let selected = state.selected.contains(&row.id);
        if selected {
            ui.painter().rect_filled(rect, 4.0, theme.palette.accent.gamma_multiply(0.16));
        } else if hovered {
            ui.painter().rect_filled(rect, 4.0, theme.palette.surface_alt);
        }
        let mut x = rect.left() + 4.0 + row.depth as f32 * 12.0;

        // Caret (collections, folders, requests with examples).
        if row.has_children {
            let caret_rect = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - 6.0), vec2(12.0, 12.0));
            let caret_response = ui.interact(caret_rect, Id::new(("sidebar-caret", &row.id)), Sense::click());
            let glyph = if row.expanded { "▾" } else { "▸" };
            ui.painter().text(caret_rect.center(), egui::Align2::CENTER_CENTER, glyph, egui::TextStyle::Small.resolve(ui.style()), theme.palette.text_muted);
            if caret_response.clicked() {
                state.toggle_expanded(&row.id);
                return;
            }
            x += 14.0;
        } else {
            x += 14.0;
        }

        if row.kind == RowKind::Request {
            let method = row.method.as_deref().unwrap_or("GET");
            ui.painter().text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, method, egui::TextStyle::Small.resolve(ui.style()), theme.method_color(method));
            x += 34.0;
        } else {
            let icon_rect = egui::Rect::from_center_size(egui::pos2(x + 6.0, rect.center().y), egui::Vec2::splat(13.0));
            icons::paint(ui.painter(), icon_rect, row.icon(), theme.palette.text_muted, 1.4);
            x += 18.0;
        }

        let label_color = if row.active { theme.palette.accent } else { theme.palette.text };
        let available_label = (rect.right() - x - 56.0).max(20.0);
        let clipped = truncate(&row.label, available_label, 6.5);
        ui.painter().text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, &clipped, egui::TextStyle::Body.resolve(ui.style()), label_color);
        if row.active {
            ui.painter().circle_filled(egui::pos2(rect.right() - 10.0, rect.center().y), 3.0, theme.palette.accent);
        }

        // Inline rename replaces the label; the widget writes `state.rename_text`, and a
        // commit becomes a Rename action the app applies through the engine.
        if let Some((rename_id, _)) = &state.rename
            && *rename_id == row.id
        {
            let Some((id, mut rename)) = state.rename.take() else { return };
            let mut text = std::mem::take(&mut state.rename_text);
            let edit_rect = egui::Rect::from_min_max(egui::pos2(x, rect.top() + 2.0), egui::pos2(rect.right() - 30.0, rect.bottom() - 2.0));
            let outcome = ui
                .scope_builder(egui::UiBuilder::new().max_rect(edit_rect), |ui| {
                    widgets::inline_rename(ui, ("sidebar-rename", &id), &mut text, &mut rename)
                })
                .inner;
            state.rename_text = text.clone();
            match outcome {
                RenameAction::Committed => {
                    state.rename = None;
                    *action = Some(SidebarAction::Rename { id, name: text });
                }
                RenameAction::Cancelled => state.rename = None,
                RenameAction::Idle => state.rename = Some((id, rename)),
            }
            return;
        }

        // Hover controls: ... and, on collections, +.
        if hovered || selected {
            let menu_rect = egui::Rect::from_min_size(egui::pos2(rect.right() - 22.0, rect.center().y - 8.0), vec2(16.0, 16.0));
            let menu = ui.interact(menu_rect, Id::new(("sidebar-menu", &row.id)), Sense::click());
            ui.painter().text(menu_rect.center(), egui::Align2::CENTER_CENTER, "…", egui::TextStyle::Body.resolve(ui.style()), theme.palette.text_muted);
            menu.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Item menu"));
            if menu.clicked() {
                state.selected.clear();
                state.selected.insert(row.id.clone());
                state.focused = Some(row.id.clone());
            }
            egui::Popup::menu(&menu).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
                ui.set_min_width(200.0);
                self.menu_contents(ui, state, row, all_rows, action);
            });
        }

        // Tooltip with the full name / URL.
        let tip = match (&row.url, row.kind) {
            (Some(url), _) => format!("{}\n{url}", row.label),
            _ => row.label.clone(),
        };
        let response = response.on_hover_text(tip);

        if response.clicked() {
            let modifiers = ui.input(|input| input.modifiers);
            state.select(row, all_rows, modifiers);
            if row.kind == RowKind::Environment && !modifiers.shift {
                *action = Some(SidebarAction::Open { id: row.id.clone(), kind: row.kind, preview: true });
            } else if row.kind != RowKind::History {
                *action = Some(SidebarAction::Open { id: row.id.clone(), kind: row.kind, preview: true });
            } else {
                *action = Some(SidebarAction::OpenHistory { id: row.id.clone() });
            }
        }
        if response.double_clicked() {
            match row.kind {
                RowKind::Collection | RowKind::Folder | RowKind::Request | RowKind::Environment => {
                    *action = Some(SidebarAction::Open { id: row.id.clone(), kind: row.kind, preview: false });
                }
                RowKind::History => *action = Some(SidebarAction::OpenHistory { id: row.id.clone() }),
                RowKind::Example => *action = Some(SidebarAction::Open { id: row.id.clone(), kind: row.kind, preview: false }),
            }
        }

        context_menu(&response, |ui| self.menu_contents(ui, state, row, all_rows, action));

        // Drag and drop.
        if response.drag_started() {
            state.drag = Some(row.id.clone());
        }
        if state.drag.as_deref() == Some(row.id.as_str()) && response.dragged() {
            ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.0, theme.palette.accent), egui::StrokeKind::Inside);
        }
        if state.drag.is_some() && response.hovered() && row.kind != RowKind::History {
            let pointer = ui.input(|input| input.pointer.hover_pos()).unwrap_or(rect.center());
            let before = pointer.y < rect.center().y;
            state.drop_target = Some((row.id.clone(), if before { row.index } else { row.index.saturating_add(1) }));
            let y = if before { rect.top() } else { rect.bottom() };
            ui.painter().line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], Stroke::new(2.0, theme.palette.accent));
        }
        let released = ui.input(|input| input.pointer.any_released());
        if released && let Some(dragged) = state.drag.take() {
            if let Some((target, index)) = state.drop_target.take()
                && target != dragged
            {
                let target_row = all_rows.iter().find(|candidate| candidate.id == target);
                let parent = match target_row.map(|row| row.kind) {
                    Some(RowKind::Collection) | Some(RowKind::Folder) => Some(target),
                    _ => target_row.and_then(|row| row.parent.clone()),
                };
                let copy = ui.input(|input| input.modifiers.command);
                *action = Some(SidebarAction::Move { id: dragged, parent, index, copy });
            }
        }
    }

    fn menu_contents(&self, ui: &mut Ui, state: &mut SidebarState, row: &SidebarRow, _all_rows: &[SidebarRow], action: &mut Option<SidebarAction>) {
        let collection = row.collection.clone().unwrap_or_else(|| row.id.clone());
        match row.kind {
            RowKind::Collection => {
                if menu_item(ui, Some("plus"), "Add request", None, true).clicked() {
                    *action = Some(SidebarAction::NewRequest { collection, parent: Some(row.id.clone()), protocol: RequestProtocol::Http });
                }
                if menu_item(ui, Some("plus"), "Add folder", None, true).clicked() {
                    *action = Some(SidebarAction::NewFolder { collection, parent: Some(row.id.clone()) });
                }
                if menu_item(ui, Some("send"), "Run", None, true).clicked() {
                    *action = Some(SidebarAction::Run { id: row.id.clone() });
                }
                ui.separator();
                if menu_item(ui, None, "Rename", Some("F2"), true).clicked() {
                    state.rename = Some((row.id.clone(), InlineRename::default()));
                    state.rename_text = row.label.clone();
                    if let Some((_, rename)) = &mut state.rename {
                        rename.begin(&row.label);
                    }
                }
                if menu_item(ui, None, "Duplicate", Some("Cmd+D"), true).clicked() {
                    *action = Some(SidebarAction::Duplicate { id: row.id.clone(), kind: row.kind });
                }
                if menu_item(ui, None, "Delete", Some("Del"), true).clicked() {
                    state.delete_confirm = Some(row.id.clone());
                }
                if menu_item(ui, None, "Export collection", None, true).clicked() {
                    *action = Some(SidebarAction::Export { id: row.id.clone() });
                }
            }
            RowKind::Folder => {
                if menu_item(ui, Some("plus"), "Add request", None, true).clicked() {
                    *action = Some(SidebarAction::NewRequest { collection, parent: Some(row.id.clone()), protocol: RequestProtocol::Http });
                }
                if menu_item(ui, Some("plus"), "Add folder", None, true).clicked() {
                    *action = Some(SidebarAction::NewFolder { collection, parent: Some(row.id.clone()) });
                }
                if menu_item(ui, Some("send"), "Run", None, true).clicked() {
                    *action = Some(SidebarAction::Run { id: row.id.clone() });
                }
                ui.separator();
                rename_item(ui, state, row, action);
                if menu_item(ui, None, "Duplicate", Some("Cmd+D"), true).clicked() {
                    *action = Some(SidebarAction::Duplicate { id: row.id.clone(), kind: row.kind });
                }
                if menu_item(ui, None, "Delete", Some("Del"), true).clicked() {
                    state.delete_confirm = Some(row.id.clone());
                }
            }
            RowKind::Request => {
                rename_item(ui, state, row, action);
                if menu_item(ui, None, "Duplicate", Some("Cmd+D"), true).clicked() {
                    *action = Some(SidebarAction::Duplicate { id: row.id.clone(), kind: row.kind });
                }
                if menu_item(ui, None, "Delete", Some("Del"), true).clicked() {
                    *action = Some(SidebarAction::Delete { id: row.id.clone(), kind: row.kind });
                }
                if menu_item(ui, None, "Add example", None, true).clicked() {
                    *action = Some(SidebarAction::CreateExample { id: row.id.clone() });
                }
                ui.separator();
                if menu_item(ui, None, "Copy as cURL", None, true).clicked() {
                    *action = Some(SidebarAction::CopyAsCurl { id: row.id.clone() });
                }
                if menu_item(ui, None, "Open in new tab", None, true).clicked() {
                    *action = Some(SidebarAction::OpenInNewTab { id: row.id.clone() });
                }
            }
            RowKind::Example => {
                rename_item(ui, state, row, action);
                if menu_item(ui, None, "Duplicate", None, true).clicked() {
                    *action = Some(SidebarAction::Duplicate { id: row.id.clone(), kind: row.kind });
                }
                if menu_item(ui, None, "Delete", Some("Del"), true).clicked() {
                    *action = Some(SidebarAction::Delete { id: row.id.clone(), kind: row.kind });
                }
                if menu_item(ui, None, "Open as request", None, true).clicked() {
                    *action = Some(SidebarAction::Open { id: row.id.clone(), kind: row.kind, preview: false });
                }
            }
            RowKind::Environment => {
                if menu_item(ui, Some("circle-check"), "Set active", None, true).clicked() {
                    *action = Some(SidebarAction::SetActiveEnvironment(Some(row.id.clone())));
                }
                rename_item(ui, state, row, action);
                if menu_item(ui, None, "Duplicate", None, true).clicked() {
                    *action = Some(SidebarAction::Duplicate { id: row.id.clone(), kind: row.kind });
                }
                if menu_item(ui, None, "Export", None, true).clicked() {
                    *action = Some(SidebarAction::Export { id: row.id.clone() });
                }
                if menu_item(ui, None, "Delete", Some("Del"), true).clicked() {
                    *action = Some(SidebarAction::Delete { id: row.id.clone(), kind: row.kind });
                }
                ui.separator();
                if menu_item(ui, None, "No environment", None, true).clicked() {
                    *action = Some(SidebarAction::SetActiveEnvironment(None));
                }
            }
            RowKind::History => {
                if menu_item(ui, None, "Open", None, true).clicked() {
                    *action = Some(SidebarAction::OpenHistory { id: row.id.clone() });
                }
                if menu_item(ui, None, "Delete", None, true).clicked() {
                    *action = Some(SidebarAction::DeleteHistory { id: row.id.clone() });
                }
            }
        }
    }

    fn keyboard(&self, ui: &mut Ui, state: &mut SidebarState, rows: &[SidebarRow], action: &mut Option<SidebarAction>) {
        if ui.ctx().wants_keyboard_input() {
            return;
        }
        let Some(focused) = state.focused.clone() else { return };
        let Some(position) = rows.iter().position(|row| row.id == focused) else { return };
        let mut move_to: Option<usize> = None;
        let mut toggle: Option<String> = None;
        let mut open_row: Option<SidebarRow> = None;
        ui.input(|input| {
            if input.key_pressed(Key::ArrowDown) {
                move_to = Some((position + 1).min(rows.len().saturating_sub(1)));
            }
            if input.key_pressed(Key::ArrowUp) {
                move_to = Some(position.saturating_sub(1));
            }
            if input.key_pressed(Key::ArrowRight) {
                if let Some(row) = rows.get(position)
                    && row.has_children
                    && !state.is_expanded(&row.id)
                {
                    toggle = Some(row.id.clone());
                }
            }
            if input.key_pressed(Key::ArrowLeft)
                && let Some(row) = rows.get(position)
                && row.has_children
                && state.is_expanded(&row.id)
            {
                toggle = Some(row.id.clone());
            }
            if input.key_pressed(Key::Enter) {
                open_row = rows.get(position).cloned();
            }
            if input.key_pressed(Key::F2) {
                if let Some(row) = rows.get(position) {
                    state.rename = Some((row.id.clone(), InlineRename::default()));
                    state.rename_text = row.label.clone();
                    if let Some((_, rename)) = &mut state.rename {
                        rename.begin(&row.label);
                    }
                }
            }
            if input.key_pressed(Key::Delete)
                && let Some(row) = rows.get(position)
            {
                match row.kind {
                    RowKind::Collection | RowKind::Folder => state.delete_confirm = Some(row.id.clone()),
                    RowKind::History => *action = Some(SidebarAction::DeleteHistory { id: row.id.clone() }),
                    RowKind::Environment => *action = Some(SidebarAction::SetActiveEnvironment(None)),
                    _ => *action = Some(SidebarAction::Delete { id: row.id.clone(), kind: row.kind }),
                }
            }
        });
        if let Some(index) = move_to
            && let Some(row) = rows.get(index)
        {
            state.selected.clear();
            state.selected.insert(row.id.clone());
            state.anchor = Some(row.id.clone());
            state.focused = Some(row.id.clone());
        }
        if let Some(id) = toggle {
            state.toggle_expanded(&id);
        }
        if let Some(row) = open_row {
            match row.kind {
                RowKind::Collection | RowKind::Folder => state.toggle_expanded(&row.id),
                RowKind::History => *action = Some(SidebarAction::OpenHistory { id: row.id }),
                _ => *action = Some(SidebarAction::Open { id: row.id, kind: row.kind, preview: false }),
            }
        }
    }
}

fn rename_item(ui: &mut Ui, state: &mut SidebarState, row: &SidebarRow, action: &mut Option<SidebarAction>) {
    if menu_item(ui, None, "Rename", Some("F2"), true).clicked() {
        state.rename = Some((row.id.clone(), InlineRename::default()));
        state.rename_text = row.label.clone();
        if let Some((_, rename)) = &mut state.rename {
            rename.begin(&row.label);
        }
        let _ = action;
    }
}

/// Row height (PROMPT.md 5.1: ~26 px).
pub const ROW_HEIGHT: f32 = 26.0;

/// Flatten the workspace into visible rows in tree order. Filtering keeps parents of matches
/// visible and expands nothing on its own (the caller can expand matches).
fn flatten(data: &SidebarData, filter: &str) -> Vec<SidebarRow> {
    let needle = filter.trim().to_ascii_lowercase();
    let filter_on = !needle.is_empty();
    let mut rows: Vec<SidebarRow> = Vec::new();
    for collection in &data.collections {
        let collection_matches = filter_on && matches_text(&collection.name, &needle, None);
        let child_rows = collect_children(&collection.items, collection, 1, None, filter_on, &needle, &HashMap::new());
        let keep = !filter_on || collection_matches || !child_rows.is_empty();
        if !keep {
            continue;
        }
        rows.push(SidebarRow {
            id: collection.id.0.clone(),
            parent: None,
            depth: 0,
            kind: RowKind::Collection,
            label: collection.name.clone(),
            url: None,
            method: None,
            has_children: !collection.items.is_empty(),
            expanded: true,
            active: false,
            index: 0,
            collection: Some(collection.id.0.clone()),
        });
        rows.extend(child_rows);
    }
    for environment in &data.environments {
        if filter_on && !matches_text(&environment.name, &needle, None) {
            continue;
        }
        rows.push(SidebarRow {
            id: environment.id.0.clone(),
            parent: None,
            depth: 0,
            kind: RowKind::Environment,
            label: environment.name.clone(),
            url: None,
            method: None,
            has_children: false,
            expanded: false,
            active: data.active_environment.as_ref() == Some(&environment.id),
            index: 0,
            collection: None,
        });
    }
    for entry in data.history.iter().rev() {
        let method = entry.request.as_ref().map(|request| match request {
            RequestKind::Http(http) => postguy_http_method(http.method),
            RequestKind::Grpc(_) => "gRPC".to_string(),
            RequestKind::WebSocket(_) => "WS".to_string(),
        });
        let url = entry.request.as_ref().and_then(|request| match request {
            RequestKind::Http(http) => Some(http.url.clone()),
            _ => None,
        });
        let label = url.clone().unwrap_or_else(|| entry.id.0.clone());
        if filter_on && !matches_text(&label, &needle, method.as_deref()) && !matches_text(&entry.id.0, &needle, None) {
            continue;
        }
        rows.push(SidebarRow {
            id: entry.id.0.clone(),
            parent: None,
            depth: 0,
            kind: RowKind::History,
            label,
            url,
            method,
            has_children: false,
            expanded: false,
            active: false,
            index: 0,
            collection: None,
        });
    }
    rows
}

fn collect_children(
    items: &[Item],
    collection: &Collection,
    depth: usize,
    parent: Option<&str>,
    filter_on: bool,
    needle: &str,
) -> Vec<SidebarRow> {
    let mut rows = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match item {
            Item::Folder(folder) => {
                let children = collect_children(&folder.items, collection, depth + 1, Some(&folder.id.0), filter_on, needle, _expanded);
                let matches = filter_on && matches_text(&folder.name, needle, None);
                if filter_on && !matches && children.is_empty() {
                    continue;
                }
                rows.push(SidebarRow {
                    id: folder.id.0.clone(),
                    parent: parent.map(str::to_string),
                    depth,
                    kind: RowKind::Folder,
                    label: folder.name.clone(),
                    url: None,
                    method: None,
                    has_children: !folder.items.is_empty(),
                    expanded: true,
                    active: false,
                    index,
                    collection: Some(collection.id.0.clone()),
                });
                rows.extend(children);
            }
            Item::Request(request) => {
                let (method, url) = match &request.kind {
                    RequestKind::Http(http) => (Some(postguy_http_method(http.method)), Some(http.url.clone())),
                    RequestKind::Grpc(_) => (Some("gRPC".to_string()), None),
                    RequestKind::WebSocket(_) => (Some("WS".to_string()), None),
                };
                let matches = filter_on && (matches_text(&request.name, needle, method.as_deref()) || url.as_deref().is_some_and(|url| matches_text(url, needle, None)));
                let examples: Vec<SidebarRow> = if filter_on {
                    Vec::new()
                } else {
                    request
                        .examples
                        .iter()
                        .enumerate()
                        .map(|(example_index, example)| SidebarRow {
                            id: example.id.0.clone(),
                            parent: Some(request.id.0.clone()),
                            depth: depth + 1,
                            kind: RowKind::Example,
                            label: example.name.clone(),
                            url: None,
                            method: None,
                            has_children: false,
                            expanded: false,
                            active: false,
                            index: example_index,
                            collection: Some(collection.id.0.clone()),
                        })
                        .collect()
                };
                if filter_on && !matches {
                    continue;
                }
                rows.push(SidebarRow {
                    id: request.id.0.clone(),
                    parent: parent.map(str::to_string),
                    depth,
                    kind: RowKind::Request,
                    label: request.name.clone(),
                    url,
                    method,
                    has_children: !examples.is_empty(),
                    expanded: false,
                    active: false,
                    index,
                    collection: Some(collection.id.0.clone()),
                });
                rows.extend(examples);
            }
        }
    }
    rows
}

fn matches_text(text: &str, needle: &str, extra: Option<&str>) -> bool {
    text.to_ascii_lowercase().contains(needle) || extra.is_some_and(|extra| extra.to_ascii_lowercase().contains(needle))
}

fn postguy_http_method(method: postguy_model::HttpMethod) -> String {
    match method {
        postguy_model::HttpMethod::Get => "GET",
        postguy_model::HttpMethod::Post => "POST",
        postguy_model::HttpMethod::Put => "PUT",
        postguy_model::HttpMethod::Patch => "PATCH",
        postguy_model::HttpMethod::Delete => "DELETE",
        postguy_model::HttpMethod::Head => "HEAD",
        postguy_model::HttpMethod::Options => "OPTIONS",
    }
    .to_string()
}

/// Truncate a name to the pixel width (approximate glyph width) with an ellipsis.
fn truncate(text: &str, width: f32, glyph_width: f32) -> String {
    let max_chars = (width / glyph_width).max(1.0) as usize;
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push('…');
    out
}

/// A row's method label colour for the painter (tests and future rows).
pub fn method_color(theme: &Theme, method: &str) -> Color32 {
    theme.method_color(method)
}

/// Align helper used by tests building a layout.
pub fn left_center(rect: egui::Rect) -> egui::Pos2 {
    egui::pos2(rect.left(), rect.center().y)
}

const _: Align = Align::Center;
const _: Layout = Layout::left_to_right(Align::Center);
