//! The app shell (PROMPT.md 6.2): header, sidebar, tab strip, central panel, optional right
//! panel and bottom bar, built with the widget kit. M3.1 delivers the frame and the theme; the
//! screens (tree, request builder, response viewer) fill it in M3.2-M3.9.
//!
//! The shell owns only layout state (panel sizes, open/closed flags, the active tab label). All
//! document state belongs to the engine façade, which the app passes in when it exists.

use egui::{Align, Layout, RichText};

use crate::collection::CollectionAction;
use crate::commands::{CommandAction, CommandRegistry, PaletteItem, PaletteState, command_palette};
use crate::console::{ConsoleAction, ConsoleRow, ConsoleState, console_view};
use crate::cookies::{CookieAction, CookieData, CookieState};
use crate::environment::EnvironmentAction;
use crate::settings::{SettingsAction, SettingsState};
use crate::sidebar::{RowKind, Sidebar, SidebarAction, SidebarData, SidebarState};
use crate::snippet::{SnippetPanelState, snippet_panel};
use crate::tabs::{self, TAB_STRIP_HEIGHT, TabAction, TabDoc, Tabs};
use crate::trash::TrashAction;

/// What an app-supplied central panel did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelAction {
    Builder(crate::builder::BuilderAction),
    Environment(EnvironmentAction),
    Collection(CollectionAction),
    Trash(TrashAction),
    Cookies(CookieAction),
    Settings(SettingsAction),
}

/// The app-supplied editor shown in the central panel for request/environment/globals tabs.
pub type ContentPanel<'a> = &'a mut dyn FnMut(&mut egui::Ui, &TabDoc) -> Option<PanelAction>;
use crate::theme::{Theme, ThemeMode};
use crate::widgets;

/// Layout state the shell keeps across frames.
#[derive(Debug, Clone)]
pub struct ShellState {
    pub theme_mode: ThemeMode,
    pub sidebar_open: bool,
    pub right_panel_open: bool,
    pub console_open: bool,
    pub two_pane: bool,
    pub status: String,
}

impl Default for ShellState {
    fn default() -> Self {
        ShellState {
            theme_mode: ThemeMode::System,
            sidebar_open: true,
            right_panel_open: false,
            console_open: false,
            two_pane: false,
            status: "Ready".to_string(),
        }
    }
}

/// The shell app. `workspace` is the folder label shown in the header (the engine wires it in
/// M3.2; the skeleton shows "No workspace").
pub struct PostguyShell {
    pub state: ShellState,
    pub workspace_label: String,
    pub sidebar: Sidebar,
    pub sidebar_data: SidebarData,
    pub sidebar_state: SidebarState,
    pub tabs: Tabs,
    /// The action the sidebar emitted this frame; the app applies it to the engine.
    pub action: Option<SidebarAction>,
    /// The action the tab strip emitted this frame.
    pub tab_action: Option<TabAction>,
    pub command_registry: CommandRegistry,
    pub palette: PaletteState,
    /// The command invoked from the palette this frame.
    pub command_action: Option<CommandAction>,
    pub console_rows: Vec<ConsoleRow>,
    pub console_state: ConsoleState,
    pub cookie_data: CookieData,
    pub cookie_state: CookieState,
    pub settings: SettingsState,
    pub settings_value: postguy_model::Settings,
    pub snippet: SnippetPanelState,
    pub snippet_request: Option<postguy_model::HttpRequest>,
}

impl Default for PostguyShell {
    fn default() -> Self {
        PostguyShell::new("No workspace")
    }
}

impl PostguyShell {
    pub fn new(workspace_label: impl Into<String>) -> PostguyShell {
        PostguyShell {
            state: ShellState::default(),
            workspace_label: workspace_label.into(),
            sidebar: Sidebar::new(),
            sidebar_data: SidebarData::default(),
            sidebar_state: SidebarState::default(),
            tabs: Tabs::new(),
            action: None,
            tab_action: None,
            command_registry: CommandRegistry::default(),
            palette: PaletteState::default(),
            command_action: None,
            console_rows: Vec::new(),
            console_state: ConsoleState::default(),
            cookie_data: CookieData::default(),
            cookie_state: CookieState::default(),
            settings: SettingsState::default(),
            settings_value: postguy_model::Settings::default(),
            snippet: SnippetPanelState::default(),
            snippet_request: None,
        }
    }

