//! Timeline skeleton shared by Console, gRPC and WebSocket: virtualised
//! rows, an optional reversed order, filters handled by the caller, and expandable detail.
//! Message bodies and per-direction colours are left to the screens that use it.

use egui::{Response, ScrollArea, Sense, Ui};
use serde_json::Value;

/// Direction or role of one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageDirection {
    Sent,
    Received,
    Info,
    Warning,
    Error,
}

impl MessageDirection {
    fn arrow(self) -> &'static str {
        match self {
            MessageDirection::Sent => "↑",
            MessageDirection::Received => "↓",
            MessageDirection::Info => "•",
            MessageDirection::Warning => "!",
            MessageDirection::Error => "x",
        }
    }

    fn color(self, theme: &crate::theme::Theme) -> egui::Color32 {
        match self {
            MessageDirection::Sent => theme.palette.accent,
            MessageDirection::Received => theme.palette.success,
            MessageDirection::Info => theme.palette.text_muted,
            MessageDirection::Warning => theme.palette.warning,
            MessageDirection::Error => theme.palette.danger,
        }
    }
}

/// One timeline row.
#[derive(Debug, Clone)]
pub struct TimelineEntry {
    pub direction: MessageDirection,
    pub time_ms: i64,
    pub title: String,
    pub size_bytes: u64,
    pub detail: Option<String>,
}

impl TimelineEntry {
    pub fn new(direction: MessageDirection, time_ms: i64, title: impl Into<String>, size_bytes: u64) -> TimelineEntry {
        TimelineEntry { direction, time_ms, title: title.into(), size_bytes, detail: None }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// The timeline widget. `reverse` shows newest first (gRPC/WebSocket default), `follow` keeps
/// the newest row visible.
#[derive(Debug, Clone)]
pub struct Timeline {
    pub entries: Vec<TimelineEntry>,
    pub reverse: bool,
    pub follow: bool,
    pub id: egui::Id,
}

impl Default for Timeline {
    fn default() -> Self {
        Timeline { entries: Vec::new(), reverse: true, follow: true, id: egui::Id::new("timeline") }
    }
}

impl Timeline {
    pub fn new(entries: Vec<TimelineEntry>) -> Timeline {
        Timeline { entries, ..Timeline::default() }
    }

    pub fn id(mut self, id: egui::Id) -> Self {
        self.id = id;
        self
    }

    /// The visible row order (reverse indexes into `entries` without cloning).
    fn order(&self) -> Vec<usize> {
        if self.reverse { (0..self.entries.len()).rev().collect() } else { (0..self.entries.len()).collect() }
    }

    pub fn show(&self, ui: &mut Ui) -> Response {
        let theme = crate::theme::Theme::current(ui.ctx());
        let order = self.order();
        let row_height = 24.0;
        let mut clicked: Option<Response> = None;
        let mut scroll = ScrollArea::vertical().id_salt((self.id, "scroll")).auto_shrink([false, false]);
        if self.follow && !self.reverse {
            scroll = scroll.stick_to_bottom(true);
        }
        scroll.show_rows(ui, row_height, order.len(), |ui, range| {
            for position in range {
                let Some(index) = order.get(position).copied() else { continue };
                let Some(entry) = self.entries.get(index) else { continue };
                let expanded_key = egui::Id::new((self.id, "expanded", index));
                let expanded = ui.data(|data| data.get_temp::<bool>(expanded_key).unwrap_or(false));
                let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_height), Sense::click());
                if response.hovered() && entry.detail.is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if response.clicked() && entry.detail.is_some() {
                    ui.data_mut(|data| data.insert_temp(expanded_key, !expanded));
                }
                if ui.is_rect_visible(rect) {
                    let color = entry.direction.color(&theme);
                    ui.painter().text(
                        egui::pos2(rect.left() + 8.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        entry.direction.arrow(),
                        egui::TextStyle::Body.resolve(ui.style()),
                        color,
                    );
                    ui.painter().text(
                        egui::pos2(rect.left() + 28.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        &entry.title,
                        egui::TextStyle::Body.resolve(ui.style()),
                        theme.palette.text,
                    );
                    ui.painter().text(
                        egui::pos2(rect.right() - 8.0, rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        format!("{} B", entry.size_bytes),
                        egui::TextStyle::Small.resolve(ui.style()),
                        theme.palette.text_muted,
                    );
                }
                if expanded && let Some(detail) = &entry.detail {
                    ui.label(egui::RichText::new(detail).monospace().color(theme.palette.text_muted));
                }
                response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &entry.title));
                clicked = Some(response);
            }
        });
        clicked.unwrap_or_else(|| ui.label(""))
    }
}

/// A small JSON detail preview for a timeline row (bodies are truncated by the caller).
pub fn json_preview(value: &Value, cap: usize) -> String {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    if text.len() > cap { format!("{}…", text.chars().take(cap).collect::<String>()) } else { text }
}
