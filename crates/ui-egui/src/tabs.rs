//! The tab strip (PROMPT.md 6.2): open documents with preview vs permanent tabs, a dirty dot,
//! close/reopen, drag reorder, middle-click close, an overflow search (open and recently closed)
//! and the per-tab context menu. Closing a dirty tab asks Save / Don't save / Cancel.
//!
//! The model is serialisable so the app can write it into the store's session file every time it
//! changes; `clean_exit` lives next to it for crash recovery. Draft text is kept in a side map
//! (the request editor fills it from M3.4).

use std::collections::BTreeMap;

use egui::{Align, Color32, Id, Key, Layout, RichText, ScrollArea, Sense, Ui, vec2};
use serde::{Deserialize, Serialize};

use crate::sidebar::SidebarData;
use crate::theme::Theme;
use crate::widgets::{self, menu_item};

/// What a tab shows. Ids are strings because the workspace files already use UUID text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TabDoc {
    Welcome,
    Settings,
    Cookies,
    Trash,
    Runner,
    Request { collection: String, request: String, method: String, protocol: String },
    Collection { id: String },
    Folder { collection: String, folder: String },
    Environment { id: String },
    Globals,
    History { id: String },
}

impl TabDoc {
    /// Stable identity for duplicate detection.
    pub fn key(&self) -> String {
        match self {
            TabDoc::Welcome => "welcome".into(),
            TabDoc::Settings => "settings".into(),
            TabDoc::Cookies => "cookies".into(),
            TabDoc::Trash => "trash".into(),
            TabDoc::Runner => "runner".into(),
            TabDoc::Request { collection, request, .. } => format!("request:{collection}:{request}"),
            TabDoc::Collection { id } => format!("collection:{id}"),
            TabDoc::Folder { collection, folder } => format!("folder:{collection}:{folder}"),
            TabDoc::Environment { id } => format!("environment:{id}"),
            TabDoc::Globals => "globals".into(),
            TabDoc::History { id } => format!("history:{id}"),
        }
    }

    /// The sidebar row this tab reveals, when there is one.
    pub fn sidebar_id(&self) -> Option<&str> {
        match self {
            TabDoc::Request { request, .. } => Some(request),
            TabDoc::Collection { id } => Some(id),
            TabDoc::Folder { folder, .. } => Some(folder),
            TabDoc::Environment { id } => Some(id),
            TabDoc::History { id } => Some(id),
            _ => None,
        }
    }

    pub fn method(&self) -> Option<&str> {
        match self {
            TabDoc::Request { method, protocol, .. } => Some(if protocol == "http" {
                method
            } else if protocol == "grpc" {
                "gRPC"
            } else {
                "WS"
            }),
            _ => None,
        }
    }

    pub fn is_request(&self) -> bool {
        matches!(self, TabDoc::Request { .. })
    }
}

/// One open document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    pub id: u64,
    pub doc: TabDoc,
    pub title: String,
    /// Preview tabs are italic and replaced by the next preview (PROMPT.md 5.1).
    #[serde(default)]
    pub preview: bool,
    /// Unsaved edits (the dirty dot replaces the close button until hover).
    #[serde(default)]
    pub dirty: bool,
    #[serde(default = "true_value")]
    pub closable: bool,
}

fn true_value() -> bool {
    true
}

/// What the strip wants the app to do beyond its own state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabAction {
    NewRequest,
    RevealInSidebar(String),
    /// The user chose Save while closing `id`; the app persists the draft, then the tab closes.
    SaveAndClose(u64),
    SetActiveEnvironment(Option<String>),
    /// Create an environment and open its editor.
    CreateEnvironment,
    /// A closed-tab reopen happened (the app may need the workspace to resolve the document).
    Restored(TabDoc),
}

/// A pending dirty-close decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseChoice {
    Save,
    Discard,
    Cancel,
}

/// The tab model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tabs {
    #[serde(default)]
    pub tabs: Vec<Tab>,
    #[serde(default)]
    pub active: Option<u64>,
    /// Most-recently closed first, capped at 20.
    #[serde(default)]
    pub closed: Vec<Tab>,
    #[serde(default)]
    next_id: u64,
    /// Close confirmation for a dirty tab (not persisted: a restart forgets the dialog).
    #[serde(skip)]
    pub pending_close: Option<u64>,
    #[serde(skip)]
    drag: Option<u64>,
    /// Draft text per tab, written by the editors from M3.4.
    #[serde(default)]
    pub drafts: BTreeMap<u64, String>,
}

