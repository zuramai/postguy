//! Command registry and palette (PROMPT.md 6.4, M3.8): every action has a stable id, label,
//! default shortcut and a group; menus, shortcuts and the palette read the same table. The
//! palette fuzzy-searches commands and workspace items, with `>` restricting it to commands.

use egui::{Align, Event, Key, KeyboardShortcut, Modifiers, RichText, Ui};
use serde::{Deserialize, Serialize};

use crate::tabs::TabDoc;
use crate::theme::Theme;

/// One registered action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: &'static str,
    pub label: &'static str,
    pub shortcut: Option<&'static str>,
    pub group: &'static str,
}

/// What invoking a command asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    NewRequest,
    NewCollection,
    NewEnvironment,
    Import,
    ExportWorkspace,
    SendRequest,
    CancelRequest,
    OpenSettings,
    OpenGlobals,
    OpenCookies,
    OpenTrash,
    ToggleSidebar,
    ToggleConsole,
    ToggleRightPanel,
    ToggleTwoPane,
    CloseTab,
    ReopenTab,
    NextTab,
    PreviousTab,
    ClearConsole,
    /// Open a workspace document from the palette's item list.
    OpenDoc(TabDoc),
    Edit(EditCommand),
    Zoom(ZoomStep),
    Window(WindowCommand),
    /// Open a link in the system browser.
    OpenUrl(&'static str),
}

/// Text-editing menu commands, routed to whichever text field has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditCommand {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Delete,
}

/// One UI zoom step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomStep {
    In,
    Out,
    Reset,
}

/// Where a placement command puts the window inside the work area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    LeftThird,
    CenterThird,
    RightThird,
    LeftTwoThirds,
    RightTwoThirds,
}

/// Window management commands (the Window menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCommand {
    Minimize,
    Zoom,
    Fill,
    Center,
    Place(Placement),
    PreviousSize,
    ToggleFullscreen,
    Close,
}

pub const DOCS_URL: &str = "https://github.com/zuramai/postguy/tree/main/docs";
pub const GITHUB_URL: &str = "https://github.com/zuramai/postguy";

/// The registry: static commands in a stable order.
#[derive(Debug, Clone)]
pub struct CommandRegistry {
    commands: Vec<Command>,
}

