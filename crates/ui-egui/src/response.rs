//! The response viewer (PROMPT.md 5.4, M3.5): the header strip with status/time/size and phase
//! popovers, Body/Cookies/Headers tabs, Pretty/Raw/Preview/Hex body modes, a search box and the
//! Save/Clear menus.
//!
//! Large bodies stay virtualised: text goes through [`LargeTextView`] (precomputed line starts,
//! only visible rows laid out) and hex is generated per visible row. Bodies over the pretty cap
//! open in Raw with a banner instead of formatting 5 MB+.

use std::sync::Arc;

use egui::{Align, Layout, RichText, ScrollArea, Ui};

use crate::theme::Theme;
use crate::widgets::{self, LargeTextView, menu_item};

/// Everything the viewer shows for one finished response (mapped by the app from the engine's
/// cached send; this crate does not depend on the engine).
#[derive(Debug, Clone)]
pub struct ResponseData {
    pub status: u16,
    pub status_text: String,
    pub http_version: String,
    pub headers: Vec<(String, String)>,
    /// DECODED body bytes.
    pub body: Vec<u8>,
    pub header_bytes: usize,
    pub body_encoded_bytes: usize,
    pub body_decoded_bytes: usize,
    pub content_encoding: Option<String>,
    pub connect_ms: u64,
    pub ttfb_ms: u64,
    pub total_ms: u64,
    pub download_ms: u64,
    pub redirects: usize,
    pub proxy: Option<String>,
}

/// Which content tab is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponseTab {
    #[default]
    Body,
    Cookies,
    Headers,
}

/// Body rendering mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BodyMode {
    #[default]
    Pretty,
    Raw,
    Preview,
    Hex,
}

/// Per-tab viewer state (kept by the app so switching tabs preserves choices).
#[derive(Debug, Clone)]
pub struct ResponseViewerState {
    pub tab: ResponseTab,
    pub mode: BodyMode,
    pub search: String,
    pub wrap: bool,
    /// Cache of the pretty-printed body and the signature it was built from.
    pretty: Option<(u64, usize, Arc<str>)>,
}

impl Default for ResponseViewerState {
    fn default() -> Self {
        ResponseViewerState { tab: ResponseTab::Body, mode: BodyMode::Pretty, search: String::new(), wrap: false, pretty: None }
    }
}

/// Actions the app must perform (clipboard actions happen in the widget).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseAction {
    SaveBody,
    Clear,
}

/// Body bytes above this are never pretty-printed in the UI thread.
pub const PRETTY_CAP: usize = 5 << 20;

impl ResponseData {
    fn text(&self) -> Option<String> {
        String::from_utf8(self.body.clone()).ok()
    }

    fn content_type(&self) -> Option<&str> {
        self.headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("content-type")).map(|(_, value)| value.as_str())
    }

    fn looks_json(&self) -> bool {
        self.content_type().is_some_and(|value| value.contains("json"))
            || self.body.iter().copied().find(|byte| !byte.is_ascii_whitespace()).is_some_and(|byte| matches!(byte, b'{' | b'['))
    }

    /// The pretty body, cached by (length, first 32 bytes checksum).
    fn pretty(&mut self, state: &mut ResponseViewerState) -> Option<Arc<str>> {
        if self.body.len() > PRETTY_CAP {
            return None;
        }
        let signature = signature(&self.body);
        if let Some((cached_signature, cached_len, text)) = &state.pretty
            && *cached_signature == signature
            && *cached_len == self.body.len()
        {
            return Some(text.clone());
        }
        let text: Arc<str> = match serde_json::from_slice::<serde_json::Value>(&self.body) {
            Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_default().into(),
            Err(_) => self.text()?.into(),
        };
        state.pretty = Some((signature, self.body.len(), text.clone()));
        Some(text)
    }
}

