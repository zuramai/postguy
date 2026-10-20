//! The app shell (PROMPT.md 6.2): header, sidebar, tab strip, central panel, optional right
//! panel and bottom bar, built with the widget kit. M3.1 delivers the frame and the theme; the
//! screens (tree, request builder, response viewer) fill it in M3.2-M3.9.
//!
//! The shell owns only layout state (panel sizes, open/closed flags, the active tab label). All
//! document state belongs to the engine façade, which the app passes in when it exists.

use egui::{Align, Layout, RichText};

use crate::theme::{Metrics, Theme, ThemeMode};
use crate::widgets;

/// The tab shown in the central panel skeleton (M3.3 replaces this with the real tab model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShellTab {
    #[default]
    Welcome,
    Settings,
}

/// Layout state the shell keeps across frames.
#[derive(Debug, Clone)]
pub struct ShellState {
    pub theme_mode: ThemeMode,
    pub sidebar_open: bool,
    pub right_panel_open: bool,
    pub console_open: bool,
    pub two_pane: bool,
    pub tab: ShellTab,
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
            tab: ShellTab::Welcome,
            status: "Ready".to_string(),
        }
    }
}

/// The shell app. `workspace` is the folder label shown in the header (the engine wires it in
/// M3.2; the skeleton shows "No workspace").
pub struct PostguyShell {
    pub state: ShellState,
    pub workspace_label: String,
}

impl Default for PostguyShell {
    fn default() -> Self {
        PostguyShell { state: ShellState::default(), workspace_label: "No workspace".to_string() }
    }
}

impl PostguyShell {
    pub fn new(workspace_label: impl Into<String>) -> PostguyShell {
        PostguyShell { state: ShellState::default(), workspace_label: workspace_label.into() }
    }

    /// Draw one frame into the root UI (eframe's `App::ui` and kittest both call this).
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let theme = Theme::from_context(&ctx, self.state.theme_mode);
        theme.install(&ctx);
        let metrics = theme.metrics;

        self.header(ui, &theme);
        self.bottom_bar(ui, &theme);
        if self.state.sidebar_open {
            self.sidebar(ui, &theme);
        }
        if self.state.right_panel_open {
            egui::Panel::right("snippet-panel").exact_size(metrics.right_panel).resizable(true).show(ui, |ui| {
                ui.heading("Code snippet");
                ui.separator();
                ui.label(RichText::new("cURL, fetch and requests arrive in M3.9.").color(theme.palette.text_muted));
            });
        }
        egui::CentralPanel::default().show(ui, |ui| match self.state.tab {
            ShellTab::Welcome => self.welcome(ui, &theme),
            ShellTab::Settings => self.settings(ui, &theme),
        });
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
                        self.state.tab = ShellTab::Settings;
                    }
                    let search = ui.add(egui::TextEdit::singleline(&mut String::new()).hint_text("Search or run a command…").desired_width(260.0));
                    search.on_hover_text("Command palette (M3.8)");
                });
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        egui::Panel::left("sidebar").default_size(theme.metrics.sidebar_default).resizable(true).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Filter…").color(theme.palette.text_muted));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label("+");
                });
            });
            ui.separator();
            for section in ["COLLECTIONS", "ENVIRONMENTS", "HISTORY"] {
                ui.add_space(4.0);
                ui.label(RichText::new(section).small().color(theme.palette.text_muted));
                ui.label(RichText::new("The tree lands in M3.2.").small().color(theme.palette.text_muted));
            }
        });
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