    /// Draw one frame into the root UI (eframe's `App::ui` and kittest both call this) with no
    /// external request panel.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let _ = self.ui_with(ui, None);
    }

    /// The same frame with an app-supplied central panel (request builder, environment or
    /// globals editor). Returns the panel's action so the app can save/send.
    pub fn ui_with(&mut self, ui: &mut egui::Ui, mut content_panel: Option<ContentPanel<'_>>) -> Option<PanelAction> {
        let ctx = ui.ctx().clone();
        let theme = Theme::from_context(&ctx, self.state.theme_mode).with_code_font(self.settings_value.editor_font_size);
        theme.install(&ctx);
        let metrics = theme.metrics;

        self.header(ui, &theme);
        self.bottom_bar(ui, &theme);
        if self.state.console_open {
            let height = 190.0;
            egui::Panel::bottom("console-panel").exact_size(height).resizable(true).show(ui, |ui| {
                if let Some(ConsoleAction::Clear) = console_view(ui, &self.console_rows, &mut self.console_state) {
                    self.command_action = Some(CommandAction::ClearConsole);
                }
            });
        }
        self.action = None;
        self.tab_action = None;
        self.command_action = None;
        if !ui.ctx().egui_wants_keyboard_input() {
            let mut shortcut: Option<CommandAction> = None;
            ui.input(|input| {
                let command = input.modifiers.command;
                let alt = input.modifiers.alt;
                let shift = input.modifiers.shift;
                if command && input.key_pressed(egui::Key::K) {
                    self.palette.open = true;
                }
                if command && input.key_pressed(egui::Key::Comma) {
                    shortcut = Some(CommandAction::OpenSettings);
                }
                if command && alt && input.key_pressed(egui::Key::C) {
                    shortcut = Some(CommandAction::ToggleConsole);
                }
                if command && alt && input.key_pressed(egui::Key::V) {
                    shortcut = Some(CommandAction::ToggleTwoPane);
                }
                if command && input.key_pressed(egui::Key::N) {
                    shortcut = Some(CommandAction::NewRequest);
                }
                if command && input.key_pressed(egui::Key::I) {
                    shortcut = Some(CommandAction::Import);
                }
                if command && shift && input.key_pressed(egui::Key::T) {
                    shortcut = Some(CommandAction::ReopenTab);
                }
                if command && !shift && input.key_pressed(egui::Key::W) {
                    shortcut = Some(CommandAction::CloseTab);
                }
                if command && input.key_pressed(egui::Key::Tab) {
                    shortcut = Some(if shift { CommandAction::PreviousTab } else { CommandAction::NextTab });
                }
            });
            if let Some(action) = shortcut {
                self.command_action = Some(action);
            }
        }
        if self.state.sidebar_open {
            self.sidebar(ui, &theme);
        }
        // The tab strip sits directly under the header, with the environment selector at its end.
        egui::Panel::top("tab-strip").exact_size(TAB_STRIP_HEIGHT).show(ui, |ui| {
            let data = &self.sidebar_data;
            self.tab_action = tabs::tab_strip(ui, &mut self.tabs, data);
        });

        let items = palette_items(&self.sidebar_data, &self.tabs);
        if let Some(action) = command_palette(ui, &mut self.palette, &self.command_registry, &items) {
            self.command_action = Some(action);
        }
        if let Some(SidebarAction::Open { id, kind, preview }) = self.action.clone()
            && let Some(row) = self.sidebar.rows(&self.sidebar_data, &self.sidebar_state.filter).iter().find(|row| row.id == id).cloned()
        {
            let doc = doc_for_row(&row, kind);
            if preview {
                self.tabs.open_preview(doc, row.label.clone());
            } else {
                self.tabs.open(doc, row.label.clone());
            }
        }
        if self.state.right_panel_open {
            egui::Panel::right("snippet-panel").exact_size(metrics.right_panel).resizable(true).show(ui, |ui| {
                egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
                    if let Some(request) = self.snippet_request.clone() {
                        snippet_panel(ui, &request, &mut self.snippet);
                    } else {
                        ui.heading("Code snippet");
                        ui.separator();
                        ui.label(RichText::new("Open a request to see its generated code.").color(theme.palette.text_muted));
                    }
                });
            });
        }
        let active = self.tabs.active_tab().cloned();
        let mut builder_action = None;
        let central = egui::CentralPanel::default().frame(egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin::symmetric(16, 12)));
        central.show(ui, |ui| match active {
            None => self.welcome(ui, &theme),
            Some(tab) => match &tab.doc {
                TabDoc::Welcome => self.welcome(ui, &theme),
                TabDoc::Request { .. }
                | TabDoc::Environment { .. }
                | TabDoc::Globals
                | TabDoc::Collection { .. }
                | TabDoc::Folder { .. }
                | TabDoc::Trash
                | TabDoc::Cookies
                | TabDoc::Settings
                    if content_panel.is_some() =>
                {
                    if let Some(panel) = content_panel.as_mut() {
                        builder_action = panel(ui, &tab.doc);
                    }
                }
                TabDoc::Settings => self.settings(ui, &theme),
                TabDoc::Request { .. }
                | TabDoc::Collection { .. }
                | TabDoc::Folder { .. }
                | TabDoc::Environment { .. }
                | TabDoc::Globals
                | TabDoc::History { .. } => {
                    let note = match &tab.doc {
                        TabDoc::Request { .. } => "The request builder arrives in M3.4.",
                        TabDoc::Collection { .. } => "The collection overview arrives in M3.7.",
                        TabDoc::Folder { .. } => "The folder detail arrives in M3.7.",
                        TabDoc::Environment { .. } => "The environment editor arrives in M3.6.",
                        TabDoc::Globals => "The globals editor arrives in M3.6.",
                        _ => "Reopening a history entry arrives with the request builder (M3.4).",
                    };
                    self.detail(ui, &theme, &tab.title, note);
                }
                TabDoc::Cookies => {
                    ui.label("The cookies manager arrives in M3.9.");
                }
                TabDoc::Trash => {
                    ui.label("The trash view arrives in M3.8.");
                }
                TabDoc::Runner => {
                    ui.label("The runner arrives in M3.8/M9.");
                }
            },
        });
        builder_action
    }

    fn header(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        egui::Panel::top("header").exact_size(theme.metrics.header_height).show(ui, |ui| {
            egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(12, 6)).show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.add_enabled_ui(false, |ui| {
                        widgets::icon_button(ui, "arrow-left", "Back");
                        widgets::icon_button(ui, "arrow-right", "Forward");
                    });
                    ui.separator();
                    let workspace = ui.add(egui::Button::new(RichText::new(&self.workspace_label).strong()).frame(false));
                    workspace.on_hover_text("Open Workspace… (M3.2)");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let settings = widgets::icon_button(ui, "settings", "Settings");
                        if settings.clicked() {
                            self.tabs.open(TabDoc::Settings, "Settings");
                        }
                        let search = ui.add_sized(
                            [320.0, 28.0],
                            egui::Button::new(RichText::new("Search or run a command…").color(theme.palette.text_muted))
                                .frame(true)
                                .fill(theme.palette.surface_sunken)
                                .corner_radius(egui::CornerRadius::same(theme.metrics.radius as u8)),
                        );
                        if search.clicked() {
                            self.palette.open = true;
                        }
                        search.on_hover_text("Command palette (Cmd/Ctrl+K)");
                    });
                });
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        egui::Panel::left("sidebar").default_size(theme.metrics.sidebar_default).resizable(true).show(ui, |ui| {
            egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
                let data = &self.sidebar_data;
                self.action = self.sidebar.show(ui, &mut self.sidebar_state, data);
            });
        });
    }

    fn detail(&mut self, ui: &mut egui::Ui, theme: &Theme, name: &str, note: &str) {
        ui.heading(name);
        ui.separator();
        ui.label(RichText::new(note).color(theme.palette.text_muted));
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        egui::Panel::bottom("bottom-bar").exact_size(theme.metrics.bottom_bar_height).show(ui, |ui| {
            egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(12, 2)).show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if ui.add(egui::Button::new("Sidebar").frame(false)).clicked() {
                        self.state.sidebar_open = !self.state.sidebar_open;
                    }
                    if ui.add(egui::Button::new("Console").frame(false)).clicked() {
                        self.state.console_open = !self.state.console_open;
                    }
                    if ui.add(egui::Button::new("Cookies").frame(false)).clicked() {
                        self.tabs.open(TabDoc::Cookies, "Cookies");
                    }
                    if ui.add(egui::Button::new("Globals").frame(false)).clicked() {
                        self.tabs.open(TabDoc::Globals, "Globals");
                    }
                    if ui.add(egui::Button::new("Trash").frame(false)).clicked() {
                        self.tabs.open(TabDoc::Trash, "Trash");
                    }
                    ui.label("Runner");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(&self.state.status).small().color(theme.palette.text_muted));
                        if ui.add(egui::Button::new("Right").frame(false).selected(self.state.right_panel_open)).clicked() {
                            self.state.right_panel_open = !self.state.right_panel_open;
                        }
                        if ui.add(egui::Button::new("Two-pane").frame(false).selected(self.state.two_pane)).clicked() {
                            self.state.two_pane = !self.state.two_pane;
                        }
                    });
                });
            });
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        ui.add_space(72.0);
        ui.vertical_centered(|ui| {
            ui.heading(RichText::new("Postguy").size(26.0));
            ui.label(RichText::new("An offline API client for HTTP, gRPC and WebSocket.").color(theme.palette.text_muted));
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 330.0).max(0.0) / 2.0);
                if ui.add(egui::Button::new("New HTTP request")).clicked() {
                    self.command_action = Some(CommandAction::NewRequest);
                }
                if ui.add(egui::Button::new("Import…")).clicked() {
                    self.command_action = Some(CommandAction::Import);
                }
                if ui.add(egui::Button::new("Settings…")).clicked() {
                    self.command_action = Some(CommandAction::OpenSettings);
                }
            });
            ui.add_space(12.0);
            ui.label(RichText::new("Cmd/Ctrl+N new request · Cmd/Ctrl+I import · Cmd/Ctrl+K commands").small().color(theme.palette.text_muted));
        });
    }

    fn settings(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        ui.heading("Settings");
        ui.separator();
        ui.label("Theme");
        let mut selected = match self.state.theme_mode {
            ThemeMode::System => "System".to_string(),
            ThemeMode::Light => "Light".to_string(),
            ThemeMode::Dark => "Dark".to_string(),
        };
        ui.horizontal(|ui| {
            widgets::radio(ui, &mut selected, "System", "System");
            widgets::radio(ui, &mut selected, "Light", "Light");
            widgets::radio(ui, &mut selected, "Dark", "Dark");
        });
        self.state.theme_mode = match selected.as_str() {
            "Light" => ThemeMode::Light,
            "Dark" => ThemeMode::Dark,
            _ => ThemeMode::System,
        };
        ui.add_space(8.0);
        ui.label(RichText::new("The real Settings tab arrives in M3.9.").color(theme.palette.text_muted));
    }
}