impl Tabs {
    pub fn new() -> Tabs {
        Tabs::default()
    }

    /// Open as a preview (single click): replaces an existing preview, never duplicates.
    pub fn open_preview(&mut self, doc: TabDoc, title: impl Into<String>) -> u64 {
        if let Some(id) = self.find(&doc) {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                tab.preview = true;
            }
            self.active = Some(id);
            return id;
        }
        if let Some(preview) = self.tabs.iter().position(|tab| tab.preview) {
            let removed = self.tabs.remove(preview);
            self.closed.insert(0, removed);
        }
        self.push(doc, title.into(), true)
    }

    /// Open permanently (double-click, F2, send): promotes an existing tab instead of adding.
    pub fn open(&mut self, doc: TabDoc, title: impl Into<String>) -> u64 {
        if let Some(id) = self.find(&doc) {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                tab.preview = false;
            }
            self.active = Some(id);
            return id;
        }
        self.push(doc, title.into(), false)
    }

    fn push(&mut self, doc: TabDoc, title: String, preview: bool) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        self.tabs.push(Tab { id, doc, title, preview, dirty: false, closable: true });
        self.active = Some(id);
        id
    }

    pub fn find(&self, doc: &TabDoc) -> Option<u64> {
        let key = doc.key();
        self.tabs.iter().find(|tab| tab.doc.key() == key).map(|tab| tab.id)
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|id| self.tabs.iter().find(|tab| tab.id == id))
    }

    pub fn activate(&mut self, id: u64) {
        if self.tabs.iter().any(|tab| tab.id == id) {
            self.active = Some(id);
        }
    }

    pub fn set_dirty(&mut self, id: u64, dirty: bool) {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.dirty = dirty;
        }
    }

    pub fn set_title(&mut self, id: u64, title: String) {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.title = title;
        }
    }

    pub fn set_draft(&mut self, id: u64, text: String) {
        self.drafts.insert(id, text);
    }

    pub fn clear_draft(&mut self, id: u64) {
        self.drafts.remove(&id);
    }

    /// Request a close. A dirty tab returns false and opens the dialog; a clean tab closes now.
    pub fn request_close(&mut self, id: u64) -> bool {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) else { return true };
        if tab.dirty {
            self.pending_close = Some(id);
            return false;
        }
        self.close_now(id);
        true
    }

    pub fn close_now(&mut self, id: u64) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.id == id) {
            let mut tab = self.tabs.remove(index);
            tab.preview = false;
            self.closed.insert(0, tab);
            self.closed.truncate(20);
            self.drafts.remove(&id);
            if self.active == Some(id) {
                self.active = self.tabs.get(index.min(self.tabs.len().saturating_sub(1))).map(|tab| tab.id);
            }
        }
        self.pending_close = None;
    }

    pub fn force_close(&mut self, id: u64) {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.dirty = false;
        }
        self.close_now(id);
    }

    pub fn close_others(&mut self, keep: u64) {
        let ids: Vec<u64> = self.tabs.iter().map(|tab| tab.id).filter(|id| *id != keep).collect();
        for id in ids {
            self.force_close(id);
        }
        self.active = Some(keep);
    }

    pub fn close_all(&mut self) {
        let ids: Vec<u64> = self.tabs.iter().map(|tab| tab.id).collect();
        for id in ids {
            self.force_close(id);
        }
        self.active = None;
    }

    /// Reopen the most recently closed tab.
    pub fn reopen_last(&mut self) -> Option<&Tab> {
        let mut tab = self.closed.first().cloned()?;
        self.closed.remove(0);
        tab.preview = false;
        if let Some(existing) = self.find(&tab.doc) {
            self.active = Some(existing);
            return self.tabs.iter().find(|candidate| candidate.id == existing);
        }
        // Closed tabs get their old id back when it is still free; otherwise a fresh one.
        if self.tabs.iter().any(|candidate| candidate.id == tab.id) {
            self.next_id = self.next_id.saturating_add(1);
            tab.id = self.next_id;
        }
        let id = tab.id;
        self.tabs.push(tab);
        self.active = Some(id);
        self.tabs.last()
    }

    /// Move the tab at `from` to `to` (drag reorder).
    pub fn reorder(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
    }

    pub fn activate_next(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let index = self.active.and_then(|id| self.tabs.iter().position(|tab| tab.id == id)).unwrap_or(0);
        let next = (index + 1) % self.tabs.len();
        self.active = self.tabs.get(next).map(|tab| tab.id);
    }

    pub fn activate_previous(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let index = self.active.and_then(|id| self.tabs.iter().position(|tab| tab.id == id)).unwrap_or(0);
        let previous = if index == 0 { self.tabs.len() - 1 } else { index - 1 };
        self.active = self.tabs.get(previous).map(|tab| tab.id);
    }

    /// Resolve the pending close dialog's choice.
    pub fn resolve_close(&mut self, choice: CloseChoice) -> Option<u64> {
        let id = self.pending_close?;
        self.pending_close = None;
        match choice {
            CloseChoice::Save => {
                self.force_close(id);
                Some(id)
            }
            CloseChoice::Discard => {
                self.force_close(id);
                None
            }
            CloseChoice::Cancel => None,
        }
    }

    /// Drop tabs whose document no longer resolves in the workspace (crash/session recovery).
    pub fn retain_valid(&mut self, valid: impl Fn(&TabDoc) -> bool) -> usize {
        let removed: Vec<u64> = self.tabs.iter().filter(|tab| !valid(&tab.doc)).map(|tab| tab.id).collect();
        for id in &removed {
            self.close_now(*id);
        }
        if self.active.is_some_and(|id| !self.tabs.iter().any(|tab| tab.id == id)) {
            self.active = self.tabs.first().map(|tab| tab.id);
        }
        removed.len()
    }
}