impl Default for CommandRegistry {
    fn default() -> Self {
        CommandRegistry {
            commands: vec![
                Command { id: "request.new", label: "New HTTP request", shortcut: Some("Cmd+N"), group: "Create" },
                Command { id: "collection.new", label: "New collection", shortcut: None, group: "Create" },
                Command { id: "environment.new", label: "New environment", shortcut: None, group: "Create" },
                Command { id: "import.open", label: "Import…", shortcut: Some("Cmd+I"), group: "Create" },
                Command { id: "workspace.export", label: "Export all workspace data…", shortcut: None, group: "Create" },
                Command { id: "request.send", label: "Send request", shortcut: Some("Cmd+Enter"), group: "Request" },
                Command { id: "request.cancel", label: "Cancel request", shortcut: Some("Esc"), group: "Request" },
                Command { id: "tab.close", label: "Close tab", shortcut: Some("Cmd+W"), group: "Tabs" },
                Command { id: "tab.reopen", label: "Reopen closed tab", shortcut: Some("Cmd+Shift+T"), group: "Tabs" },
                Command { id: "tab.next", label: "Next tab", shortcut: Some("Cmd+Tab"), group: "Tabs" },
                Command { id: "tab.previous", label: "Previous tab", shortcut: Some("Cmd+Shift+Tab"), group: "Tabs" },
                Command { id: "view.sidebar", label: "Toggle sidebar", shortcut: Some("Cmd+\\"), group: "View" },
                Command { id: "view.console", label: "Toggle console", shortcut: Some("Cmd+Alt+C"), group: "View" },
                Command { id: "view.right", label: "Toggle code snippet panel", shortcut: None, group: "View" },
                Command { id: "view.two-pane", label: "Toggle two-pane layout", shortcut: Some("Cmd+Alt+V"), group: "View" },
                Command { id: "console.clear", label: "Clear console", shortcut: None, group: "View" },
                Command { id: "settings.open", label: "Open settings", shortcut: Some("Cmd+,"), group: "Go" },
                Command { id: "globals.open", label: "Open globals", shortcut: None, group: "Go" },
                Command { id: "cookies.open", label: "Open cookies", shortcut: None, group: "Go" },
                Command { id: "trash.open", label: "Open trash", shortcut: None, group: "Go" },
                Command { id: "edit.undo", label: "Undo", shortcut: Some("Cmd+Z"), group: "Edit" },
                Command { id: "edit.redo", label: "Redo", shortcut: Some("Cmd+Shift+Z"), group: "Edit" },
                Command { id: "edit.cut", label: "Cut", shortcut: Some("Cmd+X"), group: "Edit" },
                Command { id: "edit.copy", label: "Copy", shortcut: Some("Cmd+C"), group: "Edit" },
                Command { id: "edit.paste", label: "Paste", shortcut: Some("Cmd+V"), group: "Edit" },
                Command { id: "edit.delete", label: "Delete", shortcut: None, group: "Edit" },
                Command { id: "view.zoom-in", label: "Zoom in", shortcut: Some("Cmd+="), group: "View" },
                Command { id: "view.zoom-out", label: "Zoom out", shortcut: Some("Cmd+-"), group: "View" },
                Command { id: "view.zoom-reset", label: "Reset zoom", shortcut: Some("Cmd+0"), group: "View" },
                Command { id: "view.fullscreen", label: "Toggle full screen", shortcut: Some("Ctrl+Cmd+F"), group: "View" },
                Command { id: "window.minimize", label: "Minimize", shortcut: Some("Cmd+M"), group: "Window" },
                Command { id: "window.zoom", label: "Zoom", shortcut: None, group: "Window" },
                Command { id: "window.fill", label: "Fill", shortcut: None, group: "Window" },
                Command { id: "window.center", label: "Center", shortcut: None, group: "Window" },
                Command { id: "window.left-half", label: "Left half", shortcut: None, group: "Window" },
                Command { id: "window.right-half", label: "Right half", shortcut: None, group: "Window" },
                Command { id: "window.top-half", label: "Top half", shortcut: None, group: "Window" },
                Command { id: "window.bottom-half", label: "Bottom half", shortcut: None, group: "Window" },
                Command { id: "window.top-left", label: "Top left", shortcut: None, group: "Window" },
                Command { id: "window.top-right", label: "Top right", shortcut: None, group: "Window" },
                Command { id: "window.bottom-left", label: "Bottom left", shortcut: None, group: "Window" },
                Command { id: "window.bottom-right", label: "Bottom right", shortcut: None, group: "Window" },
                Command { id: "window.left-third", label: "Left third", shortcut: None, group: "Window" },
                Command { id: "window.center-third", label: "Center third", shortcut: None, group: "Window" },
                Command { id: "window.right-third", label: "Right third", shortcut: None, group: "Window" },
                Command { id: "window.left-two-thirds", label: "Left two-thirds", shortcut: None, group: "Window" },
                Command { id: "window.right-two-thirds", label: "Right two-thirds", shortcut: None, group: "Window" },
                Command { id: "window.previous-size", label: "Return to previous size", shortcut: None, group: "Window" },
                Command { id: "window.close", label: "Close window", shortcut: Some("Cmd+Shift+W"), group: "Window" },
                Command { id: "app.quit", label: "Quit Postguy", shortcut: Some("Cmd+Q"), group: "Window" },
                Command { id: "help.docs", label: "Documentation", shortcut: None, group: "Help" },
                Command { id: "help.github", label: "GitHub", shortcut: None, group: "Help" },
            ],
        }
    }
}

impl CommandRegistry {
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    pub fn find(&self, id: &str) -> Option<&Command> {
        self.commands.iter().find(|command| command.id == id)
    }

