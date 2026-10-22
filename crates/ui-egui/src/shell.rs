//! The app shell (PROMPT.md 6.2): header, sidebar, tab strip, central panel, optional right
//! panel and bottom bar, built with the widget kit. M3.1 delivers the frame and the theme; the
//! screens (tree, request builder, response viewer) fill it in M3.2-M3.9.
//!
//! The shell owns only layout state (panel sizes, open/closed flags, the active tab label). All
//! document state belongs to the engine façade, which the app passes in when it exists.

use egui::{Align, Layout, RichText};

use crate::collection::CollectionAction;
use crate::commands::{CommandAction, CommandRegistry, PaletteItem, PaletteState, command_palette, parse_shortcut, sanitize_zoom};
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

/// How the window frame is arranged; the app sets it per platform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowChrome {
    /// Draw the egui File/Edit/View/Window/Help row (false when a native menu bar exists).
    pub in_app_menu: bool,
    /// The header doubles as the title bar: dragging it moves the window.
    pub custom_titlebar: bool,
    /// Space reserved at the header's leading edge (macOS traffic lights).
    pub leading_inset: f32,
}

impl Default for WindowChrome {
    fn default() -> Self {
        WindowChrome { in_app_menu: true, custom_titlebar: false, leading_inset: 0.0 }
    }
}

/// Commands whose shortcuts always fire, even while a text field has focus.
fn works_while_typing(id: &str) -> bool {
    id.starts_with("view.zoom") || id == "view.fullscreen" || id.starts_with("window.") || id == "app.quit"
}

fn modifier_count(modifiers: egui::Modifiers) -> usize {
    [modifiers.alt, modifiers.ctrl, modifiers.shift, modifiers.command, modifiers.mac_cmd].into_iter().filter(|set| *set).count()
}

/// Parsed shortcuts for every command the shell handles itself, most specific first (egui
/// ignores extra Shift/Alt, so `Cmd+Shift+W` must be tried before `Cmd+W`). Edit commands
/// (egui-winit already turns those keys into text events) and send/cancel (owned by the
/// builder) are left out.
fn shortcut_table(registry: &CommandRegistry) -> Vec<(egui::KeyboardShortcut, &'static str)> {
    let mut table: Vec<_> = registry
        .commands()
        .iter()
        .filter(|command| !command.id.starts_with("edit.") && !matches!(command.id, "request.send" | "request.cancel"))
        .filter_map(|command| Some((parse_shortcut(command.shortcut?)?, command.id)))
        .collect();
    table.sort_by_key(|(shortcut, _)| std::cmp::Reverse(modifier_count(shortcut.modifiers)));
    table
}

