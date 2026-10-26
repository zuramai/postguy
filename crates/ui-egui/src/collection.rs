//! Collection and folder detail tabs: Overview, Authorization, Variables,
//! Scripts and Runs shells with the inheritance dots. The editor mutates the collection (or a
//! folder inside it) and returns [`CollectionAction::Save`] on changes; the app writes through
//! the engine atomically, so secrets split into the secrets file as usual.

use egui::{Align, Layout, RichText, ScrollArea, Ui, vec2};
use postguy_model::{AuthConfig, Collection, Folder, Id, Item, Scripts, Variable};

use crate::auth::auth_editor;
use crate::runner::RunSummaryView;
use crate::theme::Theme;
use crate::widgets::{self, menu_item};

/// The detail sub-tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailTab {
    #[default]
    Overview,
    Authorization,
    Variables,
    Scripts,
    Runs,
    /// Collection-level files for `{{% file('…') %}}` shortcodes and `require`.
    Assets,
}

/// Intents the app applies through the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionAction {
    Save,
    Duplicate,
    Export,
    Delete,
    Run,
    /// Open a stored run (by run id) in the Runner tab.
    OpenRun(String),
    NewRequest,
    NewFolder,
    Close,
    /// Pick files (or a folder) and import them into the collection's assets.
    ImportAssets {
        folder: bool,
    },
    DeleteAsset(String),
    RenameAsset {
        from: String,
        to: String,
    },
}

/// One asset in the Assets tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRow {
    pub path: String,
    pub size: u64,
    pub modified_ms: i64,
}

/// Detail editor state (kept per tab by the app).
#[derive(Debug, Clone, Default)]
pub struct DetailState {
    pub tab: DetailTab,
    pub overview_preview: bool,
    pub variable_search: String,
    pub asset_search: String,
    /// `(path being renamed, edit buffer)`.
    pub asset_rename: Option<(String, String)>,
    /// The asset whose trash button was pressed and now awaits "Delete" or "Cancel".
    pub asset_delete_armed: Option<String>,
}

/// The editor widget.
pub struct CollectionDetail<'a> {
    pub collection: &'a mut Collection,
    /// None = collection detail; Some = this folder inside the collection.
    pub folder: Option<Id>,
    pub state: &'a mut DetailState,
    /// Stored runs of the collection, newest first (the Runs tab).
    pub runs: &'a [RunSummaryView],
    /// The collection's assets; `None` while they load.
    pub assets: Option<&'a [AssetRow]>,
}

struct Parts<'a> {
    name: &'a mut String,
    description: &'a mut String,
    variables: &'a mut Vec<Variable>,
    auth: &'a mut Option<AuthConfig>,
    scripts: &'a mut Scripts,
}

fn parts<'a>(collection: &'a mut Collection, folder: &Option<Id>) -> Option<Parts<'a>> {
    match folder {
        None => Some(Parts {
            name: &mut collection.name,
            description: &mut collection.description,
            variables: &mut collection.variables,
            auth: &mut collection.auth,
            scripts: &mut collection.scripts,
        }),
        Some(id) => match find_folder_mut(&mut collection.items, id) {
            Some(folder) => Some(Parts {
                name: &mut folder.name,
                description: &mut folder.description,
                variables: &mut folder.variables,
                auth: &mut folder.auth,
                scripts: &mut folder.scripts,
            }),
            None => None,
        },
    }
}

fn find_folder_mut<'a>(items: &'a mut [Item], id: &Id) -> Option<&'a mut Folder> {
    let mut stack: Vec<&'a mut Item> = items.iter_mut().collect();
    while let Some(item) = stack.pop() {
        match item {
            Item::Folder(folder) => {
                if folder.id == *id {
                    return Some(folder);
                }
                stack.extend(folder.items.iter_mut());
            }
            Item::Request(_) => {}
        }
    }
    None
}