    /// The action for an id (the registry is the single source of truth).
    pub fn action(&self, id: &str) -> Option<CommandAction> {
        Some(match id {
            "request.new" => CommandAction::NewRequest,
            "collection.new" => CommandAction::NewCollection,
            "environment.new" => CommandAction::NewEnvironment,
            "import.open" => CommandAction::Import,
            "workspace.export" => CommandAction::ExportWorkspace,
            "request.send" => CommandAction::SendRequest,
            "request.cancel" => CommandAction::CancelRequest,
            "tab.close" => CommandAction::CloseTab,
            "tab.reopen" => CommandAction::ReopenTab,
            "tab.next" => CommandAction::NextTab,
            "tab.previous" => CommandAction::PreviousTab,
            "view.sidebar" => CommandAction::ToggleSidebar,
            "view.console" => CommandAction::ToggleConsole,
            "view.right" => CommandAction::ToggleRightPanel,
            "view.two-pane" => CommandAction::ToggleTwoPane,
            "console.clear" => CommandAction::ClearConsole,
            "settings.open" => CommandAction::OpenSettings,
            "globals.open" => CommandAction::OpenGlobals,
            "cookies.open" => CommandAction::OpenCookies,
            "trash.open" => CommandAction::OpenTrash,
            "edit.undo" => CommandAction::Edit(EditCommand::Undo),
            "edit.redo" => CommandAction::Edit(EditCommand::Redo),
            "edit.cut" => CommandAction::Edit(EditCommand::Cut),
            "edit.copy" => CommandAction::Edit(EditCommand::Copy),
            "edit.paste" => CommandAction::Edit(EditCommand::Paste),
            "edit.delete" => CommandAction::Edit(EditCommand::Delete),
            "view.zoom-in" => CommandAction::Zoom(ZoomStep::In),
            "view.zoom-out" => CommandAction::Zoom(ZoomStep::Out),
            "view.zoom-reset" => CommandAction::Zoom(ZoomStep::Reset),
            "view.fullscreen" => CommandAction::Window(WindowCommand::ToggleFullscreen),
            "window.minimize" => CommandAction::Window(WindowCommand::Minimize),
            "window.zoom" => CommandAction::Window(WindowCommand::Zoom),
            "window.fill" => CommandAction::Window(WindowCommand::Fill),
            "window.center" => CommandAction::Window(WindowCommand::Center),
            "window.left-half" => CommandAction::Window(WindowCommand::Place(Placement::LeftHalf)),
            "window.right-half" => CommandAction::Window(WindowCommand::Place(Placement::RightHalf)),
            "window.top-half" => CommandAction::Window(WindowCommand::Place(Placement::TopHalf)),
            "window.bottom-half" => CommandAction::Window(WindowCommand::Place(Placement::BottomHalf)),
            "window.top-left" => CommandAction::Window(WindowCommand::Place(Placement::TopLeft)),
            "window.top-right" => CommandAction::Window(WindowCommand::Place(Placement::TopRight)),
            "window.bottom-left" => CommandAction::Window(WindowCommand::Place(Placement::BottomLeft)),
            "window.bottom-right" => CommandAction::Window(WindowCommand::Place(Placement::BottomRight)),
            "window.left-third" => CommandAction::Window(WindowCommand::Place(Placement::LeftThird)),
            "window.center-third" => CommandAction::Window(WindowCommand::Place(Placement::CenterThird)),
            "window.right-third" => CommandAction::Window(WindowCommand::Place(Placement::RightThird)),
            "window.left-two-thirds" => CommandAction::Window(WindowCommand::Place(Placement::LeftTwoThirds)),
            "window.right-two-thirds" => CommandAction::Window(WindowCommand::Place(Placement::RightTwoThirds)),
            "window.previous-size" => CommandAction::Window(WindowCommand::PreviousSize),
            "window.close" | "app.quit" => CommandAction::Window(WindowCommand::Close),
            "help.docs" => CommandAction::OpenUrl(DOCS_URL),
            "help.github" => CommandAction::OpenUrl(GITHUB_URL),
            _ => return None,
        })
    }
}

/// An extra palette item (workspace document or recently closed tab).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    pub label: String,
    pub hint: String,
    pub doc: TabDoc,
}

/// Palette state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PaletteState {
    #[serde(skip)]
    pub open: bool,
    pub query: String,
    #[serde(skip)]
    pub selected: usize,
}

