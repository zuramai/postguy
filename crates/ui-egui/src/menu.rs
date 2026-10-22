//! The application menu model (File / Edit / View / Window / Help) and the in-app egui menu
//! bar. The model lists registry command ids only, so the egui bar, the native macOS menu and
//! the command palette all run the same actions and show the same shortcuts.

use egui::{Color32, Ui};

use crate::commands::{CommandAction, CommandRegistry};
use crate::theme::Theme;
use crate::widgets;

/// One row of a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    /// A registry command id.
    Command(&'static str),
    Separator,
    Submenu {
        title: &'static str,
        entries: Vec<MenuEntry>,
    },
}

/// A top-level menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuSpec {
    pub title: &'static str,
    pub entries: Vec<MenuEntry>,
}

fn commands(ids: &[&'static str]) -> Vec<MenuEntry> {
    ids.iter().map(|id| MenuEntry::Command(id)).collect()
}

/// The menus, in display order.
pub fn app_menus() -> Vec<MenuSpec> {
    use MenuEntry::{Command, Separator, Submenu};
    vec![
        MenuSpec {
            title: "File",
            entries: vec![
                Submenu { title: "New", entries: commands(&["request.new", "collection.new", "environment.new"]) },
                Command("import.open"),
                Separator,
                Command("tab.close"),
                Command("window.close"),
            ],
        },
        MenuSpec {
            title: "Edit",
            entries: vec![
                Command("edit.undo"),
                Command("edit.redo"),
                Separator,
                Command("edit.cut"),
                Command("edit.copy"),
                Command("edit.paste"),
                Command("edit.delete"),
            ],
        },
        MenuSpec {
            title: "View",
            entries: vec![
                Command("view.zoom-in"),
                Command("view.zoom-out"),
                Command("view.zoom-reset"),
                Separator,
                Command("view.fullscreen"),
                Command("view.sidebar"),
            ],
        },
        MenuSpec {
            title: "Window",
            entries: vec![
                Command("window.minimize"),
                Command("window.zoom"),
                Separator,
                Command("window.fill"),
                Command("window.center"),
                Submenu {
                    title: "Move & Resize",
                    entries: vec![
                        Submenu {
                            title: "Halves",
                            entries: commands(&["window.left-half", "window.right-half", "window.top-half", "window.bottom-half"]),
                        },
                        Submenu {
                            title: "Quarters",
                            entries: commands(&["window.top-left", "window.top-right", "window.bottom-left", "window.bottom-right"]),
                        },
                        Submenu {
                            title: "Arrange",
                            entries: vec![
                                Command("window.left-third"),
                                Command("window.center-third"),
                                Command("window.right-third"),
                                Command("window.left-two-thirds"),
                                Command("window.right-two-thirds"),
                                Separator,
                                Command("window.previous-size"),
                            ],
                        },
                    ],
                },
                Separator,
                Command("window.close"),
            ],
        },
        MenuSpec { title: "Help", entries: commands(&["help.docs", "help.github"]) },
    ]
}

/// A registry shortcut as the platform writes it: macOS keeps `Cmd+…`; elsewhere `Cmd` is
/// `Ctrl` and full screen is `F11`.
pub fn display_shortcut(shortcut: &str, mac: bool) -> String {
    if mac {
        return shortcut.to_owned();
    }
    if shortcut == "Ctrl+Cmd+F" {
        return "F11".to_owned();
    }
    shortcut.replace("Cmd", "Ctrl")
}

/// Draw the in-app menu bar. Returns the action of the entry clicked this frame.
pub fn menu_bar(ui: &mut Ui, registry: &CommandRegistry) -> Option<CommandAction> {
    let theme = Theme::current(ui.ctx());
    let menus = app_menus();
    let mut picked: Option<&'static str> = None;
    // `MenuBar` takes the full available width; give it exactly the width of its buttons so
    // the header can lay out more after it.
    let font = egui::TextStyle::Button.resolve(ui.style());
    let padding = 16.0 + 2.0;
    let width: f32 = menus
        .iter()
        .map(|menu| ui.painter().layout_no_wrap(menu.title.to_owned(), font.clone(), Color32::PLACEHOLDER).size().x + padding)
        .sum::<f32>()
        + 4.0;
    let height = ui.spacing().interact_size.y;
    ui.scope(|ui| {
        let widgets = &mut ui.style_mut().visuals.widgets;
        widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        widgets.inactive.bg_stroke = egui::Stroke::NONE;
        widgets.hovered.weak_bg_fill = theme.palette.hover;
        widgets.hovered.bg_stroke = egui::Stroke::NONE;
        widgets.active.weak_bg_fill = theme.palette.selected;
        widgets.active.bg_stroke = egui::Stroke::NONE;
        widgets.open.weak_bg_fill = theme.palette.selected;
        widgets.open.bg_stroke = egui::Stroke::NONE;
        ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.allocate_ui(egui::vec2(width, height), |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                for menu in &menus {
                    ui.menu_button(menu.title, |ui| {
                        ui.set_min_width(230.0);
                        show_entries(ui, registry, &menu.entries, &mut picked);
                    });
                }
            });
        });
    });
    picked.and_then(|id| registry.action(id))
}

fn show_entries(ui: &mut Ui, registry: &CommandRegistry, entries: &[MenuEntry], picked: &mut Option<&'static str>) {
    for entry in entries {
        match entry {
            MenuEntry::Separator => {
                ui.separator();
            }
            MenuEntry::Command(id) => {
                let Some(command) = registry.find(id) else { continue };
                let shortcut = command.shortcut.map(|shortcut| display_shortcut(shortcut, cfg!(target_os = "macos")));
                if widgets::menu_item(ui, None, command.label, shortcut.as_deref(), true).clicked() {
                    *picked = Some(command.id);
                }
            }
            MenuEntry::Submenu { title, entries } => {
                ui.menu_button(*title, |ui| {
                    ui.set_min_width(200.0);
                    show_entries(ui, registry, entries, picked);
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(entries: &[MenuEntry], visit: &mut impl FnMut(&'static str)) {
        for entry in entries {
            match entry {
                MenuEntry::Command(id) => visit(id),
                MenuEntry::Separator => {}
                MenuEntry::Submenu { entries, .. } => walk(entries, visit),
            }
        }
    }

    #[test]
    fn every_menu_entry_is_a_registered_command() {
        let registry = CommandRegistry::default();
        let mut count = 0;
        for menu in app_menus() {
            walk(&menu.entries, &mut |id| {
                count += 1;
                assert!(registry.action(id).is_some(), "{id} has no action");
                assert!(registry.find(id).is_some(), "{id} is not registered");
            });
        }
        assert!(count > 30);
    }

    #[test]
    fn shortcuts_are_shown_the_platform_way() {
        assert_eq!(display_shortcut("Cmd+Shift+Z", true), "Cmd+Shift+Z");
        assert_eq!(display_shortcut("Cmd+Shift+Z", false), "Ctrl+Shift+Z");
        assert_eq!(display_shortcut("Ctrl+Cmd+F", false), "F11");
    }
}