impl CollectionDetail<'_> {
    pub fn show(&mut self, ui: &mut Ui) -> Option<CollectionAction> {
        let theme = Theme::current(ui.ctx());
        let (folders, requests) =
            self.folder.as_ref().map(|id| counts_in_folder(self.collection, id)).unwrap_or_else(|| counts(&self.collection.items));
        let is_folder = self.folder.is_some();
        let mut action = None;
        let mut changed = false;
        let Some(parts) = parts(self.collection, &self.folder) else {
            ui.label(RichText::new("This folder no longer exists.").color(theme.palette.danger));
            return None;
        };
        ui.horizontal(|ui| {
            let icon_rect = egui::Rect::from_min_size(ui.cursor().min, vec2(18.0, 18.0));
            crate::icons::paint(ui.painter(), icon_rect, "folder", theme.palette.text_muted, 1.4);
            ui.add_space(22.0);
            let name = ui.add(egui::TextEdit::singleline(parts.name).frame(egui::Frame::NONE).font(egui::TextStyle::Heading));
            name.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Detail name"));
            if name.changed() {
                changed = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let menu = ui.add(egui::Button::new("...").frame(false));
                egui::Popup::menu(&menu).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
                    ui.set_min_width(190.0);
                    if menu_item(ui, Some("plus"), "New request", None, true).clicked() {
                        action = Some(CollectionAction::NewRequest);
                    }
                    if !is_folder && menu_item(ui, Some("plus"), "New folder", None, true).clicked() {
                        action = Some(CollectionAction::NewFolder);
                    }
                    ui.separator();
                    if menu_item(ui, None, "Duplicate", None, true).clicked() {
                        action = Some(CollectionAction::Duplicate);
                    }
                    if menu_item(ui, None, "Export", None, true).clicked() {
                        action = Some(CollectionAction::Export);
                    }
                    if menu_item(ui, None, "Delete", None, true).clicked() {
                        action = Some(CollectionAction::Delete);
                    }
                });
                if ui.add(egui::Button::new("Run")).clicked() {
                    action = Some(CollectionAction::Run);
                }
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            for (tab, label, dot) in [
                (DetailTab::Overview, "Overview", false),
                (DetailTab::Authorization, "Authorization", parts.auth.as_ref().is_some_and(AuthConfig::resolves)),
                (DetailTab::Variables, "Variables", !parts.variables.is_empty()),
                (DetailTab::Scripts, "Scripts", !parts.scripts.is_empty()),
                (DetailTab::Runs, "Runs", false),
                (DetailTab::Assets, "Assets", self.assets.is_some_and(|rows| !rows.is_empty())),
            ] {
                if is_folder && matches!(tab, DetailTab::Runs | DetailTab::Assets) {
                    continue;
                }
                let selected = self.state.tab == tab;
                let mut rich = RichText::new(label).color(if selected { theme.palette.text } else { theme.palette.text_muted });
                if selected {
                    rich = rich.strong();
                }
                let response = ui.add(egui::Button::new(rich).frame(false));
                if response.clicked() {
                    self.state.tab = tab;
                }
                if dot {
                    ui.painter().circle_filled(response.rect.right_center() + vec2(2.0, -6.0), 2.5, theme.palette.success);
                }
                if selected {
                    ui.painter().line_segment(
                        [egui::pos2(response.rect.left(), response.rect.bottom()), egui::pos2(response.rect.right(), response.rect.bottom())],
                        egui::Stroke::new(2.0, theme.palette.accent),
                    );
                }
            }
        });
        ui.separator();
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.state.tab {
            DetailTab::Overview => {
                ui.label(RichText::new(format!("{folders} folder(s) · {requests} request(s)")).small().color(theme.palette.text_muted));
                ui.add_space(6.0);
                widgets::checkbox(ui, &mut self.state.overview_preview, "Preview");
                if self.state.overview_preview {
                    ui.add_space(4.0);
                    ui.label(parts.description.as_str());
                } else if ui
                    .add(egui::TextEdit::multiline(parts.description).hint_text("Markdown description").desired_rows(16).desired_width(f32::INFINITY))
                    .changed()
                {
                    changed = true;
                }
            }
            DetailTab::Authorization => {
                let default_auth = if is_folder { AuthConfig::Inherit } else { AuthConfig::NoAuth };
                if auth_editor(ui, parts.auth.get_or_insert(default_auth), is_folder, None).changed {
                    changed = true;
                }
                if is_folder {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Requests in this folder inherit this auth unless they set their own.").small().color(theme.palette.text_muted),
                    );
                }
            }
            DetailTab::Variables => {
                if variable_table(ui, parts.variables, &mut self.state.variable_search) {
                    changed = true;
                }
            }
            DetailTab::Scripts => {
                use crate::script_snippets::{POST_RESPONSE, PRE_REQUEST, script_editor};
                changed |= script_editor(
                    ui,
                    &theme,
                    "Pre-request",
                    egui::Id::new(("detail-script", "Pre-request")),
                    8,
                    &mut parts.scripts.prerequest,
                    PRE_REQUEST,
                );
                changed |= script_editor(
                    ui,
                    &theme,
                    "Post-response",
                    egui::Id::new(("detail-script", "Post-response")),
                    8,
                    &mut parts.scripts.test,
                    POST_RESPONSE,
                );
            }
            DetailTab::Assets => {
                if let Some(asset_action) = assets_tab(ui, &theme, self.assets, self.state) {
                    action = Some(asset_action);
                }
            }
            DetailTab::Runs => {
                if self.runs.is_empty() {
                    ui.label(RichText::new("No runs yet. Press Run to run this collection.").color(theme.palette.text_muted));
                }
                for run in self.runs {
                    let response = egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format_time(run.started_ms)).small().color(theme.palette.text_muted));
                                let (text, color) = if run.ok { ("PASS", theme.palette.success) } else { ("FAIL", theme.palette.danger) };
                                widgets::badge(ui, text, color, color.gamma_multiply(0.12));
                                ui.label(RichText::new(format!("{} passed, {} failed", run.passed, run.failed)).color(theme.palette.text));
                                ui.label(
                                    RichText::new(format!("{} iterations · {} ms", run.iterations, run.duration_ms))
                                        .small()
                                        .color(theme.palette.text_muted),
                                );
                            });
                        })
                        .response
                        .interact(egui::Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if response.clicked() {
                        action = Some(CollectionAction::OpenRun(run.id.clone()));
                    }
                    ui.separator();
                }
            }
        });
        if changed {
            action = Some(CollectionAction::Save);
        }
        action
    }
}

