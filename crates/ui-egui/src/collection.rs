//! Collection and folder detail tabs: Overview, Authorization, Variables,
//! Scripts and Runs shells with the inheritance dots. The editor mutates the collection (or a
//! folder inside it) and returns [`CollectionAction::Save`] on changes; the app writes through
//! the engine atomically, so secrets split into the secrets file as usual.

use egui::{Align, Layout, RichText, ScrollArea, Ui, vec2};
use postguy_model::{AuthConfig, Collection, Folder, Id, Item, Scripts, Variable};

use crate::auth::auth_editor;
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
}

/// Intents the app applies through the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionAction {
    Save,
    Duplicate,
    Export,
    Delete,
    Run,
    NewRequest,
    NewFolder,
    Close,
}

/// Detail editor state (kept per tab by the app).
#[derive(Debug, Clone, Default)]
pub struct DetailState {
    pub tab: DetailTab,
    pub overview_preview: bool,
    pub variable_search: String,
}

/// The editor widget.
pub struct CollectionDetail<'a> {
    pub collection: &'a mut Collection,
    /// None = collection detail; Some = this folder inside the collection.
    pub folder: Option<Id>,
    pub state: &'a mut DetailState,
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
            ] {
                if is_folder && tab == DetailTab::Runs {
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
            DetailTab::Runs => {
                ui.label(RichText::new("No runs yet. The runner is not implemented yet.").color(theme.palette.text_muted));
            }
        });
        if changed {
            action = Some(CollectionAction::Save);
        }
        action
    }
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