/// Draw the strip. `data` supplies the environment selector; the strip mutates `tabs` and returns
/// at most one app-level action.
pub fn tab_strip(ui: &mut Ui, tabs: &mut Tabs, data: &SidebarData) -> Option<TabAction> {
    let theme = Theme::current(ui.ctx());
    let mut action = None;
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ScrollArea::horizontal().id_salt("tab-strip-scroll").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(
            ui,
            |ui| {
                ui.horizontal_centered(|ui| {
                    let mut hovered_index: Option<usize> = None;
                    let mut drag_index: Option<usize> = None;
                    for index in 0..tabs.tabs.len() {
                        let tab = tabs.tabs[index].clone();
                        let drawn = draw_tab(ui, &theme, &tab, tabs.active == Some(tab.id));
                        let response = drawn.response;
                        if drawn.close_clicked {
                            tabs.request_close(tab.id);
                        }
                        if response.clicked() {
                            tabs.activate(tab.id);
                        }
                        if response.middle_clicked() {
                            tabs.request_close(tab.id);
                        }
                        if response.double_clicked()
                            && let Some(current) = tabs.tabs.get_mut(index)
                        {
                            current.preview = false;
                        }
                        if response.drag_started() {
                            tabs.drag = Some(tab.id);
                        }
                        if response.dragged() {
                            drag_index = Some(index);
                        }
                        if tabs.drag.is_some() && response.hovered() {
                            hovered_index = Some(index);
                        }
                        let tab_id = tab.id;
                        widgets::context_menu(&response, |ui| {
                            if menu_item(ui, Some("plus"), "New Request", None, true).clicked() {
                                action = Some(TabAction::NewRequest);
                            }
                            if menu_item(ui, None, "Duplicate Tab", None, true).clicked() {
                                let doc = tabs.tabs.iter().find(|candidate| candidate.id == tab_id).map(|candidate| candidate.doc.clone());
                                if let Some(doc) = doc {
                                    tabs.open(doc, tabs.tabs.iter().find(|tab| tab.id == tab_id).map(|tab| tab.title.clone()).unwrap_or_default());
                                }
                            }
                            ui.separator();
                            if menu_item(ui, None, "Close Tab", Some("Cmd+W"), true).clicked() {
                                tabs.request_close(tab_id);
                            }
                            if menu_item(ui, None, "Force Close Tab", None, true).clicked() {
                                tabs.force_close(tab_id);
                            }
                            if menu_item(ui, None, "Close Other Tabs", None, true).clicked() {
                                tabs.close_others(tab_id);
                            }
                            if menu_item(ui, None, "Close All Tabs", None, true).clicked() {
                                tabs.close_all();
                            }
                            if menu_item(ui, None, "Force Close All Tabs", None, true).clicked() {
                                let ids: Vec<u64> = tabs.tabs.iter().map(|tab| tab.id).collect();
                                for id in ids {
                                    tabs.force_close(id);
                                }
                            }
                            ui.separator();
                            if let Some(doc) = tabs.tabs.iter().find(|tab| tab.id == tab_id).map(|tab| tab.doc.clone())
                                && let Some(sidebar_id) = doc.sidebar_id()
                                && menu_item(ui, None, "Reveal in Sidebar", None, true).clicked()
                            {
                                action = Some(TabAction::RevealInSidebar(sidebar_id.to_string()));
                            }
                        });
                    }
                    if let (Some(dragged), Some(target)) = (tabs.drag, hovered_index)
                        && let Some(from) = tabs.tabs.iter().position(|tab| Some(tab.id) == Some(dragged))
                    {
                        tabs.reorder(from, target);
                    }
                    if ui.input(|input| input.pointer.any_released()) {
                        tabs.drag = None;
                    }
                    let _ = drag_index;
                });
            },
        );

        // Overflow search (open + recently closed) and the environment selector.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let active_name = data
                .active_environment
                .as_ref()
                .and_then(|id| data.environments.iter().find(|environment| environment.id == *id))
                .map(|environment| environment.name.clone())
                .unwrap_or_else(|| "No environment".to_string());
            let quick = ui.add(egui::Button::new("Vars").frame(false)).on_hover_text("Variables quick look");
            egui::Popup::menu(&quick).show(|ui| {
                ui.set_min_width(340.0);
                let collection_id = tabs.active_tab().and_then(|tab| match &tab.doc {
                    TabDoc::Request { collection, .. } => Some(collection.clone()),
                    _ => None,
                });
                let groups = vec![
                    crate::environment::VariableGroup {
                        title: "Environment".into(),
                        variables: data
                            .active_environment
                            .as_ref()
                            .and_then(|id| data.environments.iter().find(|environment| environment.id == *id))
                            .map(|environment| {
                                environment
                                    .variables
                                    .iter()
                                    .filter(|variable| variable.enabled)
                                    .map(|variable| (variable.key.clone(), variable.value.clone(), variable.secret))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    },
                    crate::environment::VariableGroup {
                        title: "Globals".into(),
                        variables: data
                            .globals
                            .variables
                            .iter()
                            .filter(|variable| variable.enabled)
                            .map(|variable| (variable.key.clone(), variable.value.clone(), variable.secret))
                            .collect(),
                    },
                    crate::environment::VariableGroup {
                        title: "Collection".into(),
                        variables: collection_id
                            .and_then(|id| data.collections.iter().find(|collection| collection.id.0 == id))
                            .map(|collection| {
                                collection
                                    .variables
                                    .iter()
                                    .filter(|variable| variable.enabled)
                                    .map(|variable| (variable.key.clone(), variable.value.clone(), variable.secret))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    },
                ];
                crate::environment::variable_quick_look(ui, &groups);
            });
            let response = ui.add(egui::Button::new(RichText::new(format!("{active_name}  ·")).color(theme.palette.text)));
            egui::Popup::menu(&response).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                ui.set_min_width(240.0);
                ui.label(RichText::new("Choose an environment for every request; pinning is P2.").small().color(theme.palette.text_muted));
                let current = data.active_environment.as_ref().map(|id| id.0.clone());
                let filter_key = egui::Id::new("env-selector-filter");
                let mut filter = ui.data(|data| data.get_temp::<String>(filter_key)).unwrap_or_default();
                if ui.add(egui::TextEdit::singleline(&mut filter).hint_text("Search environments…").desired_width(f32::INFINITY)).changed() {
                    ui.data_mut(|data| data.insert_temp(filter_key, filter.clone()));
                }
                let needle = filter.to_ascii_lowercase();
                if needle.is_empty()
                    && ui
                        .add(egui::Button::new("No environment").selected(current.is_none()).frame(false).min_size(vec2(ui.available_width(), 24.0)))
                        .clicked()
                {
                    action = Some(TabAction::SetActiveEnvironment(None));
                    ui.close();
                }
                for environment in &data.environments {
                    if !needle.is_empty() && !environment.name.to_ascii_lowercase().contains(&needle) {
                        continue;
                    }
                    let selected = current.as_deref() == Some(environment.id.0.as_str());
                    if ui
                        .add(
                            egui::Button::new(format!("{}  {}", if selected { "✓" } else { " " }, environment.name))
                                .selected(selected)
                                .frame(false)
                                .min_size(vec2(ui.available_width(), 24.0)),
                        )
                        .clicked()
                    {
                        action = Some(TabAction::SetActiveEnvironment(Some(environment.id.0.clone())));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.add(egui::Button::new("+ Create new environment").frame(false).min_size(vec2(ui.available_width(), 24.0))).clicked() {
                    action = Some(TabAction::CreateEnvironment);
                    ui.close();
                }
            });
            let overflow = ui.add(egui::Button::new("Tabs").frame(false)).on_hover_text("Search tabs");
            egui::Popup::menu(&overflow).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
                ui.set_min_width(260.0);
                ui.label(RichText::new("Open tabs").small().color(theme.palette.text_muted));
                let open_tabs: Vec<Tab> = tabs.tabs.clone();
                for tab in open_tabs {
                    if ui
                        .add(
                            egui::Button::new(format!(
                                "{} {}",
                                method_badge(&theme, &tab.doc).map(|(_, badge)| badge).unwrap_or_default(),
                                tab.title
                            ))
                            .selected(tabs.active == Some(tab.id))
                            .frame(false)
                            .min_size(vec2(ui.available_width(), 24.0)),
                        )
                        .clicked()
                    {
                        tabs.activate(tab.id);
                        ui.close();
                    }
                }
                if !tabs.closed.is_empty() {
                    ui.separator();
                    ui.label(RichText::new("Recently closed").small().color(theme.palette.text_muted));
                    let recent: Vec<Tab> = tabs.closed.iter().take(10).cloned().collect();
                    for tab in recent {
                        if ui.add(egui::Button::new(tab.title.clone()).frame(false).min_size(vec2(ui.available_width(), 24.0))).clicked() {
                            if let Some(reopened) = tabs.reopen_last() {
                                action = Some(TabAction::Restored(reopened.doc.clone()));
                            }
                            ui.close();
                        }
                    }
                }
            });
            ui.add_space(4.0);
        });
    });

    // Dirty-close dialog (its own modal: a click that opened it must not count as a backdrop
    // dismissal, so closing without a choice is impossible).
    if let Some(id) = tabs.pending_close {
        let title = tabs.tabs.iter().find(|tab| tab.id == id).map(|tab| tab.title.clone()).unwrap_or_default();
        let mut choice: Option<CloseChoice> = None;
        egui::Modal::new(Id::new(("close-tab", id))).show(ui.ctx(), |ui| {
            ui.set_min_width(360.0);
            ui.heading("Unsaved changes");
            ui.separator();
            ui.label(format!("`{title}` has unsaved changes."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    choice = Some(CloseChoice::Cancel);
                }
                if ui.button("Don't save").clicked() {
                    choice = Some(CloseChoice::Discard);
                }
                if ui.add(egui::Button::new(RichText::new("Save").color(Color32::WHITE)).fill(theme.palette.accent)).clicked() {
                    choice = Some(CloseChoice::Save);
                }
            });
        });
        if let Some(choice) = choice {
            match choice {
                CloseChoice::Save => action = Some(TabAction::SaveAndClose(id)),
                CloseChoice::Discard | CloseChoice::Cancel => {
                    tabs.resolve_close(choice);
                }
            }
        }
    }
    action
}

