//! The Console panel (PROMPT.md 5.5, M3.9): request summaries, logs, warnings and errors from the
//! engine's bounded ring buffer, with filter chips, search, timestamps and Clear. Bodies in the
//! detail are already truncated and masked by the engine.

use egui::{Align, Layout, RichText, ScrollArea, Sense, Ui};
use serde::{Deserialize, Serialize};

use crate::theme::Theme;

/// One console row as the app maps it from the engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleRow {
    pub id: u64,
    pub time_ms: i64,
    pub level: String,
    pub kind: String,
    pub message: String,
    pub detail: Option<String>,
}

/// The toolbar filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConsoleFilter {
    #[default]
    All,
    Requests,
    Logs,
    Warnings,
    Errors,
}

/// Panel state (kept by the shell/app).
#[derive(Debug, Clone, Default)]
pub struct ConsoleState {
    pub filter: ConsoleFilter,
    pub search: String,
    pub show_timestamps: bool,
    pub expanded: Option<u64>,
}

/// What the toolbar asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleAction {
    Clear,
}

impl ConsoleFilter {
    fn accepts(self, row: &ConsoleRow) -> bool {
        match self {
            ConsoleFilter::All => true,
            ConsoleFilter::Requests => row.kind == "request",
            ConsoleFilter::Logs => row.kind == "log",
            ConsoleFilter::Warnings => row.kind == "warning" || row.level == "warning",
            ConsoleFilter::Errors => row.kind == "error" || row.level == "error",
        }
    }
}

/// Draw the console panel.
pub fn console_view(ui: &mut Ui, rows: &[ConsoleRow], state: &mut ConsoleState) -> Option<ConsoleAction> {
    let theme = Theme::current(ui.ctx());
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Console panel").strong());
        ui.separator();
        for (filter, label) in [
            (ConsoleFilter::All, "All"),
            (ConsoleFilter::Requests, "Requests"),
            (ConsoleFilter::Logs, "Logs"),
            (ConsoleFilter::Warnings, "Warnings"),
            (ConsoleFilter::Errors, "Errors"),
        ] {
            if ui.add(egui::Button::new(label).selected(state.filter == filter).frame(false).small()).clicked() {
                state.filter = filter;
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.search).hint_text("Search console…").desired_width(180.0));
            ui.label("Timestamps");
            let mut stamps = state.show_timestamps;
            if crate::widgets::compact_checkbox(ui, &mut stamps).changed() {
                state.show_timestamps = stamps;
            }
            if ui.add(egui::Button::new("Clear").frame(false)).clicked() {
                action = Some(ConsoleAction::Clear);
            }
        });
    });
    ui.separator();
    let needle = state.search.trim().to_ascii_lowercase();
    let visible: Vec<&ConsoleRow> = rows
        .iter()
        .filter(|row| state.filter.accepts(row))
        .filter(|row| needle.is_empty() || row.message.to_ascii_lowercase().contains(&needle))
        .collect();
    if visible.is_empty() {
        ui.label(RichText::new("Nothing in the console yet.").color(theme.palette.text_muted));
        return action;
    }
    let row_height = 22.0;
    ScrollArea::vertical().id_salt("console-scroll").auto_shrink([false, false]).stick_to_bottom(true).show_rows(
        ui,
        row_height,
        visible.len(),
        |ui, range| {
            for index in range {
                let Some(row) = visible.get(index) else { continue };
                let color = match row.level.as_str() {
                    "error" => theme.palette.danger,
                    "warning" => theme.palette.warning,
                    _ => theme.palette.text,
                };
                let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_height), Sense::click());
                if response.hovered() && row.detail.is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &row.message));
                if response.clicked() && row.detail.is_some() {
                    state.expanded = if state.expanded == Some(row.id) { None } else { Some(row.id) };
                }
                if ui.is_rect_visible(rect) {
                    let mut x = rect.left() + 6.0;
                    if state.show_timestamps {
                        let stamp = format_time(row.time_ms);
                        ui.painter().text(
                            egui::pos2(x, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            stamp,
                            egui::TextStyle::Small.resolve(ui.style()),
                            theme.palette.text_muted,
                        );
                        x += 76.0;
                    }
                    let glyph = match row.level.as_str() {
                        "error" => "x",
                        "warning" => "!",
                        _ => "·",
                    };
                    ui.painter().text(
                        egui::pos2(x, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        glyph,
                        egui::TextStyle::Body.resolve(ui.style()),
                        color,
                    );
                    ui.painter().text(
                        egui::pos2(x + 16.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        &row.message,
                        egui::TextStyle::Body.resolve(ui.style()),
                        color,
                    );
                }
                if state.expanded == Some(row.id)
                    && let Some(detail) = &row.detail
                {
                    ui.label(RichText::new(detail).monospace().small().color(theme.palette.text_muted));
                }
            }
        },
    );
    action
}

fn format_time(millis: i64) -> String {
    // HH:MM:SS without a date dependency: seconds of the Unix day.
    let seconds = millis.div_euclid(1000).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", seconds / 3600, (seconds / 60) % 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_classify_rows() {
        let row = ConsoleRow { id: 1, time_ms: 0, level: "warning".into(), kind: "request".into(), message: "x".into(), detail: None };
        assert!(ConsoleFilter::All.accepts(&row));
        assert!(ConsoleFilter::Requests.accepts(&row));
        assert!(ConsoleFilter::Warnings.accepts(&row));
        assert!(!ConsoleFilter::Errors.accepts(&row));
    }

    #[test]
    fn times_are_utc_midnight_based() {
        assert_eq!(format_time(0), "00:00:00");
        assert_eq!(format_time(3_661_000), "01:01:01");
    }
}
