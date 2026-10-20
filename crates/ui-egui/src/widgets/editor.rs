//! Code editor and large-text viewer (PROMPT.md 6.3, 5.4):
//! - [`CodeEditor`]: monospace editing with a line-number gutter that scrolls with the text,
//!   a JSON token colourer and a `{{var}}`-aware plain mode. Above the soft cap it turns
//!   highlighting off instead of stalling (PROMPT.md 5.4: 5 MB).
//! - [`LargeTextView`]: read-only virtualised lines over an `Arc<str>` with precomputed line
//!   starts; only visible rows are laid out. Hex mode is a later response-viewer concern.

use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{FontId, Response, ScrollArea, Sense, TextEdit, TextFormat, Ui};

use crate::theme::Theme;

/// Which token rules to apply. Unknown languages stay plain (no half-correct colouring).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodeLanguage {
    #[default]
    Plain,
    Json,
    Xml,
    JavaScript,
    GraphQl,
}

impl CodeLanguage {
    pub fn label(&self) -> &'static str {
        match self {
            CodeLanguage::Plain => "Text",
            CodeLanguage::Json => "JSON",
            CodeLanguage::Xml => "XML",
            CodeLanguage::JavaScript => "JavaScript",
            CodeLanguage::GraphQl => "GraphQL",
        }
    }
}

/// Editable code area. Kept small: state is the caller's `String`; the widget adds the gutter,
/// colours and scroll behaviour.
#[derive(Debug, Clone)]
pub struct CodeEditor {
    pub language: CodeLanguage,
    pub rows: usize,
    /// Above this many bytes highlighting is off (typing stays responsive).
    pub highlight_limit: usize,
    pub id: egui::Id,
}

impl CodeEditor {
    pub fn new(language: CodeLanguage) -> CodeEditor {
        CodeEditor { language, rows: 12, highlight_limit: 2_000_000, id: egui::Id::new("code-editor") }
    }

    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows;
        self
    }

    pub fn id(mut self, id: egui::Id) -> Self {
        self.id = id;
        self
    }

    /// Show the editor inside a virtualised scroll area. Line numbers are one multiline label
    /// beside the edit, so both scroll together.
    pub fn show(self, ui: &mut Ui, text: &mut String) -> Response {
        let theme = Theme::current(ui.ctx());
        let palette = theme.palette;
        let font = FontId::monospace(theme.metrics.code_font);
        let line_count = text.lines().count().max(self.rows).max(1);
        let mut response = ui.label("");
        ScrollArea::vertical().id_salt((self.id, "scroll")).auto_shrink([false, false]).max_height(ui.available_height()).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let mut numbers = String::with_capacity(line_count * 4);
                for line in 1..=line_count {
                    numbers.push_str(&format!("{line:>width$}\n", width = (line_count.ilog10() + 1) as usize));
                }
                ui.add(
                    egui::Label::new(egui::RichText::new(numbers).monospace().color(palette.text_muted))
                        .selectable(false)
                        .wrap_mode(egui::TextWrapMode::Extend),
                );
                let language = self.language;
                let highlight = text.len() <= self.highlight_limit;
                let layout_font = font.clone();
                let mut layouter = move |ui: &Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
                    let text = buffer.as_str();
                    let job = if highlight {
                        highlight_job(language, text, &palette, layout_font.clone(), wrap_width)
                    } else {
                        let mut job = LayoutJob::default();
                        job.wrap.max_width = wrap_width;
                        job.append(text, 0.0, TextFormat { font_id: layout_font.clone(), color: palette.text, ..Default::default() });
                        job
                    };
                    ui.fonts_mut(|fonts| fonts.layout_job(job))
                };
                let edit = ui.add(
                    TextEdit::multiline(text)
                        .id(self.id)
                        .font(font)
                        .code_editor()
                        .desired_rows(line_count)
                        .desired_width(f32::INFINITY)
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::ZERO)
                        .layouter(&mut layouter),
                );
                response = edit;
            });
        });
        response
    }
}

impl Default for CodeEditor {
    fn default() -> Self {
        CodeEditor::new(CodeLanguage::Plain)
    }
}

fn highlight_job(language: CodeLanguage, text: &str, palette: &crate::theme::Palette, font: FontId, wrap_width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.wrap.break_anywhere = false;
    match language {
        CodeLanguage::Json => json_tokens(text, palette, &font, &mut job),
        _ => job.append(text, 0.0, TextFormat { font_id: font, color: palette.text, ..Default::default() }),
    }
    job
}