/// Map a sidebar row to the document a tab shows.
fn doc_for_row(row: &crate::sidebar::SidebarRow, kind: RowKind) -> TabDoc {
    match kind {
        RowKind::Collection => TabDoc::Collection { id: row.id.clone() },
        RowKind::Folder => TabDoc::Folder { collection: row.collection.clone().unwrap_or_default(), folder: row.id.clone() },
        RowKind::Request => TabDoc::Request {
            collection: row.collection.clone().unwrap_or_default(),
            request: row.id.clone(),
            method: row.method.clone().unwrap_or_else(|| "GET".into()),
            protocol: match row.method.as_deref() {
                Some("gRPC") => "grpc".into(),
                Some("WS") => "websocket".into(),
                _ => "http".into(),
            },
        },
        RowKind::Environment => TabDoc::Environment { id: row.id.clone() },
        RowKind::History => TabDoc::History { id: row.id.clone() },
        RowKind::Example => TabDoc::Request {
            collection: row.collection.clone().unwrap_or_default(),
            request: row.parent.clone().unwrap_or_else(|| row.id.clone()),
            method: "GET".into(),
            protocol: "http".into(),
        },
    }
}

/// Workspace documents and open/recent tabs for the palette.
fn palette_items(data: &SidebarData, tabs: &Tabs) -> Vec<PaletteItem> {
    let mut items = Vec::new();
    for tab in &tabs.tabs {
        items.push(PaletteItem { label: tab.title.clone(), hint: "Open tab".into(), doc: tab.doc.clone() });
    }
    for tab in tabs.closed.iter().take(10) {
        items.push(PaletteItem { label: tab.title.clone(), hint: "Recently closed".into(), doc: tab.doc.clone() });
    }
    for collection in &data.collections {
        items.push(PaletteItem {
            label: collection.name.clone(),
            hint: "Collection".into(),
            doc: TabDoc::Collection { id: collection.id.0.clone() },
        });
        let mut stack: Vec<(&String, &Vec<postguy_model::Item>)> = vec![(&collection.id.0, &collection.items)];
        while let Some((collection_id, items_slice)) = stack.pop() {
            for item in items_slice {
                match item {
                    postguy_model::Item::Folder(folder) => {
                        items.push(PaletteItem {
                            label: folder.name.clone(),
                            hint: "Folder".into(),
                            doc: TabDoc::Folder { collection: collection_id.clone(), folder: folder.id.0.clone() },
                        });
                        stack.push((collection_id, &folder.items));
                    }
                    postguy_model::Item::Request(request) => {
                        let (method, protocol) = match &request.kind {
                            postguy_model::RequestKind::Http(http) => (format!("{:?}", http.method).to_ascii_uppercase(), "http"),
                            postguy_model::RequestKind::Grpc(_) => ("gRPC".to_string(), "grpc"),
                            postguy_model::RequestKind::WebSocket(_) => ("WS".to_string(), "websocket"),
                            _ => (String::new(), "http"),
                        };
                        items.push(PaletteItem {
                            label: request.name.clone(),
                            hint: format!("{method} request"),
                            doc: TabDoc::Request {
                                collection: collection_id.clone(),
                                request: request.id.0.clone(),
                                method,
                                protocol: protocol.into(),
                            },
                        });
                    }
                }
            }
        }
    }
    for environment in &data.environments {
        items.push(PaletteItem {
            label: environment.name.clone(),
            hint: "Environment".into(),
            doc: TabDoc::Environment { id: environment.id.0.clone() },
        });
    }
    items
}
