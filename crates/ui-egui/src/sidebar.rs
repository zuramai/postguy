//! The sidebar: filter, COLLECTIONS / ENVIRONMENTS / HISTORY sections,
//! a flattened tree with caret toggles, preview-open clicks, keyboard navigation, multi-select,
//! inline rename, typed delete confirmation and drag-and-drop (move, Cmd/Ctrl-drag copies).
//! The widget is data-in/actions-out: the app owns the engine and applies the returned
//! [`SidebarAction`]s, so the sidebar stays testable without a workspace.
//!
//! Row flattening is cached against `SidebarData::revision` and the filter text, so a 10 000-item
//! tree is walked only when the workspace or the filter changes; every frame paints the cached
//! row list and virtualises it with `ScrollArea::show_rows`.

use std::collections::BTreeSet;

use egui::{Color32, Id, Key, ScrollArea, Sense, Stroke, Ui, vec2};
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
            RowKind::Environment => "circle-check",
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
    /// Globals are a single file; the quick-look popover reads them without an engine handle.
    pub globals: Environment,
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
    DeleteMany { items: Vec<(String, RowKind)> },
    Move { id: String, parent: Option<String>, index: usize, copy: bool },
    MoveMany { ids: Vec<String>, parent: Option<String>, index: usize, copy: bool },
    Run { id: String },
    Export { id: String },
    SetActiveEnvironment(Option<String>),
    CopyAsCurl { id: String },
    OpenInNewTab { id: String },
    Copy { id: String },
    Cut { id: String },
    CopyMany { ids: Vec<String> },
    CutMany { ids: Vec<String> },
    Paste { target: String },
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
    /// Requests whose examples are shown (examples start collapsed).
    pub examples_expanded: BTreeSet<String>,
    /// Section heights in pixels (collections, environments, history); draggable splitters.
    pub section_heights: [f32; 3],
    pub sections: [bool; 3],
    /// Typed confirmation for deleting a collection or folder.
    pub delete_confirm: Option<String>,
    /// Typed confirmation for a bulk delete.
    pub delete_many_confirm: Option<Vec<(String, RowKind)>>,
    drag: Option<String>,
    drag_many: Vec<String>,
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
            examples_expanded: BTreeSet::new(),
            section_heights: [356.0, 140.0, 140.0],
            sections: [true, true, true],
            delete_confirm: None,
            delete_many_confirm: None,
            drag: None,
            drag_many: Vec::new(),
            drop_target: None,
            last_click_frame: 0,
        }
    }
}

impl SidebarState {
    /// Tests and the app can tell whether a drag is in flight.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The current drop target's row id, if hovering one.
    pub fn drop_target_id(&self) -> Option<&str> {
        self.drop_target.as_ref().map(|(id, _)| id.as_str())
    }

    pub fn is_expanded(&self, id: &str) -> bool {
        !self.collapsed.contains(id)
    }

    pub fn toggle_expanded(&mut self, id: &str) {
        if !self.collapsed.remove(id) {
            self.collapsed.insert(id.to_string());
        }
    }

    /// Requests expand their examples separately so examples start collapsed.
    pub fn toggle_examples(&mut self, id: &str) {
        if !self.examples_expanded.remove(id) {
            self.examples_expanded.insert(id.to_string());
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
        let available_width = ui.available_width();
        ui.set_max_width(available_width);

        self.toolbar(ui, state, &mut action);
        ui.add_space(2.0);
        let all_rows = self.rows(data, &state.filter).to_vec();
        let rows: Vec<SidebarRow> = all_rows
            .iter()
            .filter(|row| row.ancestors.iter().all(|ancestor| state.is_expanded(ancestor)))
            .filter(|row| row.kind != RowKind::Example || row.parent.as_ref().is_some_and(|parent| state.examples_expanded.contains(parent)))
            .cloned()
            .collect();

        // COLLECTIONS
        self.section_header(ui, state, 0, "COLLECTIONS", &theme);
        if state.sections[0] {
            let height = state.section_heights[0].min((ui.available_height() - 80.0).max(80.0));
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ScrollArea::vertical()
                    .id_salt("sidebar-collections")
                    .max_width(available_width)
                    .max_height(height)
                    .min_scrolled_height(height)
                    .auto_shrink([false, false])
                    .show_rows(
                        ui,
                        ROW_HEIGHT,
                        rows.iter()
                            .filter(|row| matches!(row.kind, RowKind::Collection | RowKind::Folder | RowKind::Request | RowKind::Example))
                            .count(),
                        |ui, range| {
                            let section_rows: Vec<&SidebarRow> = rows
                                .iter()
                                .filter(|row| matches!(row.kind, RowKind::Collection | RowKind::Folder | RowKind::Request | RowKind::Example))
                                .collect();
                            self.paint_rows(ui, state, &rows, &section_rows, range, &theme, &mut action);
                        },
                    );
            });
        }

        if state.sections[0] {
            section_splitter(ui, state, 0, &theme);
        }