fn json_tokens(text: &str, palette: &crate::theme::Palette, font: &FontId, job: &mut LayoutJob) {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    let mut plain_start = 0usize;
    let flush = |job: &mut LayoutJob, from: usize, to: usize| {
        if to > from {
            job.append(text.get(from..to).unwrap_or_default(), 0.0, TextFormat { font_id: font.clone(), color: palette.text, ..Default::default() });
        }
    };
    while index < bytes.len() {
        let byte = bytes.get(index).copied().unwrap_or_default();
        let (start, end, color) = match byte {
            b'"' => {
                let start = index;
                index += 1;
                while index < bytes.len() {
                    match bytes.get(index).copied() {
                        Some(b'\\') => index += 2,
                        Some(b'"') => {
                            index += 1;
                            break;
                        }
                        _ => index += 1,
                    }
                }
                (start, index.min(bytes.len()), palette.success)
            }
            b'0'..=b'9' | b'-' => {
                let start = index;
                index += 1;
                while bytes.get(index).is_some_and(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'e' | b'E' | b'+' | b'-')) {
                    index += 1;
                }
                (start, index, palette.info)
            }
            b't' | b'f' | b'n' => {
                let start = index;
                while bytes.get(index).is_some_and(u8::is_ascii_alphabetic) {
                    index += 1;
                }
                let word = text.get(start..index).unwrap_or_default();
                if matches!(word, "true" | "false" | "null") { (start, index, palette.warning) } else { (start, index, palette.text) }
            }
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                let start = index;
                index += 1;
                (start, index, palette.text_muted)
            }
            _ => {
                index += 1;
                continue;
            }
        };
        flush(job, plain_start, start);
        job.append(text.get(start..end).unwrap_or_default(), 0.0, TextFormat { font_id: font.clone(), color, ..Default::default() });
        plain_start = end;
    }
    flush(job, plain_start, bytes.len());
}

/// A read-only viewer for large bodies. Line starts are computed once in [`LargeTextView::new`];
/// only the visible rows are materialised each frame.
#[derive(Debug, Clone)]
pub struct LargeTextView {
    text: Arc<str>,
    line_starts: Vec<usize>,
    pub wrap: bool,
    pub line_numbers: bool,
    pub id: egui::Id,
}

impl LargeTextView {
    pub fn new(text: Arc<str>) -> LargeTextView {
        let mut line_starts = Vec::with_capacity(text.len() / 32 + 1);
        line_starts.push(0);
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        LargeTextView { text, line_starts, wrap: false, line_numbers: true, id: egui::Id::new("large-text") }
    }

    pub fn id(mut self, id: egui::Id) -> Self {
        self.id = id;
        self
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The visible-line renderer. Selection works per row (labels are selectable).
    pub fn show(&self, ui: &mut Ui) -> Response {
        let theme = Theme::current(ui.ctx());
        let font = FontId::monospace(theme.metrics.code_font);
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
        let digits = self.line_starts.len().max(1).ilog10() as usize + 1;
        let gutter = if self.line_numbers { (digits as f32 + 1.0) * 8.0 } else { 0.0 };
        let mut clicked: Option<Response> = None;
        ScrollArea::vertical().id_salt((self.id, "scroll")).auto_shrink([false, false]).show_rows(
            ui,
            row_height,
            self.line_starts.len(),
            |ui, range| {
                for line in range {
                    let start = *self.line_starts.get(line).unwrap_or(&0);
                    let end = self.line_starts.get(line + 1).copied().unwrap_or(self.text.len());
                    let content = self.text.get(start..end).unwrap_or_default().trim_end_matches('\n');
                    ui.horizontal(|ui| {
                        if self.line_numbers {
                            ui.add_sized(
                                [gutter, row_height],
                                egui::Label::new(egui::RichText::new(format!("{:>digits$}", line + 1)).monospace().color(theme.palette.text_muted)),
                            );
                        }
                        let mut job = LayoutJob::default();
                        job.wrap.max_width = if self.wrap { ui.available_width().max(200.0) } else { f32::INFINITY };
                        job.append(content, 0.0, TextFormat { font_id: font.clone(), color: theme.palette.text, ..Default::default() });
                        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                        let label = egui::Label::new(galley).sense(Sense::click()).selectable(true);
                        let response = ui.add(label);
                        if response.clicked() {
                            clicked = Some(response);
                        }
                    });
                }
            },
        );
        clicked.unwrap_or_else(|| ui.label(""))
    }
}

/// A plain colour on the theme's muted text (helpers for the response viewer's toolbar).
pub fn muted(text: impl Into<String>, theme: &Theme) -> egui::RichText {
    egui::RichText::new(text.into()).color(theme.palette.text_muted)
}
