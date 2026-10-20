//! The app shell (PROMPT.md 6.2): header, sidebar, tab strip, central panel, optional right
//! panel and bottom bar, built with the widget kit. M3.1 delivers the frame and the theme; the
//! screens (tree, request builder, response viewer) fill it in M3.2-M3.9.
//!
//! The shell owns only layout state (panel sizes, open/closed flags, the active tab label). All
//! document state belongs to the engine façade, which the app passes in when it exists.

use egui::{Align, Layout, RichText};

use crate::sidebar::{RowKind, Sidebar, SidebarAction, SidebarData, SidebarState};
use crate::tabs::{self, TAB_STRIP_HEIGHT, TabAction, TabDoc, Tabs};

/// The app-supplied request editor shown in the central panel for request tabs.
pub type RequestPanel<'a> = &'a mut dyn FnMut(&mut egui::Ui) -> Option<crate::builder::BuilderAction>;
use crate::theme::{Metrics, Theme, ThemeMode};
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
        }
    }

    /// Draw one frame into the root UI (eframe's `App::ui` and kittest both call this) with no
    /// external request panel.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let _ = self.ui_with(ui, None);
    }

    /// The same frame with an app-supplied request panel (the real builder). Returns the
    /// builder's action so the app can save/send.
    pub fn ui_with(&mut self, ui: &mut egui::Ui, mut request_panel: Option<RequestPanel<'_>>) -> Option<crate::builder::BuilderAction> {
        let ctx = ui.ctx().clone();
        let theme = Theme::from_context(&ctx, self.state.theme_mode);
        theme.install(&ctx);
        let metrics = theme.metrics;

        self.header(ui, &theme);
        self.bottom_bar(ui, &theme);
        self.action = None;
        self.tab_action = None;
        if self.state.sidebar_open {
            self.sidebar(ui, &theme);
        }
        // The tab strip sits directly under the header, with the environment selector at its end.
        egui::Panel::top("tab-strip").exact_size(TAB_STRIP_HEIGHT).show(ui, |ui| {
            let data = &self.sidebar_data;
            self.tab_action = tabs::tab_strip(ui, &mut self.tabs, data);
        });
        tabs::tab_shortcuts(ui, &mut self.tabs);
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
                ui.heading("Code snippet");
                ui.separator();
                ui.label(RichText::new("cURL, fetch and requests arrive in M3.9.").color(theme.palette.text_muted));
            });
        }
        let active = self.tabs.active_tab().cloned();
        let mut builder_action = None;
        egui::CentralPanel::default().show(ui, |ui| match active {
            None => self.welcome(ui, &theme),
            Some(tab) => match &tab.doc {
                TabDoc::Settings => self.settings(ui, &theme),
                TabDoc::Welcome => self.welcome(ui, &theme),
                TabDoc::Request { .. } if request_panel.is_some() => {
                    if let Some(panel) = request_panel.as_mut() {
                        builder_action = panel(ui);
                    }
                }
                TabDoc::Request { .. } | TabDoc::Collection { .. } | TabDoc::Folder { .. } | TabDoc::Environment { .. } | TabDoc::History { .. } => {
                    let note = match &tab.doc {
                        TabDoc::Request { .. } => "The request builder arrives in M3.4.",
                        TabDoc::Collection { .. } => "The collection overview arrives in M3.7.",
                        TabDoc::Folder { .. } => "The folder detail arrives in M3.7.",
                        TabDoc::Environment { .. } => "The environment editor arrives in M3.6.",
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
                    let search = ui.add(egui::TextEdit::singleline(&mut String::new()).hint_text("Search or run a command…").desired_width(260.0));
                    search.on_hover_text("Command palette (M3.8)");
                });
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        egui::Panel::left("sidebar").default_size(theme.metrics.sidebar_default).resizable(true).show(ui, |ui| {
            let data = &self.sidebar_data;
            self.action = self.sidebar.show(ui, &mut self.sidebar_state, data);
        });
    }

    fn detail(&mut self, ui: &mut egui::Ui, theme: &Theme, name: &str, note: &str) {
        ui.heading(name);
        ui.separator();
        ui.label(RichText::new(note).color(theme.palette.text_muted));
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        egui::Panel::bottom("bottom-bar").exact_size(theme.metrics.bottom_bar_height).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                if ui.add(egui::Button::new("Sidebar").frame(false)).clicked() {
                    self.state.sidebar_open = !self.state.sidebar_open;
                }
                if ui.add(egui::Button::new("Console").frame(false)).clicked() {
                    self.state.console_open = !self.state.console_open;
                }
                ui.label("Cookies");
                ui.label("Runner");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(&self.state.status).small().color(theme.palette.text_muted));
                    if ui.add(egui::Button::new("Right").frame(false)).clicked() {
                        self.state.right_panel_open = !self.state.right_panel_open;
                    }
                });
            });
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let metrics: Metrics = theme.metrics;
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.heading("Postguy");
            ui.label(RichText::new("An offline API client: HTTP, gRPC and WebSocket.").color(theme.palette.text_muted));
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 240.0).max(0.0) / 2.0);
                let mut checked = true;
                widgets::checkbox(ui, &mut checked, "Square checkbox");
            });
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 240.0).max(0.0) / 2.0);
                let mut on = true;
                widgets::switch(ui, &mut on, "Toggle switch");
            });
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 240.0).max(0.0) / 2.0);
                let mut choice = "one".to_string();
                widgets::radio(ui, &mut choice, "one", "Outlined radio");
            });
            ui.add_space(8.0);
            ui.label(RichText::new(format!("Theme: {} · base {:.0} px", if theme.dark { "dark" } else { "light" }, metrics.ui_font)).small());
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