        // ENVIRONMENTS
        self.section_header(ui, state, 1, "ENVIRONMENTS", &theme);
        if state.sections[1] {
            let env_rows: Vec<&SidebarRow> = rows.iter().filter(|row| row.kind == RowKind::Environment).collect();
            let height = state.section_heights[1].min((ui.available_height() - 60.0).max(60.0));
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ScrollArea::vertical()
                    .id_salt("sidebar-environments")
                    .max_width(available_width)
                    .max_height(height)
                    .min_scrolled_height(height)
                    .auto_shrink([false, false])
                    .show_rows(ui, ROW_HEIGHT, env_rows.len(), |ui, range| {
                        self.paint_rows(ui, state, &rows, &env_rows, range, &theme, &mut action);
                    });
            });
        }

        if state.sections[1] {
            section_splitter(ui, state, 1, &theme);
        }

        // HISTORY
        self.section_header(ui, state, 2, "HISTORY", &theme);
        if state.sections[2] {
            let history_rows: Vec<&SidebarRow> = rows.iter().filter(|row| row.kind == RowKind::History).collect();
            let height = state.section_heights[2].min((ui.available_height() - 30.0).max(30.0));
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ScrollArea::vertical()
                    .id_salt("sidebar-history")
                    .max_width(available_width)
                    .max_height(height)
                    .min_scrolled_height(height)
                    .auto_shrink([false, false])
                    .show_rows(ui, ROW_HEIGHT, history_rows.len(), |ui, range| {
                        self.paint_rows(ui, state, &rows, &history_rows, range, &theme, &mut action);
                    });
            });
        }

        // Typed confirmation for destructive deletes.
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

        if let Some(items) = state.delete_many_confirm.clone() {
            let mut open = true;
            let outcome = widgets::typed_confirm(
                ui.ctx(),
                ("sidebar-delete-many", items.len()),
                &mut open,
                "Delete items",
                &format!("Deleting {} items moves them to the trash.", items.len()),
                "delete",
                "Delete all",
            );
            match outcome {
                widgets::ConfirmOutcome::Confirmed => {
                    state.delete_many_confirm = None;
                    action = Some(SidebarAction::DeleteMany { items });
                }
                widgets::ConfirmOutcome::Cancelled => state.delete_many_confirm = None,
                widgets::ConfirmOutcome::Pending => {}
            }
        }

        self.keyboard(ui, state, &rows, &mut action);
        action
    }

    fn toolbar(&mut self, ui: &mut Ui, state: &mut SidebarState, action: &mut Option<SidebarAction>) {
        let theme = Theme::current(ui.ctx());
        let total_width = ui.available_width().max(80.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.spacing_mut().interact_size = vec2(24.0, 24.0);
            let filter_outer = if state.filter.is_empty() { (total_width - 72.0).max(40.0) } else { total_width };
            let (filter_rect, _) = ui.allocate_exact_size(vec2(filter_outer, 28.0), Sense::hover());
            ui.painter().rect(
                filter_rect,
                egui::CornerRadius::same(theme.metrics.radius as u8),
                theme.palette.input_bg,
                Stroke::new(1.0, theme.palette.input_border),
                egui::StrokeKind::Inside,
            );
            icons::paint(
                ui.painter(),
                egui::Rect::from_center_size(egui::pos2(filter_rect.left() + 15.0, filter_rect.center().y), vec2(13.0, 13.0)),
                "funnel",
                theme.palette.text_muted,
                1.5,
            );
            let inner_rect = egui::Rect::from_min_max(
                egui::pos2(filter_rect.left() + 26.0, filter_rect.top() + 4.0),
                egui::pos2((filter_rect.right() - 8.0).max(filter_rect.left() + 27.0), filter_rect.bottom() - 4.0),
            );
            let response = ui
                .scope_builder(egui::UiBuilder::new().max_rect(inner_rect), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut state.filter)
                            .hint_text("Filter")
                            .desired_width(inner_rect.width())
                            .margin(egui::Margin::ZERO)
                            .frame(egui::Frame::NONE),
                    )
                })
                .inner;
            if state.filter.is_empty() {
                let add = widgets::icon_button_with(ui, "plus", "+", "New…");
                egui::Popup::menu(&add).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
                    ui.set_min_width(190.0);
                    let selected_container =
                        state.selected.iter().next().and_then(|id| self.rows.iter().find(|row| row.id == *id)).and_then(|row| match row.kind {
                            RowKind::Collection => Some((row.collection.clone().unwrap_or_else(|| row.id.clone()), None)),
                            RowKind::Folder => Some((row.collection.clone().unwrap_or_else(|| row.id.clone()), Some(row.id.clone()))),
                            _ => None,
                        });
                    if menu_item(ui, Some("plus"), "New HTTP request", None, selected_container.is_some()).clicked()
                        && let Some((collection, parent)) = &selected_container
                    {
                        *action = Some(SidebarAction::NewRequest {
                            collection: collection.clone(),
                            parent: parent.clone(),
                            protocol: RequestProtocol::Http,
                        });
                    }
                    if menu_item(ui, Some("plus"), "New gRPC request", None, selected_container.is_some()).clicked()
                        && let Some((collection, parent)) = &selected_container
                    {
                        *action = Some(SidebarAction::NewRequest {
                            collection: collection.clone(),
                            parent: parent.clone(),
                            protocol: RequestProtocol::Grpc,
                        });
                    }
                    if menu_item(ui, Some("plus"), "New WebSocket request", None, selected_container.is_some()).clicked()
                        && let Some((collection, parent)) = &selected_container
                    {
                        *action = Some(SidebarAction::NewRequest {
                            collection: collection.clone(),
                            parent: parent.clone(),
                            protocol: RequestProtocol::WebSocket,
                        });
                    }
                    ui.separator();
                    if menu_item(ui, None, "New collection", None, true).clicked() {
                        *action = Some(SidebarAction::NewCollection);
                    }
                    if menu_item(ui, None, "New environment", None, true).clicked() {
                        *action = Some(SidebarAction::NewEnvironment);
                    }
                });
                let import = widgets::icon_button_with(ui, "import", "Import", "Import…");
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
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
        if response.clicked() {
            state.sections[index] = !state.sections[index];
        }
        if response.hovered() {
            ui.painter().rect_filled(rect.shrink2(vec2(2.0, 2.0)), egui::CornerRadius::same(theme.metrics.radius as u8), theme.palette.hover);
        }
        paint_chevron(ui.painter(), rect.left_center() + vec2(8.0, 0.0), state.sections[index], theme.palette.text, 3.8);
        ui.painter().text(
            rect.left_center() + vec2(20.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            theme.semibold_font(11.0),
            theme.palette.text_muted,
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_rows(
        &mut self,
        ui: &mut Ui,
        state: &mut SidebarState,
        all_rows: &[SidebarRow],
        section_rows: &[&SidebarRow],
        range: std::ops::Range<usize>,
        theme: &Theme,
        action: &mut Option<SidebarAction>,
    ) {
        for index in range {
            if let Some(row) = section_rows.get(index) {
                let next_row = section_rows.get(index + 1).copied();
                self.paint_row(ui, state, all_rows, row, next_row, theme, action);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_row(
        &mut self,
        ui: &mut Ui,
        state: &mut SidebarState,
        all_rows: &[SidebarRow],
        row: &SidebarRow,
        next_row: Option<&SidebarRow>,
        theme: &Theme,
        action: &mut Option<SidebarAction>,
    ) {
        let indented = ui.available_width().max(1.0);
        let (rect, response) = ui.allocate_exact_size(vec2(indented, ROW_HEIGHT), Sense::click_and_drag());
        let pointer_inside = ui.input(|input| input.pointer.hover_pos().is_some_and(|pos| rect.contains(pos)));
        let hovered = response.hovered() || pointer_inside;
        let selected = state.selected.contains(&row.id);
        let pill_rect = rect.shrink2(vec2(2.0, 1.0));
        let rounding = egui::CornerRadius::same(theme.metrics.radius as u8);
        if selected {
            ui.painter().rect_filled(pill_rect, rounding, theme.palette.selected);
        } else if hovered {
            ui.painter().rect_filled(pill_rect, rounding, theme.palette.hover);
        }

        // Vertical tree guide lines directly under the collection chevron (depth 0) and each
        // expanded folder icon (depth >= 1).
        let guide_stroke = Stroke::new(1.0, theme.palette.guide);
        if matches!(row.kind, RowKind::Collection | RowKind::Folder | RowKind::Request | RowKind::Example) {
            let folder_levels = if row.kind == RowKind::Example { row.depth.saturating_sub(1) } else { row.depth };
            for lvl in 0..folder_levels {
                let gx = if lvl == 0 { rect.left() + TREE_COL_CHEVRON_X } else { tree_folder_icon_x(rect.left(), lvl) };
                let continues = next_row.is_some_and(|next| next.depth > lvl && next.ancestors.get(lvl) == row.ancestors.get(lvl));
                let y_bottom = if continues { rect.bottom() } else { rect.bottom() - 4.0 };
                ui.painter().line_segment([egui::pos2(gx, rect.top()), egui::pos2(gx, y_bottom)], guide_stroke);
            }
            // Connect the top of the guide line right below an expanded collection/folder icon.
            if next_row.is_some_and(|next| next.depth > row.depth) {
                let (gx, y_start) = match row.kind {
                    RowKind::Collection => (rect.left() + TREE_COL_CHEVRON_X, rect.center().y + 7.0),
                    RowKind::Folder => (tree_folder_icon_x(rect.left(), row.depth), rect.center().y + 8.5),
                    _ => (0.0, rect.bottom()),
                };
                if y_start < rect.bottom() {
                    ui.painter().line_segment([egui::pos2(gx, y_start), egui::pos2(gx, rect.bottom())], guide_stroke);
                }
            }
        }

        let chevron_center_x = tree_chevron_x(rect.left(), row.depth);
        // Caret (collections, folders, requests with examples).
        if row.has_children || matches!(row.kind, RowKind::Collection | RowKind::Folder) {
            let caret_rect = egui::Rect::from_center_size(egui::pos2(chevron_center_x, rect.center().y), vec2(14.0, 14.0));
            let caret_response = ui.interact(caret_rect, Id::new(("sidebar-caret", &row.id)), Sense::click());
            let expanded = if row.kind == RowKind::Request { state.examples_expanded.contains(&row.id) } else { state.is_expanded(&row.id) };
            let caret_color = if caret_response.hovered() { theme.palette.text } else { theme.palette.text_muted };
            if row.has_children {
                paint_chevron(ui.painter(), caret_rect.center(), expanded, caret_color, 3.6);
            } else {
                paint_chevron(ui.painter(), caret_rect.center(), false, caret_color, 3.6);
            }
            caret_response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{} {}", if expanded { "Collapse" } else { "Expand" }, row.label))
            });
            let caret_response = caret_response.on_hover_cursor(egui::CursorIcon::PointingHand);
            if caret_response.clicked() {
                if row.kind == RowKind::Request {
                    state.toggle_examples(&row.id);
                } else {
                    state.toggle_expanded(&row.id);
                }
                return;
            }
        }

        let mut x = match row.kind {
            RowKind::Collection => rect.left() + TREE_DEPTH1_CHEVRON_X,
            RowKind::Folder => {
                let folder_x = tree_folder_icon_x(rect.left(), row.depth);
                let icon_rect = egui::Rect::from_center_size(egui::pos2(folder_x, rect.center().y), egui::Vec2::splat(15.0));
                let folder_color = theme.palette.text_muted;
                let folder_icon = if state.is_expanded(&row.id) { "folder-open" } else { "folder" };
                icons::paint(ui.painter(), icon_rect, folder_icon, folder_color, 1.45);
                folder_x + 12.0
            }
            RowKind::Request => {
                let method = row.method.as_deref().unwrap_or("GET");
                let method_x = chevron_center_x + 8.0;
                let galley = ui.painter().layout_no_wrap(method.to_uppercase(), theme.semibold_font(9.5), theme.method_color(method));
                let width = galley.size().x;
                ui.painter().galley(egui::pos2(method_x, rect.center().y - galley.size().y / 2.0), galley, theme.method_color(method));
                method_x + (width + 6.0).max(34.0)
            }
            RowKind::Example => {
                let parent_depth = row.depth.saturating_sub(1);
                let ex_left = tree_chevron_x(rect.left(), parent_depth) + 8.0;
                let ex_rect = egui::Rect::from_min_size(egui::pos2(ex_left, rect.center().y - 6.5), vec2(18.0, 13.0));
                paint_example_icon(ui.painter(), ex_rect, theme.palette.text_muted);
                ex_rect.right() + 6.0
            }
            RowKind::Environment => rect.left() + TREE_DEPTH1_CHEVRON_X,
            RowKind::History => {
                let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 14.0, rect.center().y), egui::Vec2::splat(13.5));
                icons::paint(ui.painter(), icon_rect, row.kind.icon(), theme.palette.text_muted, 1.4);
                if let Some(method) = row.method.as_deref().filter(|m| !m.is_empty()) {
                    let method_x = rect.left() + 25.0;
                    let label = method.to_uppercase();
                    let galley = ui.painter().layout_no_wrap(label, theme.semibold_font(9.5), theme.method_color(method));
                    let width = galley.size().x;
                    ui.painter().galley(egui::pos2(method_x, rect.center().y - galley.size().y / 2.0), galley, theme.method_color(method));
                    method_x + (width + 6.0).max(34.0)
                } else {
                    rect.left() + 25.0
                }
            }
        };

        let label_color = theme.palette.text;
        let right_reserve = if hovered || selected {
            if row.kind == RowKind::Collection { 46.0 } else { 28.0 }
        } else if row.active {
            26.0
        } else {
            10.0
        };
        let available_label = (rect.right() - x - right_reserve).max(20.0);
        let clipped = truncate(&row.label, available_label, 7.2);
        paint_highlighted_name(ui, egui::pos2(x, rect.center().y), &clipped, label_color, &state.filter, theme);
        let _ = &mut x;
        if row.active {
            let badge_x = if hovered || selected { rect.right() - 34.0 } else { rect.right() - 14.0 };
            paint_active_env_badge(ui.painter(), egui::pos2(badge_x, rect.center().y), theme);
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
            if row.kind == RowKind::Collection {
                let add_rect = egui::Rect::from_min_size(egui::pos2(rect.right() - 44.0, rect.center().y - 10.0), vec2(20.0, 20.0));
                let add = ui.interact(add_rect, Id::new(("sidebar-add", &row.id)), Sense::click());
                if add.hovered() {
                    ui.painter().rect_filled(add_rect, egui::CornerRadius::same(theme.metrics.radius as u8), theme.palette.button_hover);
                }
                ui.painter().text(
                    add_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "+",
                    egui::TextStyle::Body.resolve(ui.style()),
                    if add.hovered() { theme.palette.text } else { theme.palette.text_muted },
                );
                add.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Add request to {}", row.label)));
                let add = add.on_hover_cursor(egui::CursorIcon::PointingHand);
                if add.clicked() {
                    *action = Some(SidebarAction::NewRequest {
                        collection: row.collection.clone().unwrap_or_else(|| row.id.clone()),
                        parent: None,
                        protocol: RequestProtocol::Http,
                    });
                }
            }
            let menu_rect = egui::Rect::from_min_size(egui::pos2(rect.right() - 22.0, rect.center().y - 10.0), vec2(20.0, 20.0));
            let menu = ui.interact(menu_rect, Id::new(("sidebar-menu", &row.id)), Sense::click());
            if menu.hovered() {
                ui.painter().rect_filled(menu_rect, egui::CornerRadius::same(theme.metrics.radius as u8), theme.palette.button_hover);
            }
            ui.painter().text(
                menu_rect.center(),
                egui::Align2::CENTER_CENTER,
                "…",
                egui::TextStyle::Body.resolve(ui.style()),
                if menu.hovered() { theme.palette.text } else { theme.palette.text_muted },
            );
            menu.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Item menu"));
            let menu = menu.on_hover_cursor(egui::CursorIcon::PointingHand);
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
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, &row.label));
        if response.clicked() {
            let modifiers = ui.input(|input| input.modifiers);
            state.select(row, all_rows, modifiers);
            if row.kind != RowKind::History {
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
        if response.drag_started() && row.kind != RowKind::Example {
            state.drag = Some(row.id.clone());
            state.drag_many = if state.selected.len() > 1 && state.selected.contains(&row.id) {
                all_rows
                    .iter()
                    .filter(|candidate| {
                        state.selected.contains(&candidate.id) && matches!(candidate.kind, RowKind::Folder | RowKind::Request | RowKind::Example)
                    })
                    .map(|candidate| candidate.id.clone())
                    .collect()
            } else {
                Vec::new()
            };
        }
        if state.drag.as_deref() == Some(row.id.as_str()) && response.dragged() {
            ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.0, theme.palette.accent), egui::StrokeKind::Inside);
        }
        let pointer_over_row = ui.input(|input| input.pointer.hover_pos().is_some_and(|pointer| rect.contains(pointer)));
        if state.drag.is_some() && pointer_over_row && row.kind != RowKind::History {
            let pointer = ui.input(|input| input.pointer.hover_pos()).unwrap_or(rect.center());
            let container = matches!(row.kind, RowKind::Collection | RowKind::Folder);
            let onto = container && pointer.y > rect.top() + rect.height() * 0.25 && pointer.y < rect.bottom() - rect.height() * 0.25;
            if onto {
                state.drop_target = Some((row.id.clone(), usize::MAX));
                ui.painter().rect_stroke(rect, 4.0, Stroke::new(2.0, theme.palette.accent), egui::StrokeKind::Inside);
            } else {
                let before = pointer.y < rect.center().y;
                state.drop_target = Some((row.id.clone(), if before { row.index } else { row.index.saturating_add(1) }));
                let y = if before { rect.top() } else { rect.bottom() };
                ui.painter().line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], Stroke::new(2.0, theme.palette.accent));
            }
        }
        let released = ui.input(|input| input.pointer.any_released());
        if released
            && let Some(dragged) = state.drag.take()
            && let Some((target, index)) = state.drop_target.take()
            && target != dragged
        {
            let target_row = all_rows.iter().find(|candidate| candidate.id == target);
            let (parent, index) = if index == usize::MAX { (Some(target), 0) } else { (target_row.and_then(|row| row.parent.clone()), index) };
            let copy = ui.input(|input| input.modifiers.command);
            let many = std::mem::take(&mut state.drag_many);
            if many.len() > 1 {
                *action = Some(SidebarAction::MoveMany { ids: many, parent, index, copy });
            } else {
                *action = Some(SidebarAction::Move { id: dragged, parent, index, copy });
            }
        }
    }

    fn menu_contents(&self, ui: &mut Ui, state: &mut SidebarState, row: &SidebarRow, all_rows: &[SidebarRow], action: &mut Option<SidebarAction>) {
        let collection = row.collection.clone().unwrap_or_else(|| row.id.clone());
        match row.kind {
            RowKind::Collection => {
                if menu_item(ui, Some("plus"), "Add request", None, true).clicked() {
                    *action = Some(SidebarAction::NewRequest { collection: collection.clone(), parent: None, protocol: RequestProtocol::Http });
                }
                if menu_item(ui, Some("plus"), "Add folder", None, true).clicked() {
                    *action = Some(SidebarAction::NewFolder { collection: collection.clone(), parent: None });
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
                    apply_delete_intent(state, row, all_rows, action);
                }
                if menu_item(ui, None, "Export collection", None, true).clicked() {
                    *action = Some(SidebarAction::Export { id: row.id.clone() });
                }
                ui.separator();
                if menu_item(ui, None, "Copy", Some("Cmd+C"), true).clicked() {
                    *action = Some(SidebarAction::Copy { id: row.id.clone() });
                }
                if menu_item(ui, None, "Paste", Some("Cmd+V"), true).clicked() {
                    *action = Some(SidebarAction::Paste { target: row.id.clone() });
                }
            }
            RowKind::Folder => {
                if menu_item(ui, Some("plus"), "Add request", None, true).clicked() {
                    *action = Some(SidebarAction::NewRequest {
                        collection: collection.clone(),
                        parent: Some(row.id.clone()),
                        protocol: RequestProtocol::Http,
                    });
                }
                if menu_item(ui, Some("plus"), "Add folder", None, true).clicked() {
                    *action = Some(SidebarAction::NewFolder { collection: collection.clone(), parent: Some(row.id.clone()) });
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
                    apply_delete_intent(state, row, all_rows, action);
                }
                ui.separator();
                if menu_item(ui, None, "Copy", Some("Cmd+C"), true).clicked() {
                    *action = Some(SidebarAction::Copy { id: row.id.clone() });
                }
                if menu_item(ui, None, "Cut", Some("Cmd+X"), true).clicked() {
                    *action = Some(SidebarAction::Cut { id: row.id.clone() });
                }
                if menu_item(ui, None, "Paste", Some("Cmd+V"), true).clicked() {
                    *action = Some(SidebarAction::Paste { target: row.id.clone() });
                }
            }
            RowKind::Request => {
                rename_item(ui, state, row, action);
                if menu_item(ui, None, "Duplicate", Some("Cmd+D"), true).clicked() {
                    *action = Some(SidebarAction::Duplicate { id: row.id.clone(), kind: row.kind });
                }
                if menu_item(ui, None, "Delete", Some("Del"), true).clicked() {
                    apply_delete_intent(state, row, all_rows, action);
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
                ui.separator();
                if menu_item(ui, None, "Copy", Some("Cmd+C"), true).clicked() {
                    *action = Some(SidebarAction::Copy { id: row.id.clone() });
                }
                if menu_item(ui, None, "Cut", Some("Cmd+X"), true).clicked() {
                    *action = Some(SidebarAction::Cut { id: row.id.clone() });
                }
                if menu_item(ui, None, "Paste", Some("Cmd+V"), true).clicked() {
                    *action = Some(SidebarAction::Paste { target: row.collection.clone().unwrap_or_else(|| row.id.clone()) });
                }
            }
            RowKind::Example => {
                rename_item(ui, state, row, action);
                if menu_item(ui, None, "Duplicate", None, true).clicked() {
                    *action = Some(SidebarAction::Duplicate { id: row.id.clone(), kind: row.kind });
                }
                if menu_item(ui, None, "Delete", Some("Del"), true).clicked() {
                    apply_delete_intent(state, row, all_rows, action);
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
                    apply_delete_intent(state, row, all_rows, action);
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
        if ui.ctx().egui_wants_keyboard_input() {
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
            if input.key_pressed(Key::ArrowRight)
                && let Some(row) = rows.get(position)
                && row.has_children
                && (row.kind == RowKind::Request && !state.examples_expanded.contains(&row.id)
                    || row.kind != RowKind::Request && !state.is_expanded(&row.id))
            {
                toggle = Some(row.id.clone());
            }
            if input.key_pressed(Key::ArrowLeft)
                && let Some(row) = rows.get(position)
                && row.has_children
                && (row.kind == RowKind::Request && state.examples_expanded.contains(&row.id)
                    || row.kind != RowKind::Request && state.is_expanded(&row.id))
            {
                toggle = Some(row.id.clone());
            }
            if input.key_pressed(Key::Enter) {
                open_row = rows.get(position).cloned();
            }
            if input.key_pressed(Key::F2)
                && let Some(row) = rows.get(position)
            {
                state.rename = Some((row.id.clone(), InlineRename::default()));
                state.rename_text = row.label.clone();
                if let Some((_, rename)) = &mut state.rename {
                    rename.begin(&row.label);
                }
            }
            if input.modifiers.command
                && input.key_pressed(Key::C)
                && let Some(row) = rows.get(position)
            {
                *action = Some(if state.selected.len() > 1 {
                    SidebarAction::CopyMany { ids: state.selected.iter().cloned().collect() }
                } else {
                    SidebarAction::Copy { id: row.id.clone() }
                });
            }
            if input.modifiers.command
                && input.key_pressed(Key::X)
                && let Some(row) = rows.get(position)
            {
                *action = Some(if state.selected.len() > 1 {
                    SidebarAction::CutMany { ids: state.selected.iter().cloned().collect() }
                } else {
                    SidebarAction::Cut { id: row.id.clone() }
                });
            }
            if input.modifiers.command
                && input.key_pressed(Key::V)
                && let Some(row) = rows.get(position)
            {
                let target = match row.kind {
                    RowKind::Collection | RowKind::Folder => row.id.clone(),
                    _ => row.collection.clone().unwrap_or_else(|| row.id.clone()),
                };
                *action = Some(SidebarAction::Paste { target });
            }
            if input.key_pressed(Key::Delete)
                && let Some(row) = rows.get(position)
            {
                if row.kind == RowKind::History {
                    *action = Some(SidebarAction::DeleteHistory { id: row.id.clone() });
                } else {
                    apply_delete_intent(state, row, rows, action);
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
            let is_request = rows.iter().any(|row| row.id == id && row.kind == RowKind::Request);
            if is_request {
                state.toggle_examples(&id);
            } else {
                state.toggle_expanded(&id);
            }
        }
        if let Some(row) = open_row {
            match row.kind {
                RowKind::Collection | RowKind::Folder => state.toggle_expanded(&row.id),
                RowKind::Request if row.has_children => state.toggle_examples(&row.id),
                RowKind::History => *action = Some(SidebarAction::OpenHistory { id: row.id }),
                _ => *action = Some(SidebarAction::Open { id: row.id, kind: row.kind, preview: false }),
            }
        }
    }
}

/// Turn a Delete click/key into either a single or a bulk intent. A multi-selection containing
/// the clicked row deletes all selected rows (collections/folders in the set need one typed
/// confirmation for the group); otherwise collections/folders ask and the rest act immediately.
fn apply_delete_intent(state: &mut SidebarState, row: &SidebarRow, all_rows: &[SidebarRow], action: &mut Option<SidebarAction>) {
    let bulk = state.selected.len() > 1 && state.selected.contains(&row.id);
    let mut items: Vec<(String, RowKind)> = if bulk {
        all_rows
            .iter()
            .filter(|candidate| state.selected.contains(&candidate.id) && candidate.kind != RowKind::History)
            .map(|candidate| (candidate.id.clone(), candidate.kind))
            .collect()
    } else {
        vec![(row.id.clone(), row.kind)]
    };
    if items.is_empty() {
        return;
    }
    let needs_typed = items.iter().any(|(_, kind)| matches!(kind, RowKind::Collection | RowKind::Folder));
    if needs_typed {
        if bulk {
            state.delete_many_confirm = Some(items);
        } else if let Some((id, _)) = items.pop() {
            state.delete_confirm = Some(id);
        }
    } else if bulk {
        *action = Some(SidebarAction::DeleteMany { items });
    } else if let Some((id, kind)) = items.pop() {
        *action = Some(SidebarAction::Delete { id, kind });
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

/// Row height (about 26 px).
pub const ROW_HEIGHT: f32 = 28.0;

/// Flatten the workspace into visible rows in tree order. Filtering keeps parents of matches
/// visible and expands nothing on its own (the caller can expand matches).
fn flatten(data: &SidebarData, filter: &str) -> Vec<SidebarRow> {
    let needle = filter.trim().to_ascii_lowercase();
    let filter_on = !needle.is_empty();
    let mut rows: Vec<SidebarRow> = Vec::new();
    for collection in &data.collections {
        let collection_matches = filter_on && matches_text(&collection.name, &needle, None);
        let root_ancestors = [collection.id.0.clone()];
        let child_rows = collect_children(&collection.items, collection, 1, None, filter_on, &needle, &root_ancestors);
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
            ancestors: Vec::new(),
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
            ancestors: Vec::new(),
        });
    }
    for entry in data.history.iter().rev() {
        let method = entry.request.as_ref().map(|request| match request {
            RequestKind::Http(http) => postguy_http_method(http.method),
            RequestKind::Grpc(_) => "gRPC".to_string(),
            RequestKind::WebSocket(_) => "WS".to_string(),
            _ => String::new(),
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
            ancestors: Vec::new(),
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
    ancestors: &[String],
) -> Vec<SidebarRow> {
    let mut rows = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match item {
            Item::Folder(folder) => {
                let mut child_ancestors = ancestors.to_vec();
                child_ancestors.push(folder.id.0.clone());
                let children = collect_children(&folder.items, collection, depth + 1, Some(&folder.id.0), filter_on, needle, &child_ancestors);
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
                    ancestors: ancestors.to_vec(),
                });
                rows.extend(children);
            }
            Item::Request(request) => {
                let (method, url) = match &request.kind {
                    RequestKind::Http(http) => (Some(postguy_http_method(http.method)), Some(http.url.clone())),
                    RequestKind::Grpc(_) => (Some("gRPC".to_string()), None),
                    RequestKind::WebSocket(_) => (Some("WS".to_string()), None),
                    _ => (None, None),
                };
                let matches = filter_on
                    && (matches_text(&request.name, needle, method.as_deref()) || url.as_deref().is_some_and(|url| matches_text(url, needle, None)));
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
                            ancestors: {
                                let mut chain = ancestors.to_vec();
                                chain.push(request.id.0.clone());
                                chain
                            },
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
                    ancestors: ancestors.to_vec(),
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

/// Horizontal coordinate of a row's expand/collapse chevron center at `depth`.
const TREE_COL_CHEVRON_X: f32 = 12.0;
const TREE_DEPTH1_CHEVRON_X: f32 = 24.0;
const TREE_DEPTH_STEP_X: f32 = 26.0;
const TREE_FOLDER_ICON_OFFSET_X: f32 = 14.5;

fn tree_chevron_x(left: f32, depth: usize) -> f32 {
    if depth == 0 { left + TREE_COL_CHEVRON_X } else { left + TREE_DEPTH1_CHEVRON_X + (depth - 1) as f32 * TREE_DEPTH_STEP_X }
}

fn tree_folder_icon_x(left: f32, depth: usize) -> f32 {
    tree_chevron_x(left, depth) + TREE_FOLDER_ICON_OFFSET_X
}

/// A draggable 7 px splitter above section `index`; dragging changes that section's height.
fn section_splitter(ui: &mut Ui, state: &mut SidebarState, index: usize, theme: &Theme) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::drag());
    let hovered = response.hovered() || response.dragged();
    ui.painter().line_segment(
        [egui::pos2(rect.left() + 2.0, rect.center().y), egui::pos2(rect.right() - 2.0, rect.center().y)],
        Stroke::new(1.0, if hovered { theme.palette.accent } else { theme.palette.border }),
    );
    if response.dragged()
        && let Some(height) = state.section_heights.get_mut(index)
    {
        *height = (*height + response.drag_delta().y).clamp(60.0, 1200.0);
    }
    response.on_hover_cursor(egui::CursorIcon::ResizeVertical);
}

/// Stroked right (`>`) or down (`v`) chevron matching the clean tree carets in Collections.png.
fn paint_chevron(painter: &egui::Painter, center: egui::Pos2, down: bool, color: Color32, size: f32) {
    let points = if down {
        vec![
            egui::pos2(center.x - size, center.y - size * 0.45),
            egui::pos2(center.x, center.y + size * 0.55),
            egui::pos2(center.x + size, center.y - size * 0.45),
        ]
    } else {
        vec![
            egui::pos2(center.x - size * 0.45, center.y - size),
            egui::pos2(center.x + size * 0.55, center.y),
            egui::pos2(center.x - size * 0.45, center.y + size),
        ]
    };
    painter.add(egui::Shape::line(points, Stroke::new(1.5, color)));
}

/// The `e.g.` example glyph (top rule, `e.g.` label, bottom dashed rule).
fn paint_example_icon(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    let stroke = Stroke::new(1.15, color);
    painter.line_segment([egui::pos2(rect.left() + 1.0, rect.top() + 1.0), egui::pos2(rect.right() - 1.0, rect.top() + 1.0)], stroke);
    painter.text(rect.center() + vec2(0.0, -0.5), egui::Align2::CENTER_CENTER, "e.g.", egui::FontId::proportional(8.5), color);
    let y = rect.bottom() - 1.0;
    let dash = 3.2;
    let gap = 2.2;
    let mut x = rect.left() + 1.0;
    while x < rect.right() - 1.0 {
        let end = (x + dash).min(rect.right() - 1.0);
        painter.line_segment([egui::pos2(x, y), egui::pos2(end, y)], stroke);
        x += dash + gap;
    }
}

/// Filled circle with a white checkmark on the right edge of the active environment row.
fn paint_active_env_badge(painter: &egui::Painter, center: egui::Pos2, theme: &Theme) {
    let fill = if theme.dark { Color32::from_rgb(0x88, 0x90, 0x9E) } else { Color32::from_rgb(0x61, 0x66, 0x6E) };
    let check = if theme.dark { theme.palette.surface } else { Color32::WHITE };
    painter.circle_filled(center, 7.5, fill);
    painter.add(egui::Shape::line(
        vec![egui::pos2(center.x - 3.2, center.y + 0.1), egui::pos2(center.x - 0.8, center.y + 2.5), egui::pos2(center.x + 3.4, center.y - 2.3)],
        Stroke::new(1.6, check),
    ));
}
/// Paint the row name, tinting filter matches. Left-centered
/// at `center_left`.
fn paint_highlighted_name(ui: &Ui, center_left: egui::Pos2, text: &str, color: Color32, filter: &str, theme: &Theme) {
    let needle = filter.trim().to_ascii_lowercase();
    if needle.is_empty() {
        ui.painter().text(center_left, egui::Align2::LEFT_CENTER, text, egui::TextStyle::Body.resolve(ui.style()), color);
        return;
    }
    let haystack = text.to_ascii_lowercase();
    let mut job = egui::text::LayoutJob::default();
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut cursor = 0usize;
    while let Some(found) = haystack.get(cursor..).and_then(|rest| rest.find(&needle)) {
        let start = cursor + found;
        let end = start + needle.len();
        if start > cursor {
            job.append(text.get(cursor..start).unwrap_or_default(), 0.0, egui::TextFormat { font_id: font.clone(), color, ..Default::default() });
        }
        job.append(
            text.get(start..end).unwrap_or_default(),
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color: theme.palette.accent,
                background: theme.palette.accent.gamma_multiply(0.22),
                ..Default::default()
            },
        );
        cursor = end;
    }
    if cursor < text.len() {
        job.append(text.get(cursor..).unwrap_or_default(), 0.0, egui::TextFormat { font_id: font.clone(), color, ..Default::default() });
    }
    if job.sections.is_empty() {
        job.append(text, 0.0, egui::TextFormat { font_id: font, color, ..Default::default() });
    }
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let pos = egui::pos2(center_left.x, center_left.y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, color);
}

/// A row's method label colour for the painter (tests and future rows).
pub fn method_color(theme: &Theme, method: &str) -> Color32 {
    theme.method_color(method)
}

/// Align helper used by tests building a layout.
pub fn left_center(rect: egui::Rect) -> egui::Pos2 {
    egui::pos2(rect.left(), rect.center().y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_thousand_rows_flatten_and_filter_within_a_frame_budget() {
        let mut collections = Vec::new();
        for collection in 0..100 {
            let mut items = Vec::new();
            for request in 0..100 {
                items.push(Item::Request(Box::new(postguy_model::Request {
                    id: postguy_model::Id(format!("r{collection}-{request}")),
                    name: format!("Request {collection}/{request}"),
                    kind: postguy_model::RequestKind::Http(postguy_model::HttpRequest {
                        url: format!("https://api.example.com/c{collection}/r{request}"),
                        ..Default::default()
                    }),
                    ..Default::default()
                })));
            }
            collections.push(Collection {
                id: postguy_model::Id(format!("c{collection}")),
                name: format!("Collection {collection}"),
                items,
                ..Default::default()
            });
        }
        let data = SidebarData { revision: 1, collections, ..Default::default() };
        let mut sidebar = Sidebar::new();
        let started = std::time::Instant::now();
        let rows = sidebar.rows(&data, "").len();
        let cold = started.elapsed();
        assert_eq!(rows, 10_100, "100 collections + 10 000 requests");
        let started = std::time::Instant::now();
        let filtered = sidebar.rows(&data, "Request 99/99").len();
        let filter_time = started.elapsed();
        assert!(filtered >= 1);
        println!("sidebar: 10 100 rows cold {cold:?}, filtered {filter_time:?}");
        // Debug builds are far slower than release; keep a generous ceiling so this catches
        // accidental O(n^2) work without becoming a flaky timing test. The recorded release
        // number is the budget's evidence.
        assert!(cold < std::time::Duration::from_millis(500), "cold flatten {cold:?}");
        assert!(filter_time < std::time::Duration::from_millis(500), "filter {filter_time:?}");
    }
}
