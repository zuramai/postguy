//! Command registry and palette (PROMPT.md 6.4, M3.8): every action has a stable id, label,
//! default shortcut and a group; menus, shortcuts and the palette read the same table. The
//! palette fuzzy-searches commands and workspace items, with `>` restricting it to commands.

use egui::{Align, Key, RichText, Ui};
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
}

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
                Command { id: "tab.close", label: "Close tab", shortcut: Some("Cmd+W"), group: "Tabs" },
                Command { id: "tab.reopen", label: "Reopen closed tab", shortcut: Some("Cmd+Shift+T"), group: "Tabs" },
                Command { id: "tab.next", label: "Next tab", shortcut: Some("Cmd+Tab"), group: "Tabs" },
                Command { id: "tab.previous", label: "Previous tab", shortcut: Some("Cmd+Shift+Tab"), group: "Tabs" },
                Command { id: "view.sidebar", label: "Toggle sidebar", shortcut: None, group: "View" },
                Command { id: "view.console", label: "Toggle console", shortcut: Some("Cmd+Alt+C"), group: "View" },
                Command { id: "view.right", label: "Toggle code snippet panel", shortcut: None, group: "View" },
                Command { id: "view.two-pane", label: "Toggle two-pane layout", shortcut: Some("Cmd+Alt+V"), group: "View" },
                Command { id: "console.clear", label: "Clear console", shortcut: None, group: "View" },
                Command { id: "settings.open", label: "Open settings", shortcut: Some("Cmd+,"), group: "Go" },
                Command { id: "globals.open", label: "Open globals", shortcut: None, group: "Go" },
                Command { id: "cookies.open", label: "Open cookies", shortcut: None, group: "Go" },
                Command { id: "trash.open", label: "Open trash", shortcut: None, group: "Go" },
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
    fn fuzzy_matching_is_subsequence_and_case_insensitive() {
        assert!(fuzzy("ncol", "New collection"));
        assert!(fuzzy("", "anything"));
        assert!(!fuzzy("xyz", "New collection"));
        assert!(fuzzy("NEW", "new request"));
    }
}
