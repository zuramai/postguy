//! macOS-only glue: the native menu bar (muda) and the visible screen area (AppKit).
//!
//! muda is not used elsewhere: on Windows `Menu::init_for_hwnd` is `unsafe` (the workspace
//! forbids `unsafe_code`) and on Linux/FreeBSD it needs GTK (LGPL). Those platforms get the
//! in-app egui menu bar instead.

use std::sync::mpsc::{Receiver, channel};

use muda::accelerator::Accelerator;
use muda::{AboutMetadata, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use postguy_ui_egui::commands::CommandRegistry;
use postguy_ui_egui::menu::{MenuEntry, app_menus};

fn accelerator(shortcut: Option<&str>) -> Option<Accelerator> {
    shortcut.and_then(|shortcut| shortcut.parse::<Accelerator>().ok())
}

fn build_entries(registry: &CommandRegistry, entries: &[MenuEntry]) -> Result<Vec<Box<dyn IsMenuItem>>, String> {
    let mut items: Vec<Box<dyn IsMenuItem>> = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            MenuEntry::Separator => items.push(Box::new(PredefinedMenuItem::separator())),
            MenuEntry::Command(id) => {
                let command = registry.find(id).ok_or_else(|| format!("menu entry `{id}` is not a registered command"))?;
                items.push(Box::new(MenuItem::with_id(command.id, command.label, true, accelerator(command.shortcut))));
            }
            MenuEntry::Submenu { title, entries } => {
                let children = build_entries(registry, entries)?;
                let refs: Vec<&dyn IsMenuItem> = children.iter().map(|child| child.as_ref()).collect();
                items.push(Box::new(Submenu::with_items(*title, true, &refs).map_err(|error| error.to_string())?));
            }
        }
    }
    Ok(items)
}

/// Install the native menu bar. Menu clicks arrive as registry command ids on the returned
/// receiver; the egui context is woken so the click is handled promptly.
pub fn install_menu(ctx: &egui::Context) -> Result<(Menu, Receiver<String>), String> {
    let registry = CommandRegistry::default();
    let menu = Menu::new();
    let metadata = AboutMetadata { name: Some("Postguy".into()), version: Some(env!("CARGO_PKG_VERSION").into()), ..Default::default() };
    let about = PredefinedMenuItem::about(None, Some(metadata));
    let separator = PredefinedMenuItem::separator();
    let settings = build_entries(&registry, &[MenuEntry::Command("settings.open")])?;
    let services = PredefinedMenuItem::services(None);
    let hide = PredefinedMenuItem::hide(None);
    let hide_others = PredefinedMenuItem::hide_others(None);
    let show_all = PredefinedMenuItem::show_all(None);
    // Not `PredefinedMenuItem::quit`: it terminates NSApp without `on_exit`, losing the session.
    let quit = build_entries(&registry, &[MenuEntry::Command("app.quit")])?;
    let mut app_items: Vec<&dyn IsMenuItem> = vec![&about, &separator];
    app_items.extend(settings.iter().map(|item| item.as_ref()));
    app_items.extend([&separator as &dyn IsMenuItem, &services, &separator, &hide, &hide_others, &show_all, &separator]);
    app_items.extend(quit.iter().map(|item| item.as_ref()));
    let app_menu = Submenu::with_items("Postguy", true, &app_items).map_err(|error| error.to_string())?;
    menu.append(&app_menu).map_err(|error| error.to_string())?;

    let mut help_menu: Option<Submenu> = None;
    for spec in app_menus() {
        let children = build_entries(&registry, &spec.entries)?;
        let refs: Vec<&dyn IsMenuItem> = children.iter().map(|child| child.as_ref()).collect();
        let submenu = Submenu::with_items(spec.title, true, &refs).map_err(|error| error.to_string())?;
        menu.append(&submenu).map_err(|error| error.to_string())?;
        if spec.title == "Help" {
            help_menu = Some(submenu);
        }
    }
    menu.init_for_nsapp();
    if let Some(help) = &help_menu {
        help.set_as_help_menu_for_nsapp();
    }
    // Deliberately no `set_as_windows_menu_for_nsapp`: macOS would inject its own Move & Resize
    // next to ours.

    let (sender, receiver) = channel();
    let ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = sender.send(event.id().0.clone());
        ctx.request_repaint();
    }));
    Ok((menu, receiver))
}

/// The visible area (below the menu bar, above the Dock) of the screen holding `window`, in
/// egui points with a top-left origin, or of the primary screen when `window` is unknown.
/// `zoom` is egui's zoom factor: egui points are macOS points divided by it.
pub fn work_area(window: Option<egui::Rect>, zoom: f32) -> Option<egui::Rect> {
    let mtm = objc2::MainThreadMarker::new()?;
    let screens = objc2_app_kit::NSScreen::screens(mtm);
    let zoom = f64::from(zoom);
    if !zoom.is_finite() || zoom <= 0.0 {
        return None;
    }
    let mut primary_height: Option<f64> = None;
    let mut areas: Vec<egui::Rect> = Vec::new();
    for screen in screens.iter() {
        let primary = *primary_height.get_or_insert(screen.frame().size.height);
        let visible = screen.visibleFrame();
        let x = visible.origin.x / zoom;
        let y = (primary - (visible.origin.y + visible.size.height)) / zoom;
        let (w, h) = (visible.size.width / zoom, visible.size.height / zoom);
        if [x, y, w, h].iter().all(|value| value.is_finite()) && w > 0.0 && h > 0.0 {
            areas.push(egui::Rect::from_min_size(egui::pos2(x as f32, y as f32), egui::vec2(w as f32, h as f32)));
        }
    }
    let containing = window.and_then(|rect| areas.iter().copied().find(|area| area.contains(rect.center())));
    containing.or_else(|| areas.first().copied())
}