fn signature(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes.iter().take(32).chain(bytes.iter().rev().take(8)) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Draw the response pane. Returns an app-level action (Save/Clear).
pub fn response_view(ui: &mut Ui, data: &mut ResponseData, state: &mut ResponseViewerState) -> Option<ResponseAction> {
    let theme = Theme::current(ui.ctx());
    let mut action = None;
    let language = language_label(data);
    // Header strip.
    ui.horizontal(|ui| {
        for (tab, label) in [(ResponseTab::Body, "Body"), (ResponseTab::Cookies, "Cookies"), (ResponseTab::Headers, "Headers")] {
            let selected = state.tab == tab;
            let count = match tab {
                ResponseTab::Cookies => Some(set_cookie_rows(&data.headers).len()),
                ResponseTab::Headers => Some(data.headers.len()),
                ResponseTab::Body => None,
            };
            let text = match count {
                Some(count) if count > 0 => format!("{label} {count}"),
                _ => label.to_string(),
            };
            let mut rich = RichText::new(text).color(if selected { theme.palette.text } else { theme.palette.text_muted });
            if selected {
                rich = rich.strong();
            }
            let response = ui.add(egui::Button::new(rich).frame(false));
            if response.clicked() {
                state.tab = tab;
            }
            if selected {
                ui.painter().line_segment(
                    [egui::pos2(response.rect.left(), response.rect.bottom()), egui::pos2(response.rect.right(), response.rect.bottom())],
                    egui::Stroke::new(2.0, theme.palette.accent),
                );
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let menu = ui.add(egui::Button::new("...").frame(false));
            egui::Popup::menu(&menu).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
                ui.set_min_width(180.0);
                if menu_item(ui, None, "Save body to file", None, true).clicked() {
                    action = Some(ResponseAction::SaveBody);
                }
                if menu_item(ui, None, "Copy body", None, true).clicked()
                    && let Some(text) = data.text()
                {
                    ui.ctx().copy_text(text);
                }
                if menu_item(ui, None, "Copy headers", None, true).clicked() {
                    let text = data.headers.iter().map(|(name, value)| format!("{name}: {value}")).collect::<Vec<String>>().join("\n");
                    ui.ctx().copy_text(text);
                }
                ui.separator();
                if menu_item(ui, None, "Clear response", None, true).clicked() {
                    action = Some(ResponseAction::Clear);
                }
            });
            let save = ui.add(egui::Button::new("Save Response"));
            if save.clicked() {
                action = Some(ResponseAction::SaveBody);
            }
            let size = ui.label(RichText::new(format_size(data.body_decoded_bytes as u64)).small().color(theme.palette.text_muted));
            size.on_hover_ui(|ui| {
                ui.label(format!("Headers: {} B", data.header_bytes));
                ui.label(format!("Body: {} B decoded", format_size(data.body_decoded_bytes as u64)));
                if let Some(encoding) = &data.content_encoding {
                    ui.label(format!("Wire body: {} B ({encoding})", format_size(data.body_encoded_bytes as u64)));
                }
            });
            let time = ui.label(RichText::new(format!("{} ms", data.total_ms)).small().color(theme.palette.text_muted));
            time.on_hover_ui(|ui| {
                ui.label(format!("Prepare + connect: {}", data.connect_ms));
                ui.label(format!("Waiting (TTFB): {}", data.ttfb_ms.saturating_sub(data.connect_ms)));
                ui.label(format!("Download: {}", data.download_ms));
                ui.label(format!("Total: {}", data.total_ms));
            });
            let status = ui.label(RichText::new(format!("{} {}", data.status, data.status_text)).color(theme.status_color(data.status)).strong());
            status.on_hover_ui(|ui| {
                ui.label(format!("{} · {} redirect(s)", data.http_version, data.redirects));
                if let Some(proxy) = &data.proxy {
                    ui.label(format!("Proxy: {proxy}"));
                }
            });
            let _ = language;
        });
    });
    ui.separator();
    match state.tab {
        ResponseTab::Body => body_view(ui, data, state, &theme),
        ResponseTab::Headers => headers_view(ui, data, &theme),
        ResponseTab::Cookies => cookies_view(ui, data, &theme),
    }
    action
}