const ASSET_ROW: f32 = 30.0;

/// The shortcode that embeds `path` as base64 in a body.
fn asset_shortcode(path: &str) -> String {
    let escaped = path.replace('\\', "\\\\").replace('\'', "\\'");
    format!("{{{{% base64(file('{escaped}')) %}}}}")
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS.get(unit).copied().unwrap_or("TB"))
}

fn assets_tab(ui: &mut Ui, theme: &Theme, assets: Option<&[AssetRow]>, state: &mut DetailState) -> Option<CollectionAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        if widgets::text_button(ui, widgets::ButtonKind::Secondary, Some("import"), "Import files…", true).clicked() {
            action = Some(CollectionAction::ImportAssets { folder: false });
        }
        if widgets::text_button(ui, widgets::ButtonKind::Secondary, Some("folder-open"), "Import folder…", true).clicked() {
            action = Some(CollectionAction::ImportAssets { folder: true });
        }
        ui.add(egui::TextEdit::singleline(&mut state.asset_search).hint_text("Search assets…").desired_width(220.0));
    });
    ui.add_space(6.0);
    let Some(rows) = assets else {
        ui.label(RichText::new("Loading assets…").color(theme.palette.text_muted));
        return action;
    };
    if rows.is_empty() {
        ui.label(
            RichText::new("No assets yet. Import images or documents, then use {{% base64(file('name.jpg')) %}} in a request body.")
                .color(theme.palette.text_muted),
        );
        return action;
    }
    let needle = state.asset_search.trim().to_lowercase();
    let visible: Vec<&AssetRow> = rows.iter().filter(|row| needle.is_empty() || row.path.to_lowercase().contains(&needle)).collect();
    let now = postguy_util::time::now_unix_ms();
    let offset = postguy_util::time::local_offset_seconds(now);
    let muted = theme.palette.text_muted;
    let (header, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), egui::Sense::hover());
    let columns = asset_columns(header);
    for (rect, label) in columns.iter().zip(["Path", "Size", "Modified"]) {
        ui.painter().text(egui::pos2(rect.left() + 8.0, header.center().y), egui::Align2::LEFT_CENTER, label, theme.semibold_font(12.0), muted);
    }
    ui.painter().hline(header.x_range(), header.bottom() - 0.5, egui::Stroke::new(1.0, theme.palette.border));
    if visible.is_empty() {
        ui.label(RichText::new("No assets match the search.").color(muted));
        return action;
    }
    if state.asset_delete_armed.is_some() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        state.asset_delete_armed = None;
    }
    ScrollArea::vertical().id_salt("assets-rows").auto_shrink([false, false]).show_rows(ui, ASSET_ROW, visible.len(), |ui, range| {
        for index in range {
            let Some(row) = visible.get(index) else { continue };
            let (rect, row_response) = ui.allocate_exact_size(vec2(ui.available_width(), ASSET_ROW), egui::Sense::click());
            let columns = asset_columns(rect);
            let armed = state.asset_delete_armed.as_deref() == Some(row.path.as_str());
            if row_response.clicked() && !armed {
                state.asset_delete_armed = None;
            }
            let renaming = state.asset_rename.as_ref().is_some_and(|(from, _)| *from == row.path);
            if renaming {
                let mut commit = false;
                let mut cancel = false;
                if let Some((_, buffer)) = state.asset_rename.as_mut() {
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(columns[0]).layout(Layout::left_to_right(Align::Center)));
                    let edit_id = egui::Id::new(("asset-rename", &row.path));
                    let edit = child.add(egui::TextEdit::singleline(buffer).id(edit_id).desired_width(columns[0].width() - 8.0));
                    if !edit.has_focus() && !edit.lost_focus() {
                        edit.request_focus();
                    }
                    commit = edit.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    cancel = ui.input(|input| input.key_pressed(egui::Key::Escape));
                }
                if commit {
                    if let Some((from, to)) = state.asset_rename.take() {
                        let to = to.trim().to_string();
                        if !to.is_empty() && to != from {
                            action = Some(CollectionAction::RenameAsset { from, to });
                        }
                    }
                } else if cancel {
                    state.asset_rename = None;
                }
            } else {
                let name = row.path.rsplit('/').next().unwrap_or(&row.path);
                let (label, color) = if armed { (format!("Delete {name}?"), theme.palette.danger) } else { (row.path.clone(), theme.palette.text) };
                ui.painter().text(
                    egui::pos2(columns[0].left() + 8.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::TextStyle::Body.resolve(ui.style()),
                    color,
                );
            }
            ui.painter().text(
                egui::pos2(columns[1].left() + 8.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                human_size(row.size),
                egui::TextStyle::Body.resolve(ui.style()),
                muted,
            );
            let modified = postguy_util::time::format_datetime(row.modified_ms, offset, postguy_util::time::DateStyle::Compact, now);
            ui.painter().text(
                egui::pos2(columns[2].left() + 8.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                modified,
                egui::TextStyle::Body.resolve(ui.style()),
                muted,
            );
            let mut buttons = ui.new_child(egui::UiBuilder::new().max_rect(columns[3]).layout(Layout::right_to_left(Align::Center)));
            if armed {
                let small = widgets::ButtonOptions { small: true, height: Some(24.0), ..Default::default() };
                if widgets::text_button_with(&mut buttons, widgets::ButtonKind::Ghost, None, "Cancel", true, small).clicked() {
                    state.asset_delete_armed = None;
                }
                if widgets::text_button_with(&mut buttons, widgets::ButtonKind::Danger, None, "Delete", true, small).clicked() {
                    state.asset_delete_armed = None;
                    action = Some(CollectionAction::DeleteAsset(row.path.clone()));
                }
            } else {
                if widgets::icon_button(&mut buttons, "trash", "Delete asset").clicked() {
                    state.asset_delete_armed = Some(row.path.clone());
                }
                if widgets::text_button_with(
                    &mut buttons,
                    widgets::ButtonKind::Ghost,
                    None,
                    "Rename",
                    true,
                    widgets::ButtonOptions { small: true, height: Some(24.0), ..Default::default() },
                )
                .clicked()
                {
                    state.asset_rename = Some((row.path.clone(), row.path.clone()));
                }
                if widgets::icon_button(&mut buttons, "copy", "Copy shortcode").clicked() {
                    buttons.ctx().copy_text(asset_shortcode(&row.path));
                }
            }
            ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, egui::Stroke::new(1.0, theme.palette.border));
        }
    });
    action
}