/// Show the palette overlay. Returns the chosen action. `Escape` closes it.
pub fn command_palette(ui: &mut Ui, state: &mut PaletteState, registry: &CommandRegistry, items: &[PaletteItem]) -> Option<CommandAction> {
    if !state.open {
        return None;
    }
    let theme = Theme::current(ui.ctx());
    let mut action = None;
    let mut close = false;
    egui::Area::new(egui::Id::new("command-palette")).order(egui::Order::Foreground).anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 64.0)).show(
        ui.ctx(),
        |ui| {
            egui::Frame::popup(ui.style()).fill(theme.palette.surface).stroke(egui::Stroke::new(1.0, theme.palette.border)).show(ui, |ui| {
                ui.set_min_width(520.0);
                let query = ui.add(
                    egui::TextEdit::singleline(&mut state.query)
                        .hint_text("Search commands, requests, collections… (`>` commands only)")
                        .desired_width(f32::INFINITY),
                );
                query.request_focus();
                if ui.input(|input| input.key_pressed(Key::Escape)) {
                    close = true;
                }
                let commands_only = state.query.trim_start().starts_with('>');
                let needle = state.query.trim_start().trim_start_matches('>').trim().to_ascii_lowercase();
                let mut results: Vec<(CommandAction, String, String)> = Vec::new();
                if !commands_only {
                    for item in items {
                        if fuzzy(&needle, &item.label) || fuzzy(&needle, &item.hint) {
                            results.push((CommandAction::OpenDoc(item.doc.clone()), item.label.clone(), item.hint.clone()));
                        }
                    }
                }
                for command in registry.commands() {
                    if fuzzy(&needle, command.label)
                        && let Some(action) = registry.action(command.id)
                    {
                        let hint = format!("{} · {}", command.group, command.shortcut.unwrap_or(""));
                        results.push((action, command.label.to_string(), hint));
                    }
                }
                results.truncate(12);
                if ui.input(|input| input.key_pressed(Key::ArrowDown)) {
                    state.selected = (state.selected + 1).min(results.len().saturating_sub(1));
                }
                if ui.input(|input| input.key_pressed(Key::ArrowUp)) {
                    state.selected = state.selected.saturating_sub(1);
                }
                if ui.input(|input| input.key_pressed(Key::Enter))
                    && let Some((action_value, _, _)) = results.get(state.selected)
                {
                    action = Some(action_value.clone());
                    close = true;
                }
                ui.separator();
                egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                    for (index, (candidate, label, hint)) in results.iter().enumerate() {
                        let selected = index == state.selected;
                        let mut rich = RichText::new(label).color(if selected { theme.palette.text } else { theme.palette.text_muted });
                        if selected {
                            rich = rich.strong();
                        }
                        let response =
                            ui.add(egui::Button::new(rich).selected(selected).frame(false).min_size(egui::vec2(ui.available_width(), 24.0)));
                        if !hint.trim().is_empty() {
                            response.clone().on_hover_text(hint);
                        }
                        if response.clicked() {
                            action = Some(candidate.clone());
                            close = true;
                        }
                    }
                    if results.is_empty() {
                        ui.label(RichText::new("No matches").color(theme.palette.text_muted));
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Enter open · Esc close · ↑↓ move").small().color(theme.palette.text_muted));
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("Cmd/Ctrl+K").small().color(theme.palette.text_muted));
                    });
                });
            });
        },
    );
    if close {
        state.open = false;
        state.selected = 0;
    }
    action
}