fn body_view(ui: &mut Ui, data: &mut ResponseData, state: &mut ResponseViewerState, theme: &Theme) {
    ui.horizontal(|ui| {
        for mode in [BodyMode::Pretty, BodyMode::Raw, BodyMode::Preview, BodyMode::Hex] {
            let label = match mode {
                BodyMode::Pretty => "Pretty",
                BodyMode::Raw => "Raw",
                BodyMode::Preview => "Preview",
                BodyMode::Hex => "Hex",
            };
            let mut rich = RichText::new(label).small().color(if state.mode == mode { theme.palette.text } else { theme.palette.text_muted });
            if state.mode == mode {
                rich = rich.strong();
            }
            if ui.add(egui::Button::new(rich).frame(false)).clicked() {
                state.mode = mode;
            }
        }
        ui.separator();
        ui.label(RichText::new(language_label(data)).small().color(theme.palette.text_muted));
        let mut wrap = state.wrap;
        if widgets::checkbox(ui, &mut wrap, "Wrap").changed() {
            state.wrap = wrap;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let search = ui.add(egui::TextEdit::singleline(&mut state.search).hint_text("Search body…").desired_width(170.0));
            let _ = search;
            if !state.search.trim().is_empty() {
                let matches = text_for_search(data, state)
                    .map(|text| text.to_ascii_lowercase().matches(&state.search.to_ascii_lowercase()).count())
                    .unwrap_or(0);
                ui.label(RichText::new(format!("{matches} match(es)")).small().color(theme.palette.text_muted));
            }
        });
    });
    if data.body.len() > PRETTY_CAP {
        ui.label(
            RichText::new(format!("Body is larger than {}; Pretty is disabled (Raw by default).", format_size(PRETTY_CAP as u64)))
                .color(theme.palette.warning),
        );
    }
    let rendered: Option<Arc<str>> = match state.mode {
        BodyMode::Pretty => {
            if data.body.len() > PRETTY_CAP {
                data.text().map(Arc::<str>::from)
            } else {
                data.pretty(state)
            }
        }
        BodyMode::Raw | BodyMode::Preview => {
            if state.mode == BodyMode::Preview && data.content_type().is_some_and(is_image) {
                None
            } else {
                data.text().map(Arc::<str>::from)
            }
        }
        BodyMode::Hex => None,
    };
    if state.mode == BodyMode::Hex {
        hex_view(ui, data, theme);
        return;
    }
    match rendered {
        Some(text) => {
            let view = LargeTextView::new(text).id(egui::Id::new(("response-body", state.mode as u8))).wrap(state.wrap);
            let _ = view.show(ui);
        }
        None => {
            ui.add_space(12.0);
            ui.label(
                RichText::new(format!("Binary body · {} · {}", data.content_type().unwrap_or("unknown type"), format_size(data.body.len() as u64)))
                    .color(theme.palette.text_muted),
            );
            if data.content_type().is_some_and(is_image) {
                ui.label(
                    RichText::new("Image preview arrives with our decoders; Save Response keeps the bytes.").small().color(theme.palette.text_muted),
                );
            }
        }
    }
}

fn headers_view(ui: &mut Ui, data: &ResponseData, theme: &Theme) {
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (name, value) in &data.headers {
            ui.horizontal(|ui| {
                ui.add_sized([180.0, 20.0], egui::Label::new(RichText::new(name).monospace().color(theme.palette.text_muted)));
                ui.add(egui::Label::new(RichText::new(value).monospace().color(theme.palette.text)).selectable(true));
            });
        }
    });
    let _ = data;
}

fn cookies_view(ui: &mut Ui, data: &ResponseData, theme: &Theme) {
    let rows = set_cookie_rows(&data.headers);
    if rows.is_empty() {
        ui.label(RichText::new("No cookies were set by this response.").color(theme.palette.text_muted));
        return;
    }
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for row in rows {
            ui.horizontal(|ui| {
                ui.add_sized([160.0, 20.0], egui::Label::new(RichText::new(row.name).monospace().color(theme.palette.text)));
                ui.add_sized([220.0, 20.0], egui::Label::new(RichText::new(row.value).monospace().color(theme.palette.text)).truncate());
                ui.label(RichText::new(row.attributes).small().color(theme.palette.text_muted));
            });
        }
    });
}

struct CookieRow {
    name: String,
    value: String,
    attributes: String,
}