/// Path, Size, Modified and the button strip of one row (or the header).
fn asset_columns(rect: egui::Rect) -> [egui::Rect; 4] {
    let buttons = 150.0_f32.min(rect.width() * 0.3);
    let rest = (rect.width() - buttons).max(120.0);
    let (size, modified) = (90.0_f32.min(rest * 0.2), 120.0_f32.min(rest * 0.25));
    let path = rest - size - modified;
    let at = |left: f32, width: f32| egui::Rect::from_min_max(egui::pos2(left, rect.top()), egui::pos2(left + width, rect.bottom()));
    [at(rect.left(), path), at(rect.left() + path, size), at(rect.left() + path + size, modified), at(rect.left() + rest, buttons)]
}

fn variable_table(ui: &mut Ui, variables: &mut Vec<Variable>, search: &mut String) -> bool {
    ui.add(egui::TextEdit::singleline(search).hint_text("Search variables…").desired_width(220.0));
    ui.add_space(6.0);
    let needle = search.trim().to_ascii_lowercase();
    let grid = widgets::VariableGrid { salt: "collection-variables", show_description: true, reveal: false, search: &needle };
    ScrollArea::vertical().max_height(420.0).show(ui, |ui| widgets::variable_table(ui, variables, &grid)).inner
}

fn counts(items: &[Item]) -> (usize, usize) {
    let mut folders = 0;
    let mut requests = 0;
    let mut stack: Vec<&Item> = items.iter().collect();
    while let Some(item) = stack.pop() {
        match item {
            Item::Folder(folder) => {
                folders += 1;
                stack.extend(folder.items.iter());
            }
            Item::Request(_) => requests += 1,
        }
    }
    (folders, requests)
}

