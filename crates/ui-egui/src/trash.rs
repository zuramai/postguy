//! The trash view: soft-deleted collections, environments and files with
//! their kind, original path and deletion time; Restore puts one back. Item-level deletes are
//! parked as JSON snapshots and appear here once the item-restore path lands.

use egui::{Align, Layout, RichText, ScrollArea, Ui};

use crate::theme::Theme;

/// One row the app maps from the store's trash entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashEntryView {
    pub id: String,
    pub kind: String,
    pub original: String,
    pub deleted_at_ms: i64,
    pub size_bytes: Option<u64>,
}

/// What the app should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrashAction {
    Restore(String),
}

/// Draw the trash list.
pub fn trash_view(ui: &mut Ui, entries: &[TrashEntryView]) -> Option<TrashAction> {
    let theme = Theme::current(ui.ctx());
    let mut action = None;
    ui.horizontal(|ui| {
        ui.heading("Trash");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(format!("{} item(s)", entries.len())).small().color(theme.palette.text_muted));
        });
    });
    ui.separator();
    if entries.is_empty() {
        ui.label(
            RichText::new("The trash is empty. Deleted collections and environments land here and can be restored.").color(theme.palette.text_muted),
        );
        return None;
    }
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for entry in entries {
            ui.horizontal(|ui| {
                ui.add_sized([100.0, 20.0], egui::Label::new(RichText::new(&entry.kind).color(theme.palette.text_muted)));
                ui.add_sized([320.0, 20.0], egui::Label::new(RichText::new(&entry.original).color(theme.palette.text)).truncate());
                ui.add_sized(
                    [180.0, 20.0],
                    egui::Label::new(RichText::new(format_time(entry.deleted_at_ms)).small().color(theme.palette.text_muted)),
                );
                if let Some(size) = entry.size_bytes {
                    ui.label(RichText::new(format!("{size} B")).small().color(theme.palette.text_muted));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(egui::Button::new("Restore")).clicked() {
                        action = Some(TrashAction::Restore(entry.id.clone()));
                    }
                });
            });
            ui.separator();
        }
    });
    action
}

fn format_time(millis: i64) -> String {
    // The UI only needs relative age; absolute formatting arrives with the history list.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|duration| duration.as_millis() as i64).unwrap_or(0);
    let seconds = now.saturating_sub(millis) / 1000;
    if seconds < 60 {
        format!("{seconds}s ago")
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_are_relative_and_never_negative() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|duration| duration.as_millis() as i64).unwrap_or(0);
        assert_eq!(format_time(now - 5_000), "5s ago");
        assert_eq!(format_time(now - 120_000), "2m ago");
        assert!(format_time(now + 60_000).ends_with("s ago"));
    }
}