struct DrawnTab {
    response: egui::Response,
    close_clicked: bool,
}

fn draw_tab(ui: &mut Ui, theme: &Theme, tab: &Tab, active: bool) -> DrawnTab {
    let method = method_badge(theme, &tab.doc);
    let mut text = String::new();
    if let Some((_, badge)) = &method {
        text.push_str(badge);
        text.push(' ');
    }
    text.push_str(&tab.title);
    let width = (text.chars().count() as f32 * 7.2 + 34.0).clamp(90.0, 240.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click_and_drag());
    if active {
        ui.painter().rect_filled(rect, egui::CornerRadius { nw: 5, sw: 0, ne: 5, se: 0 }, theme.palette.surface_alt);
        ui.painter().line_segment(
            [egui::pos2(rect.left() + 6.0, rect.bottom() - 1.0), egui::pos2(rect.right() - 6.0, rect.bottom() - 1.0)],
            egui::Stroke::new(2.0, theme.palette.accent),
        );
    } else if response.hovered() {
        ui.painter().rect_filled(rect, egui::CornerRadius { nw: 5, sw: 0, ne: 5, se: 0 }, theme.palette.surface_alt.gamma_multiply(0.6));
    }
    let mut x = rect.left() + 8.0;
    if let Some((color, badge)) = method {
        ui.painter().text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, badge, egui::TextStyle::Small.resolve(ui.style()), color);
        x += 34.0;
    }
    let color = if active { theme.palette.text } else { theme.palette.text_muted };
    ui.painter().text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, &tab.title, egui::TextStyle::Body.resolve(ui.style()), color);
    if tab.preview {
        // A thin italic-approximation underline keeps preview tabs recognisable in the strip.
        ui.painter().line_segment(
            [egui::pos2(x, rect.bottom() - 7.0), egui::pos2(x + tab.title.chars().count() as f32 * 6.5, rect.bottom() - 7.0)],
            egui::Stroke::new(1.0, theme.palette.text_muted.gamma_multiply(0.5)),
        );
    }
    // Dirty dot replaces the close button until hover (PROMPT.md 6.2).
    let mut close_clicked = false;
    if tab.dirty && !response.hovered() {
        ui.painter().circle_filled(egui::pos2(rect.right() - 12.0, rect.center().y), 3.5, theme.palette.text_muted);
    } else {
        let close = egui::Rect::from_center_size(egui::pos2(rect.right() - 12.0, rect.center().y), egui::Vec2::splat(14.0));
        let close_response = ui.interact(close, Id::new(("tab-close", tab.id)), Sense::click());
        let stroke = egui::Stroke::new(1.2, theme.palette.text_muted);
        ui.painter().line_segment([egui::pos2(close.left() + 4.0, close.top() + 4.0), egui::pos2(close.right() - 4.0, close.bottom() - 4.0)], stroke);
        ui.painter().line_segment([egui::pos2(close.right() - 4.0, close.top() + 4.0), egui::pos2(close.left() + 4.0, close.bottom() - 4.0)], stroke);
        close_response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Close {}", tab.title)));
        close_clicked = close_response.clicked();
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, &tab.title));
    DrawnTab { response, close_clicked }
}

fn method_badge(theme: &Theme, doc: &TabDoc) -> Option<(Color32, String)> {
    let method = doc.method()?;
    Some((theme.method_color(method), method.to_string()))
}

/// The tab strip's fixed height (PROMPT.md 6.2 keeps it compact).
pub const TAB_STRIP_HEIGHT: f32 = 34.0;

/// Keyboard handling for the strip: Cmd/Ctrl+W close, Cmd/Ctrl+Shift+T reopen, Cmd/Ctrl+Tab
/// cycles (the full shortcut table lands in M3.10).
pub fn tab_shortcuts(ui: &Ui, tabs: &mut Tabs) {
    let input = ui.input(|input| (input.modifiers, input.key_pressed(Key::W), input.key_pressed(Key::T), input.key_pressed(Key::Tab)));
    let (modifiers, close, reopen, cycle) = input;
    if modifiers.command
        && close
        && let Some(id) = tabs.active
    {
        tabs.request_close(id);
    }
    if modifiers.command && modifiers.shift && reopen {
        tabs.reopen_last();
    }
    if modifiers.command && cycle {
        tabs.activate_next();
    }
}