/// A 1 px horizontal rule across `x`, drawn over the panel edge (panel clip rects stop at
/// their inner margin).
fn edge_line(ui: &egui::Ui, x: egui::Rangef, y: f32, color: egui::Color32) {
    ui.ctx().layer_painter(ui.layer_id()).hline(x, y, egui::Stroke::new(1.0, color));
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
    /// How the window frame is arranged (the app sets this per platform).
    pub chrome: WindowChrome,
    /// Registry shortcuts, most specific first (see [`shortcut_table`]).
    shortcuts: Vec<(egui::KeyboardShortcut, &'static str)>,
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
            chrome: WindowChrome::default(),
            shortcuts: shortcut_table(&CommandRegistry::default()),
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
        if ctx.options(|options| options.zoom_with_keyboard) {
            // Zoom is a registry command (View menu, persisted setting), not an egui built-in.
            ctx.options_mut(|options| options.zoom_with_keyboard = false);
        }
        let zoom = sanitize_zoom(self.settings_value.ui_zoom);
        if (ctx.zoom_factor() - zoom).abs() > 1e-3 {
            ctx.set_zoom_factor(zoom);
        }
        let metrics = theme.metrics;

        self.action = None;
        self.tab_action = None;
        self.command_action = None;
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
        if let Some(action) = self.handle_shortcuts(&ctx) {
            self.command_action = Some(action);
        }
        if self.state.sidebar_open {
            self.sidebar(ui, &theme);
        }
        // The tab strip sits directly under the header, with the environment selector at its end.
        let strip_frame = egui::Frame::NONE.fill(theme.palette.chrome).inner_margin(egui::Margin::symmetric(8, 4));
        egui::Panel::top("tab-strip").exact_size(TAB_STRIP_HEIGHT).frame(strip_frame).show(ui, |ui| {
            let data = &self.sidebar_data;
            self.tab_action = tabs::tab_strip(ui, &mut self.tabs, data);
        });

        let items = palette_items(&self.sidebar_data, &self.tabs);
        if let Some(action) = command_palette(ui, &mut self.palette, &self.command_registry, &items) {
            self.command_action = Some(action);
        }
        if let Some(SidebarAction::Open { id, kind, .. }) = self.action.clone()
            && let Some(row) = self.sidebar.rows(&self.sidebar_data, &self.sidebar_state.filter).iter().find(|row| row.id == id).cloned()
        {
            // Every open adds its own tab (an already-open document is just activated); a click
            // never replaces the tab you were on.
            let doc = doc_for_row(&row, kind);
            self.tabs.open(doc, row.label.clone());
        }
        if self.state.right_panel_open {
            egui::Panel::right("snippet-panel")
                .exact_size(metrics.right_panel)
                .resizable(true)
                .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(12, 10)))
                .show(ui, |ui| {
                    if let Some(request) = self.snippet_request.clone() {
                        snippet_panel(ui, &request, &mut self.snippet);
                    } else {
                        ui.heading("Code snippet");
                        ui.separator();
                        ui.label(RichText::new("Open a request to see its generated code.").color(theme.palette.text_muted));
                    }
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

    /// Run the registry shortcuts for this frame and open the palette on Cmd+K. Returns the
    /// command to run.
    fn handle_shortcuts(&mut self, ctx: &egui::Context) -> Option<CommandAction> {
        let typing = ctx.egui_wants_keyboard_input();
        let registry = &self.command_registry;
        let table = &self.shortcuts;
        let mut open_palette = false;
        let action = ctx.input_mut(|input| {
            if !typing && input.modifiers.command && input.key_pressed(egui::Key::K) {
                open_palette = true;
            }
            if input.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Plus) {
                return registry.action("view.zoom-in");
            }
            for (shortcut, id) in table {
                if typing && !works_while_typing(id) {
                    continue;
                }
                if input.consume_shortcut(shortcut) {
                    return registry.action(id);
                }
            }
            None
        });
        if open_palette {
            self.palette.open = true;
        }
        action
    }

    fn header(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let chrome = self.chrome;
        let palette = theme.palette;
        let mut menu_action = None;
        egui::Panel::top("header")
            .exact_size(theme.metrics.header_height)
            .frame(egui::Frame::NONE.fill(palette.chrome).inner_margin(egui::Margin::symmetric(8, 0)))
            .show(ui, |ui| {
                let rect = ui.max_rect();
                let full = rect.expand2(egui::vec2(8.0, 0.0));
                edge_line(ui, full.x_range(), full.bottom() - 0.5, palette.border);
                if chrome.custom_titlebar {
                    let drag = ui.interact(rect, egui::Id::new("titlebar-drag"), egui::Sense::click_and_drag());
                    if drag.drag_started() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                    if drag.double_clicked() {
                        let maximized = ui.ctx().input(|input| input.viewport().maximized.unwrap_or(false));
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                    }
                }
                let mut free = (rect.left(), rect.right());
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add_space(chrome.leading_inset);
                    if chrome.in_app_menu {
                        if let Some(action) = crate::menu::menu_bar(ui, &self.command_registry) {
                            menu_action = Some(action);
                        }
                        ui.add_space(4.0);
                    }
                    ui.add_enabled_ui(false, |ui| {
                        widgets::icon_button(ui, "arrow-left", "Back");
                        widgets::icon_button(ui, "arrow-right", "Forward");
                    });
                    ui.add_space(4.0);
                    let workspace = ui.add(egui::Button::new(RichText::new(&self.workspace_label).font(theme.semibold_font(13.0))).frame(false));
                    workspace.on_hover_text("Open Workspace… (M3.2)");
                    let left_end = ui.cursor().left();
                    let mut right_start = rect.right();
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let settings = widgets::icon_button(ui, "settings", "Settings");
                        right_start = settings.rect.left();
                        if settings.clicked() {
                            self.tabs.open(TabDoc::Settings, "Settings");
                        }
                    });
                    free = (left_end, right_start);
                });
                // The search box is centred in the header, squeezed only when the left or right
                // groups need the room.
                let width = (rect.width() * 0.4).min(420.0);
                let left = (rect.center().x - width / 2.0).max(free.0 + 12.0);
                let right = (rect.center().x + width / 2.0).min(free.1 - 12.0);
                if right - left >= 80.0 {
                    let search_rect = egui::Rect::from_min_max(egui::pos2(left, rect.center().y - 14.0), egui::pos2(right, rect.center().y + 14.0));
                    let search = ui.interact(search_rect, egui::Id::new("header-search"), egui::Sense::click());
                    let border = if search.hovered() { palette.input_border_hover } else { palette.input_border };
                    ui.painter().rect(
                        search_rect,
                        egui::CornerRadius::same(theme.metrics.radius as u8),
                        palette.input_bg,
                        egui::Stroke::new(1.0, border),
                        egui::StrokeKind::Inside,
                    );
                    crate::icons::paint(
                        ui.painter(),
                        egui::Rect::from_center_size(egui::pos2(search_rect.left() + 17.0, search_rect.center().y), egui::vec2(14.0, 14.0)),
                        "search",
                        palette.text_muted,
                        1.5,
                    );
                    ui.painter().text(
                        egui::pos2(search_rect.left() + 32.0, search_rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        "Search Postguy",
                        egui::TextStyle::Body.resolve(ui.style()),
                        palette.text_muted,
                    );
                    search.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Search or run a command…"));
                    if search.clicked() {
                        self.palette.open = true;
                    }
                    search.on_hover_text("Command palette (Cmd/Ctrl+K)");
                }
            });
        if menu_action.is_some() {
            self.command_action = menu_action;
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, theme: &Theme) {
        let palette = theme.palette;
        egui::Panel::left("sidebar")
            .default_size(theme.metrics.sidebar_default)
            .size_range(180.0..=520.0)
            .resizable(true)
            .frame(egui::Frame::NONE.fill(palette.sidebar).inner_margin(egui::Margin::symmetric(8, 8)))
            .show(ui, |ui| {
                let full = ui.max_rect().expand2(egui::vec2(8.0, 8.0));
                let painter = ui.ctx().layer_painter(ui.layer_id());
                painter.vline(full.right() - 0.5, full.y_range(), egui::Stroke::new(1.0, palette.border));
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
        let palette = theme.palette;
        let bar_height = theme.metrics.bottom_bar_height;
        egui::Panel::bottom("bottom-bar")
            .exact_size(bar_height)
            .frame(egui::Frame::NONE.fill(palette.chrome).inner_margin(egui::Margin::symmetric(8, 2)))
            .show(ui, |ui| {
                let full = ui.max_rect().expand2(egui::vec2(8.0, 2.0));
                edge_line(ui, full.x_range(), full.top() + 0.5, palette.border);
                let item = |selected: bool| widgets::ButtonOptions { height: Some(bar_height - 4.0), small: true, selected, color: None };
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let sidebar = self.state.sidebar_open;
                    if widgets::text_button_with(ui, widgets::ButtonKind::Ghost, Some("panel-left"), "Sidebar", true, item(sidebar)).clicked() {
                        self.state.sidebar_open = !self.state.sidebar_open;
                    }
                    let console = self.state.console_open;
                    if widgets::text_button_with(ui, widgets::ButtonKind::Ghost, Some("terminal"), "Console", true, item(console)).clicked() {
                        self.state.console_open = !self.state.console_open;
                    }
                    if widgets::text_button_with(ui, widgets::ButtonKind::Ghost, Some("cookie"), "Cookies", true, item(false)).clicked() {
                        self.tabs.open(TabDoc::Cookies, "Cookies");
                    }
                    if widgets::text_button_with(ui, widgets::ButtonKind::Ghost, Some("globe"), "Globals", true, item(false)).clicked() {
                        self.tabs.open(TabDoc::Globals, "Globals");
                    }
                    if widgets::text_button_with(ui, widgets::ButtonKind::Ghost, Some("trash"), "Trash", true, item(false)).clicked() {
                        self.tabs.open(TabDoc::Trash, "Trash");
                    }
                    ui.add_space(6.0);
                    let (icon_rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                    crate::icons::paint(ui.painter(), icon_rect, "play", palette.text_muted, 1.5);
                    ui.label(RichText::new("Runner").small().color(palette.text_muted));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let right = self.state.right_panel_open;
                        if widgets::text_button_with(ui, widgets::ButtonKind::Ghost, Some("code"), "Right", true, item(right)).clicked() {
                            self.state.right_panel_open = !self.state.right_panel_open;
                        }
                        let two_pane = self.state.two_pane;
                        if widgets::text_button_with(ui, widgets::ButtonKind::Ghost, Some("panel-bottom"), "Two-pane", true, item(two_pane)).clicked()
                        {
                            self.state.two_pane = !self.state.two_pane;
                        }
                        ui.label(RichText::new(&self.state.status).small().color(palette.text_muted));
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