/// Parse `Set-Cookie` values for display (the jar itself lives in the engine).
fn set_cookie_rows(headers: &[(String, String)]) -> Vec<CookieRow> {
    headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .filter_map(|(_, value)| {
            let mut parts = value.split(';');
            let pair = parts.next()?.trim();
            let (name, value) = pair.split_once('=')?;
            let attributes = parts.map(str::trim).filter(|part| !part.is_empty()).collect::<Vec<&str>>().join("; ");
            Some(CookieRow { name: name.trim().to_string(), value: value.trim().to_string(), attributes })
        })
        .collect()
}

/// Hex dump of the visible rows only (16 bytes per line).
fn hex_view(ui: &mut Ui, data: &ResponseData, theme: &Theme) {
    let bytes_per_line = 16usize;
    let lines = data.body.len().div_ceil(bytes_per_line);
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
    ScrollArea::vertical().id_salt("response-hex").auto_shrink([false, false]).show_rows(ui, row_height, lines, |ui, range| {
        for line in range {
            let start = line.saturating_mul(bytes_per_line);
            let end = (start + bytes_per_line).min(data.body.len());
            let slice = data.body.get(start..end).unwrap_or_default();
            let hex = slice.iter().map(|byte| format!("{byte:02x}")).collect::<Vec<String>>().join(" ");
            let ascii: String = slice.iter().map(|byte| if byte.is_ascii_graphic() { *byte as char } else { '.' }).collect();
            ui.label(RichText::new(format!("{:08x}  {hex:<47}  {ascii}", start)).monospace().color(theme.palette.text));
        }
    });
}

fn text_for_search(data: &ResponseData, state: &ResponseViewerState) -> Option<String> {
    match state.mode {
        BodyMode::Hex => None,
        _ => data.text(),
    }
}

fn language_label(data: &ResponseData) -> &'static str {
    if data.looks_json() {
        "JSON"
    } else if data.content_type().is_some_and(|value| value.contains("xml")) {
        "XML"
    } else if data.content_type().is_some_and(|value| value.contains("html")) {
        "HTML"
    } else {
        "Text"
    }
}

fn is_image(content_type: &str) -> bool {
    content_type.starts_with("image/")
}

/// `7.63 KB` style sizes, matching the strip.
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(json: bool) -> ResponseData {
        let body = if json { b"{\"b\":2,\"a\":1}".to_vec() } else { b"hello".to_vec() };
        ResponseData {
            status: 200,
            status_text: "OK".into(),
            http_version: "HTTP/1.1".into(),
            headers: vec![
                ("content-type".into(), if json { "application/json".into() } else { "text/plain".into() }),
                ("set-cookie".into(), "sid=abc; Path=/; HttpOnly".into()),
            ],
            body,
            header_bytes: 120,
            body_encoded_bytes: 18,
            body_decoded_bytes: 18,
            content_encoding: None,
            connect_ms: 2,
            ttfb_ms: 10,
            total_ms: 14,
            download_ms: 4,
            redirects: 0,
            proxy: None,
        }
    }

    #[test]
    fn json_bodies_pretty_print_and_are_cached() {
        let mut data = data(true);
        let mut state = ResponseViewerState::default();
        let first = data.pretty(&mut state).expect("pretty");
        assert!(first.contains("\"a\": 1"));
        assert!(state.pretty.is_some());
        let second = data.pretty(&mut state).expect("cached");
        assert_eq!(first, second);
    }

    #[test]
    fn cookies_parse_from_set_cookie() {
        let rows = set_cookie_rows(&data(true).headers);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "sid");
        assert_eq!(rows[0].value, "abc");
        assert!(rows[0].attributes.contains("HttpOnly"));
    }

    #[test]
    fn sizes_read_like_the_strip() {
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1536), "1.50 KB");
        assert_eq!(format_size(2 * 1024 * 1024), "2.00 MB");
    }

    #[test]
    fn large_bodies_skip_pretty() {
        let mut data = data(true);
        data.body = vec![b'x'; PRETTY_CAP + 1];
        let mut state = ResponseViewerState::default();
        assert!(data.pretty(&mut state).is_none());
    }
}