/// A cheap subsequence fuzzy match (case-insensitive); empty needles match everything.
pub fn fuzzy(needle: &str, text: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let lower = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut chars = lower.chars().peekable();
    for wanted in needle.chars() {
        loop {
            match chars.next() {
                Some(candidate) if candidate == wanted => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

/// Parse a registry shortcut string (`Cmd+Shift+Z`) into an egui shortcut. `Ctrl+Cmd+F`
/// (full screen) is `F11` off macOS. Returns `None` for anything outside the grammar.
pub fn parse_shortcut(shortcut: &str) -> Option<KeyboardShortcut> {
    parse_shortcut_for(shortcut, cfg!(target_os = "macos"))
}

fn parse_shortcut_for(shortcut: &str, mac: bool) -> Option<KeyboardShortcut> {
    if shortcut == "Ctrl+Cmd+F" && !mac {
        return Some(KeyboardShortcut::new(Modifiers::NONE, Key::F11));
    }
    let mut tokens: Vec<&str> = shortcut.split('+').collect();
    let key_token = tokens.pop()?;
    let mut modifiers = Modifiers::NONE;
    for token in tokens {
        modifiers |= match token {
            "Cmd" => Modifiers::COMMAND,
            "Shift" => Modifiers::SHIFT,
            "Alt" => Modifiers::ALT,
            "Ctrl" => Modifiers::CTRL,
            _ => return None,
        };
    }
    let key = match key_token {
        "=" => Key::Equals,
        "-" => Key::Minus,
        "0" => Key::Num0,
        "\\" => Key::Backslash,
        "," => Key::Comma,
        "Enter" => Key::Enter,
        "Tab" => Key::Tab,
        "Esc" => Key::Escape,
        letter if letter.len() == 1 && letter.chars().all(|c| c.is_ascii_alphabetic()) => Key::from_name(&letter.to_ascii_uppercase())?,
        _ => return None,
    };
    Some(KeyboardShortcut::new(modifiers, key))
}

/// Clamp the UI zoom to 80-200 %; non-finite values reset to 100 %.
pub fn sanitize_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() { zoom.clamp(0.8, 2.0) } else { 1.0 }
}

/// The zoom after one step, snapped to 10 % increments.
pub fn step_zoom(zoom: f32, step: ZoomStep) -> f32 {
    let snapped = (sanitize_zoom(zoom) * 10.0).round() / 10.0;
    match step {
        ZoomStep::In => sanitize_zoom(snapped + 0.1),
        ZoomStep::Out => sanitize_zoom(snapped - 0.1),
        ZoomStep::Reset => 1.0,
    }
}

/// The egui events that make the focused text field perform `command` (what egui-winit would
/// emit for the key). Paste returns none: it needs a viewport request for the clipboard text.
pub fn edit_events(command: EditCommand) -> Vec<Event> {
    let key = |key: Key, modifiers: Modifiers| Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
    match command {
        EditCommand::Undo => vec![key(Key::Z, Modifiers::COMMAND)],
        EditCommand::Redo => vec![key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT)],
        EditCommand::Cut => vec![Event::Cut],
        EditCommand::Copy => vec![Event::Copy],
        EditCommand::Paste => Vec::new(),
        EditCommand::Delete => vec![key(Key::Delete, Modifiers::NONE)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_ids_and_actions_line_up() {
        let registry = CommandRegistry::default();
        for command in registry.commands() {
            assert!(registry.action(command.id).is_some(), "{} has no action", command.id);
            assert!(registry.find(command.id).is_some());
        }
        assert!(registry.find("missing").is_none());
    }

    #[test]
    fn shortcuts_parse_and_zoom_steps_clamp() {
        assert_eq!(parse_shortcut("Cmd+Shift+Z"), Some(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)));
        assert_eq!(parse_shortcut("Cmd+="), Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::Equals)));
        assert_eq!(parse_shortcut("Cmd+\\"), Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::Backslash)));
        assert_eq!(parse_shortcut("Esc"), Some(KeyboardShortcut::new(Modifiers::NONE, Key::Escape)));
        assert_eq!(parse_shortcut("bogus+"), None);
        assert_eq!(parse_shortcut("Hyper+Z"), None);
        assert_eq!(parse_shortcut_for("Ctrl+Cmd+F", false), Some(KeyboardShortcut::new(Modifiers::NONE, Key::F11)));
        assert_eq!(parse_shortcut_for("Ctrl+Cmd+F", true), Some(KeyboardShortcut::new(Modifiers::CTRL | Modifiers::COMMAND, Key::F)));
        assert!((step_zoom(2.0, ZoomStep::In) - 2.0).abs() < 1e-6);
        assert!((step_zoom(0.8, ZoomStep::Out) - 0.8).abs() < 1e-6);
        assert!((step_zoom(1.0, ZoomStep::In) - 1.1).abs() < 1e-6);
        assert!((step_zoom(1.5, ZoomStep::Reset) - 1.0).abs() < 1e-6);
        assert!((sanitize_zoom(f32::NAN) - 1.0).abs() < 1e-6);
        assert!((sanitize_zoom(9.0) - 2.0).abs() < 1e-6);
        assert_eq!(edit_events(EditCommand::Copy), vec![Event::Copy]);
        assert!(edit_events(EditCommand::Paste).is_empty());
    }

    #[test]
    fn every_registered_shortcut_parses() {
        for command in CommandRegistry::default().commands() {
            if let Some(shortcut) = command.shortcut {
                assert!(parse_shortcut(shortcut).is_some(), "{} has an unparsable shortcut {shortcut}", command.id);
            }
        }
    }

    #[test]
    fn fuzzy_matching_is_subsequence_and_case_insensitive() {
        assert!(fuzzy("ncol", "New collection"));
        assert!(fuzzy("", "anything"));
        assert!(!fuzzy("xyz", "New collection"));
        assert!(fuzzy("NEW", "new request"));
    }
}