fn counts_in_folder(collection: &Collection, folder: &Id) -> (usize, usize) {
    // Read-only search for the folder's subtree.
    fn find<'a>(items: &'a [Item], id: &Id) -> Option<&'a Folder> {
        let mut stack: Vec<&Item> = items.iter().collect();
        while let Some(item) = stack.pop() {
            match item {
                Item::Folder(folder) => {
                    if folder.id == *id {
                        return Some(folder);
                    }
                    stack.extend(folder.items.iter());
                }
                Item::Request(_) => {}
            }
        }
        None
    }
    find(&collection.items, folder).map(|folder| counts(&folder.items)).unwrap_or((0, 0))
}

/// A small outline icon rect helper for tests/toolbars.
pub fn icon_rect(pos: egui::Pos2) -> egui::Rect {
    egui::Rect::from_min_size(pos, vec2(18.0, 18.0))
}

/// Relative age ("4m ago"); absolute dates arrive with the history list.
fn format_time(millis: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|duration| duration.as_millis() as i64).unwrap_or(0);
    let seconds = now.saturating_sub(millis) / 1000;
    if seconds < 60 {
        format!("{seconds}s ago")
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_walk_nested_folders() {
        let folder = Folder {
            id: Id("f".into()),
            name: "f".into(),
            items: vec![Item::Request(Box::new(postguy_model::Request {
                id: Id("r".into()),
                name: "r".into(),
                kind: postguy_model::RequestKind::Http(Default::default()),
                ..Default::default()
            }))],
            ..Default::default()
        };
        let collection = Collection { id: Id("c".into()), name: "c".into(), items: vec![Item::Folder(Box::new(folder))], ..Default::default() };
        assert_eq!(counts(&collection.items), (1, 1));
        assert_eq!(counts_in_folder(&collection, &Id("f".into())), (0, 1));
        assert_eq!(counts_in_folder(&collection, &Id("missing".into())), (0, 0));
    }
}
