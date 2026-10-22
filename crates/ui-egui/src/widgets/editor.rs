//! Code editor and large-text viewer (PROMPT.md 6.3, 5.4):
//! - [`CodeEditor`]: monospace editing with a line-number gutter that scrolls with the text,
//!   a per-language token colourer and a `{{var}}`-aware plain mode. Above the soft cap it
//!   turns highlighting off instead of stalling (PROMPT.md 5.4: 5 MB).
//! - [`LargeTextView`]: read-only virtualised lines over an `Arc<str>` with precomputed line
//!   starts; only visible rows are laid out and coloured.
//!
//! Every tokenizer walks bytes with `get()` and only slices at ASCII boundaries, so a malformed
//! or truncated body can never panic (AGENTS.md section 4).

use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Color32, FontId, Response, ScrollArea, Sense, TextEdit, TextFormat, Ui};

use crate::theme::{Palette, Theme};
use crate::widgets::field::variable_spans;

/// Which token rules to apply. Unknown languages stay plain (no half-correct colouring).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodeLanguage {
    #[default]
    Plain,
    Json,
    Xml,
    Html,
    JavaScript,
    GraphQl,
    Yaml,
    Shell,
    Http,
    Python,
    Go,
    Rust,
    PowerShell,
}

impl CodeLanguage {
    pub fn label(&self) -> &'static str {
        match self {
            CodeLanguage::Plain => "Text",
            CodeLanguage::Json => "JSON",
            CodeLanguage::Xml => "XML",
            CodeLanguage::Html => "HTML",
            CodeLanguage::JavaScript => "JavaScript",
            CodeLanguage::GraphQl => "GraphQL",
            CodeLanguage::Yaml => "YAML",
            CodeLanguage::Shell => "Shell",
            CodeLanguage::Http => "HTTP",
            CodeLanguage::Python => "Python",
            CodeLanguage::Go => "Go",
            CodeLanguage::Rust => "Rust",
            CodeLanguage::PowerShell => "PowerShell",
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
    /// Generated snippets stay selectable but not editable.
    pub read_only: bool,
    pub id: egui::Id,
}

impl CodeEditor {
    pub fn new(language: CodeLanguage) -> CodeEditor {
        CodeEditor { language, rows: 12, highlight_limit: 2_000_000, read_only: false, id: egui::Id::new("code-editor") }
    }

    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn id(mut self, id: egui::Id) -> Self {
        self.id = id;
        self
    }

    /// Show the editor inside a scroll area. The line-number gutter is painted from the edit's
    /// own galley rows, so numbers scroll with the text and follow soft wraps.
    pub fn show(self, ui: &mut Ui, text: &mut String) -> Response {
        let theme = Theme::current(ui.ctx());
        let palette = theme.palette;
        let font = FontId::monospace(theme.metrics.code_font);
        let line_count = text.lines().count().max(self.rows).max(1);
        let char_w = ui.painter().layout_no_wrap(" ".into(), font.clone(), Color32::PLACEHOLDER).size().x;
        let digits = line_count.ilog10() as usize + 1;
        let gutter_w = (digits as f32 + 1.0) * char_w + 12.0;
        let mut response = ui.label("");
        egui::Frame::new()
            .fill(palette.input_bg)
            .stroke(egui::Stroke::new(1.0, palette.border))
            .corner_radius(egui::CornerRadius::same(theme.metrics.radius as u8))
            .inner_margin(egui::Margin::symmetric(0, 6))
            .show(ui, |ui| {
                ScrollArea::vertical().id_salt((self.id, "scroll")).auto_shrink([false, false]).max_height(ui.available_height()).show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let gutter_left = ui.cursor().min.x;
                        ui.add_space(gutter_w + 8.0);
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
                        let output = TextEdit::multiline(text)
                            .id(self.id)
                            .font(font.clone())
                            .code_editor()
                            .desired_rows(line_count)
                            .desired_width(f32::INFINITY)
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::ZERO)
                            .interactive(!self.read_only)
                            .layouter(&mut layouter)
                            .show(ui);
                        let cursor_row = output.cursor_range.map(|range| text.chars().take(range.primary.index.0).filter(|c| *c == '\n').count());
                        let right = gutter_left + gutter_w - 6.0;
                        let mut logical = 0usize;
                        let mut starts_line = true;
                        for placed in &output.galley.rows {
                            if starts_line {
                                let color = if cursor_row == Some(logical) { palette.text } else { palette.line_number };
                                let pos = egui::pos2(right, output.galley_pos.y + placed.pos.y);
                                ui.painter().text(pos, egui::Align2::RIGHT_TOP, (logical + 1).to_string(), font.clone(), color);
                                logical += 1;
                            }
                            starts_line = placed.ends_with_newline;
                        }
                        response = output.response.response;
                    });
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

/// Colour one body for `language`. `wrap_width` is honored by the returned job; an empty body
/// still yields a job (so the caller can highlight incrementally).
pub fn highlight_job(language: CodeLanguage, text: &str, palette: &Palette, font: FontId, wrap_width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.wrap.break_anywhere = false;
    let mut highlighter = Highlighter { text, job: &mut job, font, palette, cursor: 0 };
    match language {
        CodeLanguage::Plain => plain_tokens(&mut highlighter),
        CodeLanguage::Json => json_tokens(&mut highlighter),
        CodeLanguage::Xml | CodeLanguage::Html => markup_tokens(&mut highlighter),
        CodeLanguage::GraphQl => graphql_tokens(&mut highlighter),
        CodeLanguage::Yaml
        | CodeLanguage::Shell
        | CodeLanguage::Http
        | CodeLanguage::JavaScript
        | CodeLanguage::Python
        | CodeLanguage::Go
        | CodeLanguage::Rust
        | CodeLanguage::PowerShell => code_tokens(language, &mut highlighter),
    }
    if highlighter.cursor < text.len() {
        highlighter.push(highlighter.cursor, text.len(), palette.text, None);
    }
    if text.is_empty() {
        // An empty job has zero height, which misplaces the caret.
        highlighter.raw(0, 0, palette.text, None);
    }
    job
}

/// Appends tokens in increasing byte order; anything not claimed stays the default text colour.
struct Highlighter<'a> {
    text: &'a str,
    job: &'a mut LayoutJob,
    font: FontId,
    palette: &'a Palette,
    cursor: usize,
}

impl Highlighter<'_> {
    fn push(&mut self, start: usize, end: usize, color: Color32, background: Option<Color32>) {
        let start = start.max(self.cursor);
        let end = end.min(self.text.len());
        if end <= start {
            return;
        }
        if start > self.cursor {
            self.raw(self.cursor, start, self.palette.text, None);
        }
        self.raw(start, end, color, background);
        self.cursor = end;
    }

    fn raw(&mut self, start: usize, end: usize, color: Color32, background: Option<Color32>) {
        let Some(slice) = self.text.get(start..end) else { return };
        let format = TextFormat { font_id: self.font.clone(), color, background: background.unwrap_or(Color32::TRANSPARENT), ..Default::default() };
        self.job.append(slice, 0.0, format);
    }

    /// Push a quoted literal, splitting any `{{var}}` inside it into the variable chip colour.
    fn push_literal(&mut self, start: usize, end: usize, color: Color32) {
        let text = self.text;
        let variable = self.palette.code_variable;
        let chip = variable.gamma_multiply(0.14);
        let Some(slice) = text.get(start..end) else {
            self.push(start, end, color, None);
            return;
        };
        let mut cursor = start;
        for (span_start, span_end) in variable_spans(slice) {
            let (absolute_start, absolute_end) = (start.saturating_add(span_start), start.saturating_add(span_end));
            if absolute_start > cursor {
                self.push(cursor, absolute_start, color, None);
            }
            self.push(absolute_start, absolute_end, variable, Some(chip));
            cursor = absolute_end;
        }
        if cursor < end {
            self.push(cursor, end, color, None);
        }
    }
}

/// The byte end of a `{{var}}` span starting at `start`, when there is one.
fn variable_end(text: &str, start: usize) -> Option<usize> {
    let tail = text.get(start..)?;
    if !tail.starts_with("{{") {
        return None;
    }
    let inner = tail.get(2..)?.find('}')?;
    let close = tail.get(2 + inner + 1..)?;
    if !close.starts_with('}') {
        return None;
    }
    Some(start + 2 + inner + 2)
}

fn line_end(bytes: &[u8], from: usize) -> usize {
    let mut index = from;
    while let Some(byte) = bytes.get(index) {
        if *byte == b'\n' {
            break;
        }
        index += 1;
    }
    index
}

/// The end of a quoted literal at `start` (delimiter included when it is closed).
fn quoted_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let Some(&quote) = bytes.get(start) else { return start };
    let mut index = start + 1;
    while let Some(byte) = bytes.get(index) {
        match *byte {
            b'\\' => index += 2,
            byte if byte == quote => return (index + 1).min(bytes.len()),
            b'\n' => return index,
            _ => index += 1,
        }
    }
    bytes.len()
}

fn plain_tokens(highlighter: &mut Highlighter) {
    let text = highlighter.text;
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes.get(index).copied().unwrap_or_default();
        if byte == b'{'
            && let Some(end) = variable_end(text, index)
        {
            let variable = highlighter.palette.code_variable;
            highlighter.push(index, end, variable, Some(variable.gamma_multiply(0.14)));
            index = end;
            continue;
        }
        if text.get(index..).is_some_and(|tail| tail.starts_with("http://") || tail.starts_with("https://")) {
            let mut end = index;
            while let Some(byte) = bytes.get(end) {
                if byte.is_ascii_whitespace() || matches!(byte, b'"' | b'\'' | b'<' | b'>' | b')' | b']' | b',' | b'`') {
                    break;
                }
                end += 1;
            }
            let url = highlighter.palette.code_string;
            highlighter.push(index, end, url, None);
            index = end;
            continue;
        }
        index += 1;
    }
}

fn json_tokens(highlighter: &mut Highlighter) {
    let text = highlighter.text;
    let bytes = text.as_bytes();
    let key = highlighter.palette.code_key;
    let string = highlighter.palette.code_string;
    let number = highlighter.palette.code_number;
    let keyword = highlighter.palette.code_keyword;
    let punct = highlighter.palette.code_punct;
    let variable = highlighter.palette.code_variable;
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes.get(index).copied().unwrap_or_default();
        match byte {
            b'{' if bytes.get(index + 1) == Some(&b'{') => {
                if let Some(end) = variable_end(text, index) {
                    highlighter.push(index, end, variable, Some(variable.gamma_multiply(0.14)));
                    index = end;
                } else {
                    highlighter.push(index, index + 1, punct, None);
                    index += 1;
                }
            }
            b'"' => {
                let end = quoted_end(text, index);
                // A string followed by a colon is an object key, not a value.
                let mut next = end;
                while bytes.get(next).is_some_and(u8::is_ascii_whitespace) {
                    next += 1;
                }
                let color = if bytes.get(next) == Some(&b':') { key } else { string };
                highlighter.push_literal(index, end, color);
                index = end;
            }
            b'0'..=b'9' | b'-' => {
                let mut end = index + 1;
                while bytes.get(end).is_some_and(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'e' | b'E' | b'+' | b'-')) {
                    end += 1;
                }
                highlighter.push(index, end, number, None);
                index = end;
            }
            b't' | b'f' | b'n' => {
                let mut end = index;
                while bytes.get(end).is_some_and(u8::is_ascii_alphabetic) {
                    end += 1;
                }
                let word = text.get(index..end).unwrap_or_default();
                if matches!(word, "true" | "false" | "null") {
                    highlighter.push(index, end, keyword, None);
                }
                index = end;
            }
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                highlighter.push(index, index + 1, punct, None);
                index += 1;
            }
            _ => index += 1,
        }
    }
}

fn markup_tokens(highlighter: &mut Highlighter) {
    let text = highlighter.text;
    let bytes = text.as_bytes();
    let tag = highlighter.palette.code_tag;
    let string = highlighter.palette.code_string;
    let comment = highlighter.palette.code_comment;
    let keyword = highlighter.palette.code_keyword;
    let attr = highlighter.palette.code_attr;
    let number = highlighter.palette.code_number;
    let punct = highlighter.palette.code_punct;
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes.get(index).copied().unwrap_or_default();
        if byte == b'&' {
            let mut end = index + 1;
            while let Some(next) = bytes.get(end) {
                if next == &b';' {
                    break;
                }
                if !next.is_ascii_alphanumeric() || end - index > 12 {
                    end += 1;
                    break;
                }
                end += 1;
            }
            if bytes.get(end) == Some(&b';') {
                highlighter.push(index, end + 1, number, None);
                index = end + 1;
            } else {
                index += 1;
            }
            continue;
        }
        if byte != b'<' {
            index += 1;
            continue;
        }
        if text.get(index..).is_some_and(|tail| tail.starts_with("<!--")) {
            let end = text.get(index..).and_then(|tail| tail.find("-->")).map_or(bytes.len(), |offset| index + offset + 3);
            highlighter.push(index, end, comment, None);
            index = end;
            continue;
        }
        let declaration = text.get(index..).is_some_and(|tail| tail.starts_with("<!") || tail.starts_with("<?"));
        let mut end = index + 1;
        let mut quote: Option<u8> = None;
        while let Some(byte) = bytes.get(end) {
            match quote {
                Some(quote_byte) => {
                    if *byte == quote_byte {
                        quote = None;
                    }
                }
                None => {
                    if *byte == b'"' || *byte == b'\'' {
                        quote = Some(*byte);
                    } else if *byte == b'>' {
                        break;
                    }
                }
            }
            end += 1;
        }
        let end = (end + 1).min(bytes.len());
        if declaration {
            highlighter.push(index, end, keyword, None);
            index = end;
            continue;
        }
        // A start/end tag: delimiters and the tag name in the tag colour, attribute names in the
        // attribute colour, attribute strings in the string colour.
        let mut cursor = index;
        highlighter.push(cursor, cursor + 1, tag, None);
        cursor += 1;
        if bytes.get(cursor) == Some(&b'/') {
            highlighter.push(cursor, cursor + 1, tag, None);
            cursor += 1;
        }
        let name_start = cursor;
        while bytes.get(cursor).is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_')) {
            cursor += 1;
        }
        if cursor > name_start {
            highlighter.push(name_start, cursor, tag, None);
        }
        while cursor < end {
            let byte = bytes.get(cursor).copied().unwrap_or(b'>');
            if byte == b'/' || byte == b'>' {
                highlighter.push(cursor, (cursor + 1).min(end), tag, None);
                cursor += 1;
                continue;
            }
            if byte == b'=' {
                highlighter.push(cursor, cursor + 1, punct, None);
                cursor += 1;
                continue;
            }
            if byte == b'"' || byte == b'\'' {
                let literal_end = quoted_end(text, cursor).min(end);
                highlighter.push_literal(cursor, literal_end, string);
                cursor = literal_end.max(cursor + 1);
                continue;
            }
            if byte.is_ascii_alphabetic() || byte == b'_' {
                let attribute_start = cursor;
                while bytes.get(cursor).is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')) {
                    cursor += 1;
                }
                highlighter.push(attribute_start, cursor, attr, None);
                continue;
            }
            cursor += 1;
        }
        index = end;
    }
}

fn graphql_tokens(highlighter: &mut Highlighter) {
    let text = highlighter.text;
    let bytes = text.as_bytes();
    let key = highlighter.palette.code_key;
    let string = highlighter.palette.code_string;
    let comment = highlighter.palette.code_comment;
    let keyword_color = highlighter.palette.code_keyword;
    let variable = highlighter.palette.code_variable;
    let number = highlighter.palette.code_number;
    let punct = highlighter.palette.code_punct;
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes.get(index).copied().unwrap_or_default();
        match byte {
            b'#' => {
                let end = line_end(bytes, index);
                highlighter.push(index, end, comment, None);
                index = end;
            }
            b'"' => {
                let end = if text.get(index..).is_some_and(|tail| tail.starts_with("\"\"\"")) {
                    text.get(index + 3..).and_then(|tail| tail.find("\"\"\"")).map_or(bytes.len(), |offset| index + offset + 6)
                } else {
                    quoted_end(text, index)
                };
                highlighter.push_literal(index, end, string);
                index = end.max(index + 1);
            }
            b'$' | b'@' => {
                let mut end = index + 1;
                while bytes.get(end).is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')) {
                    end += 1;
                }
                highlighter.push(index, end, variable, None);
                index = end;
            }
            b'0'..=b'9' | b'-' => {
                let mut end = index + 1;
                while bytes.get(end).is_some_and(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'e' | b'E')) {
                    end += 1;
                }
                highlighter.push(index, end, number, None);
                index = end;
            }
            b'{' | b'}' | b'(' | b')' | b'[' | b']' | b':' | b',' | b'!' | b'|' | b'=' => {
                highlighter.push(index, index + 1, punct, None);
                index += 1;
            }
            _ if byte.is_ascii_alphabetic() || byte == b'_' => {
                let mut end = index;
                while bytes.get(end).is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_') {
                    end += 1;
                }
                let word = text.get(index..end).unwrap_or_default();
                let color = if keyword(GRAPHQL_KEYWORDS, word) {
                    keyword_color
                } else if word.starts_with(|character: char| character.is_ascii_uppercase()) {
                    number
                } else {
                    key
                };
                highlighter.push(index, end, color, None);
                index = end;
            }
            _ => index += 1,
        }
    }
}

const GRAPHQL_KEYWORDS: &[&str] = &[
    "query",
    "mutation",
    "subscription",
    "fragment",
    "on",
    "type",
    "input",
    "enum",
    "interface",
    "union",
    "scalar",
    "schema",
    "extend",
    "directive",
    "true",
    "false",
    "null",
];

const HTTP_KEYWORDS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT", "HTTP", "HTTP/1.1", "HTTP/2"];

const JAVASCRIPT_KEYWORDS: &[&str] = &[
    "const", "let", "var", "function", "async", "await", "return", "if", "else", "for", "while", "import", "from", "export", "new", "try", "catch",
    "throw", "class", "typeof", "of", "in", "this",
];

const SHELL_KEYWORDS: &[&str] = &["curl", "wget", "echo", "export", "sudo", "then", "fi", "do", "done", "if", "else", "for", "while", "in"];

const PYTHON_KEYWORDS: &[&str] = &[
    "def", "lambda", "pass", "with", "as", "elif", "not", "and", "or", "raise", "yield", "global", "import", "from", "class", "return", "if", "else",
    "for", "while", "in",
];

const GO_KEYWORDS: &[&str] = &[
    "package",
    "func",
    "struct",
    "interface",
    "chan",
    "go",
    "defer",
    "range",
    "type",
    "map",
    "make",
    "var",
    "const",
    "import",
    "return",
    "if",
    "else",
    "for",
    "switch",
    "case",
];

const RUST_KEYWORDS: &[&str] = &[
    "fn", "pub", "use", "mut", "impl", "trait", "match", "enum", "mod", "crate", "where", "let", "const", "struct", "ref", "move", "async", "await",
    "dyn", "Box", "Some", "Ok", "Err", "Self",
];

const POWERSHELL_KEYWORDS: &[&str] =
    &["Invoke-RestMethod", "Invoke-WebRequest", "param", "foreach", "function", "Write-Host", "Get-Item", "if", "else", "return"];

const YAML_KEYWORDS: &[&str] = &["true", "false", "null", "yes", "no", "on", "off"];

/// Case-insensitive keyword test (HTTP methods are uppercase, everything else is lowercase).
fn keyword(words: &[&str], word: &str) -> bool {
    words.iter().any(|candidate| candidate.eq_ignore_ascii_case(word))
}

fn boolean(word: &str) -> bool {
    matches!(word, "true" | "false" | "null" | "None" | "True" | "False" | "nil" | "undefined")
}

fn keywords(language: CodeLanguage) -> &'static [&'static str] {
    match language {
        CodeLanguage::JavaScript => JAVASCRIPT_KEYWORDS,
        CodeLanguage::Shell => SHELL_KEYWORDS,
        CodeLanguage::Http => HTTP_KEYWORDS,
        CodeLanguage::Python => PYTHON_KEYWORDS,
        CodeLanguage::Go => GO_KEYWORDS,
        CodeLanguage::Rust => RUST_KEYWORDS,
        CodeLanguage::PowerShell => POWERSHELL_KEYWORDS,
        CodeLanguage::Yaml => YAML_KEYWORDS,
        _ => &[],
    }
}

fn code_tokens(language: CodeLanguage, highlighter: &mut Highlighter) {
    let text = highlighter.text;
    let bytes = text.as_bytes();
    let key = highlighter.palette.code_key;
    let string = highlighter.palette.code_string;
    let comment = highlighter.palette.code_comment;
    let keyword_color = highlighter.palette.code_keyword;
    let number = highlighter.palette.code_number;
    let variable = highlighter.palette.code_variable;
    let hash_comments = matches!(language, CodeLanguage::Shell | CodeLanguage::Python | CodeLanguage::Yaml | CodeLanguage::PowerShell);
    let slash_comments = matches!(language, CodeLanguage::JavaScript | CodeLanguage::Go | CodeLanguage::Rust);
    let cli_flags = matches!(language, CodeLanguage::Shell | CodeLanguage::Http | CodeLanguage::PowerShell);
    let colon_keys = matches!(language, CodeLanguage::Http | CodeLanguage::Yaml);
    let backtick_strings = matches!(language, CodeLanguage::JavaScript | CodeLanguage::Shell | CodeLanguage::PowerShell);
    let words = keywords(language);
    let mut index = 0usize;
    let mut at_line_start = true;
    while index < bytes.len() {
        let byte = bytes.get(index).copied().unwrap_or_default();
        if byte == b'\n' {
            at_line_start = true;
            index += 1;
            continue;
        }
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if byte == b'{'
            && let Some(end) = variable_end(text, index)
        {
            highlighter.push(index, end, variable, Some(variable.gamma_multiply(0.14)));
            index = end;
            at_line_start = false;
            continue;
        }
        if hash_comments && byte == b'#' {
            let end = line_end(bytes, index);
            highlighter.push(index, end, comment, None);
            index = end;
            continue;
        }
        if slash_comments && byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            let end = line_end(bytes, index);
            highlighter.push(index, end, comment, None);
            index = end;
            continue;
        }
        if slash_comments && byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            let end = text.get(index..).and_then(|tail| tail.find("*/")).map_or(bytes.len(), |offset| index + offset + 2);
            highlighter.push(index, end, comment, None);
            index = end;
            continue;
        }
        if byte == b'"' || byte == b'\'' || (byte == b'`' && backtick_strings) {
            let end = quoted_end(text, index);
            highlighter.push_literal(index, end, string);
            index = end.max(index + 1);
            at_line_start = false;
            continue;
        }
        if colon_keys && at_line_start {
            let mut key_start = index;
            if language == CodeLanguage::Yaml && byte == b'-' && bytes.get(key_start + 1) == Some(&b' ') {
                key_start += 1;
                while bytes.get(key_start) == Some(&b' ') {
                    key_start += 1;
                }
            }
            let mut end = key_start;
            while bytes.get(end).is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')) {
                end += 1;
            }
            if end > key_start && bytes.get(end) == Some(&b':') {
                highlighter.push(key_start, end, key, None);
                index = end;
                at_line_start = false;
                continue;
            }
        }
        if cli_flags && byte == b'-' && bytes.get(index + 1).is_some_and(u8::is_ascii_alphabetic) {
            let mut end = index + 1;
            while bytes.get(end).is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) {
                end += 1;
            }
            highlighter.push(index, end, key, None);
            index = end;
            continue;
        }
        if byte.is_ascii_digit() {
            let mut end = index + 1;
            while bytes.get(end).is_some_and(|byte| byte.is_ascii_digit() || *byte == b'.') {
                end += 1;
            }
            highlighter.push(index, end, number, None);
            index = end;
            at_line_start = false;
            continue;
        }
        if byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$' {
            let mut end = index;
            while bytes.get(end).is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'-')) {
                end += 1;
            }
            let word = text.get(index..end).unwrap_or_default();
            let color = if boolean(word) || word == "nil" {
                number
            } else if word.starts_with('$') || matches!(word, "pm" | "console" | "expect" | "postman") {
                key
            } else if keyword(words, word) {
                keyword_color
            } else {
                highlighter.palette.text
            };
            highlighter.push(index, end, color, None);
            index = end;
            at_line_start = false;
            continue;
        }
        index += 1;
        at_line_start = false;
    }
}

/// Tint every case-insensitive match of `needle` by splitting the job's sections. ASCII-only
/// needles keep byte offsets stable; a needle that changes length when lowercased is skipped.
fn tint_matches(job: &mut LayoutJob, needle: &str, tint: Color32) {
    let needle = needle.trim();
    if needle.is_empty() {
        return;
    }
    let text = job.text.clone();
    let lowered = text.to_lowercase();
    let needle_lowered = needle.to_lowercase();
    if lowered.len() != text.len() || needle_lowered.len() != needle.len() {
        return;
    }
    let mut matches: Vec<(usize, usize)> = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = lowered.get(from..).and_then(|tail| tail.find(&needle_lowered)) {
        let start = from + offset;
        let end = start + needle_lowered.len();
        matches.push((start, end));
        from = end.max(start + 1);
        if matches.len() >= 512 {
            break;
        }
    }
    if matches.is_empty() {
        return;
    }
    let mut sections = Vec::with_capacity(job.sections.len() + matches.len() * 2);
    let mut push = |range: std::ops::Range<usize>, format: TextFormat| {
        if range.start < range.end {
            sections.push(egui::text::LayoutSection {
                leading_space: 0.0,
                byte_range: egui::text::ByteIndex(range.start)..egui::text::ByteIndex(range.end),
                format,
            });
        }
    };
    for section in &job.sections {
        let range = section.byte_range.start.0..section.byte_range.end.0;
        let mut cursor = range.start;
        for (match_start, match_end) in &matches {
            let (start, end) = (*match_start, *match_end);
            if end <= cursor || start >= range.end {
                continue;
            }
            let start = start.max(cursor);
            let end = end.min(range.end);
            push(cursor..start, section.format.clone());
            let mut tinted = section.format.clone();
            tinted.background = tint;
            push(start..end, tinted);
            cursor = end;
        }
        push(cursor..range.end, section.format.clone());
    }
    job.sections = sections;
}

/// A physical line may be split into several display rows so one minified 10 MB line never
/// becomes a single galley (the layout cost is what freezes the UI, not the line index).
const MAX_DISPLAY_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy)]
struct DisplayLine {
    start: usize,
    end: usize,
    /// 1-based physical line number shown in the gutter.
    number: usize,
}

/// A read-only viewer for large bodies. Line starts are computed once in [`LargeTextView::new`];
/// only the visible rows are materialised each frame.
#[derive(Debug, Clone)]
pub struct LargeTextView {
    text: Arc<str>,
    line_starts: Vec<usize>,
    display: Vec<DisplayLine>,
    pub wrap: bool,
    pub line_numbers: bool,
    pub id: egui::Id,
    pub language: CodeLanguage,
    pub search: String,
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
        let mut display = Vec::with_capacity(line_starts.len());
        for (index, start) in line_starts.iter().copied().enumerate() {
            let end = line_starts.get(index + 1).copied().unwrap_or(text.len());
            let mut chunk = start;
            while chunk < end {
                let mut chunk_end = (chunk + MAX_DISPLAY_BYTES).min(end);
                while chunk_end > chunk && !text.is_char_boundary(chunk_end) {
                    chunk_end -= 1;
                }
                if chunk_end == chunk {
                    chunk_end = end;
                }
                display.push(DisplayLine { start: chunk, end: chunk_end, number: index + 1 });
                chunk = chunk_end;
            }
            if start == end {
                display.push(DisplayLine { start, end, number: index + 1 });
            }
        }
        LargeTextView {
            text,
            line_starts,
            display,
            wrap: false,
            line_numbers: true,
            id: egui::Id::new("large-text"),
            language: CodeLanguage::Plain,
            search: String::new(),
        }
    }

    pub fn id(mut self, id: egui::Id) -> Self {
        self.id = id;
        self
    }

    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    pub fn language(mut self, language: CodeLanguage) -> Self {
        self.language = language;
        self
    }

    pub fn search(mut self, search: impl Into<String>) -> Self {
        self.search = search.into();
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
        let palette = theme.palette;
        let font = FontId::monospace(theme.metrics.code_font);
        let row_height = (ui.text_style_height(&egui::TextStyle::Monospace) + 4.0).max(20.0);
        let char_w = ui.painter().layout_no_wrap(" ".into(), font.clone(), Color32::PLACEHOLDER).size().x;
        let digits = self.line_starts.len().max(1).ilog10() as usize + 1;
        let gutter = if self.line_numbers { (digits as f32 + 1.0) * char_w + 12.0 } else { 0.0 };
        let tint = palette.warning.gamma_multiply(0.28);
        let guide = egui::Stroke::new(1.0, palette.guide);
        let mut clicked: Option<Response> = None;
        egui::Frame::new()
            .fill(palette.input_bg)
            .stroke(egui::Stroke::new(1.0, palette.border))
            .corner_radius(egui::CornerRadius::same(theme.metrics.radius as u8))
            .inner_margin(egui::Margin::symmetric(0, 6))
            .show(ui, |ui| {
                ScrollArea::vertical().id_salt((self.id, "scroll")).auto_shrink([false, false]).show_rows(
                    ui,
                    row_height,
                    self.display.len(),
                    |ui, range| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for line in range {
                            let Some(display) = self.display.get(line) else { continue };
                            let content = self.text.get(display.start..display.end).unwrap_or_default().trim_end_matches('\n');
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                if self.line_numbers {
                                    let (rect, _) = ui.allocate_exact_size(egui::vec2(gutter, row_height), Sense::hover());
                                    // Continuation chunks of a split line carry no number.
                                    if self.line_starts.get(display.number.saturating_sub(1)) == Some(&display.start) {
                                        ui.painter().text(
                                            egui::pos2(rect.right() - 6.0, rect.center().y),
                                            egui::Align2::RIGHT_CENTER,
                                            display.number.to_string(),
                                            font.clone(),
                                            palette.line_number,
                                        );
                                    }
                                }
                                let wrap_width = if self.wrap { ui.available_width().max(200.0) } else { f32::INFINITY };
                                let mut job = highlight_job(self.language, content, &palette, font.clone(), wrap_width);
                                tint_matches(&mut job, &self.search, tint);
                                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                                let label = egui::Label::new(galley).sense(Sense::click()).selectable(true);
                                let response = ui.add(label);
                                if response.clicked() {
                                    clicked = Some(response.clone());
                                }
                                // Indent guides, one per two leading columns (a tab counts as two).
                                let mut columns = 0usize;
                                for byte in content.bytes() {
                                    match byte {
                                        b' ' => columns += 1,
                                        b'\t' => columns += 2,
                                        _ => break,
                                    }
                                }
                                if columns >= 2 && self.line_starts.get(display.number.saturating_sub(1)) == Some(&display.start) {
                                    let rect = response.rect;
                                    let y_range = (rect.top() - 1.0)..=(rect.bottom() + 1.0);
                                    for level in 1..=columns / 2 {
                                        let x = rect.left() + (level - 1) as f32 * 2.0 * char_w + char_w * 0.5;
                                        ui.painter().vline(x, y_range.clone(), guide);
                                    }
                                }
                            });
                        }
                    },
                );
            });
        clicked.unwrap_or_else(|| ui.label(""))
    }
}

/// A plain colour on the theme's muted text (helpers for the response viewer's toolbar).
pub fn muted(text: impl Into<String>, theme: &Theme) -> egui::RichText {
    egui::RichText::new(text.into()).color(theme.palette.text_muted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Palette;

    fn highlight(language: CodeLanguage, text: &str) -> (LayoutJob, Palette) {
        let palette = Palette::LIGHT;
        let job = highlight_job(language, text, &palette, FontId::monospace(13.0), f32::INFINITY);
        (job, palette)
    }

    fn slice<'a>(job: &'a LayoutJob, section: &egui::text::LayoutSection) -> Option<&'a str> {
        job.text.get(section.byte_range.start.0..section.byte_range.end.0)
    }

    /// The colour each section is painted with, in order.
    fn colours(job: &LayoutJob) -> Vec<(String, Color32)> {
        job.sections.iter().filter_map(|section| Some((slice(job, section)?.to_string(), section.format.color))).collect()
    }

    fn colour_of(job: &LayoutJob, needle: &str) -> Option<Color32> {
        colours(job).into_iter().find(|(text, _)| text == needle).map(|(_, color)| color)
    }

    #[test]
    fn json_separates_keys_values_numbers_and_booleans() {
        let (job, palette) = highlight(CodeLanguage::Json, r#"{"user": "ada", "count": 42, "active": true}"#);
        assert_eq!(colour_of(&job, "\"user\""), Some(palette.code_key), "keys use the key colour");
        assert_eq!(colour_of(&job, "\"ada\""), Some(palette.code_string), "string values use the string colour");
        assert_eq!(colour_of(&job, "42"), Some(palette.code_number), "numbers use the number colour");
        assert_eq!(colour_of(&job, "true"), Some(palette.code_keyword), "booleans use the keyword colour");
    }

    #[test]
    fn json_tints_variables_inside_strings() {
        let (job, palette) = highlight(CodeLanguage::Json, r#"{"id": "{{user_id}}"}"#);
        assert_eq!(colour_of(&job, "{{user_id}}"), Some(palette.code_variable));
    }

    #[test]
    fn every_markup_and_code_language_produces_more_than_one_colour() {
        for (language, source) in [
            (CodeLanguage::Xml, "<root attr=\"1\"><child>text</child></root>"),
            (CodeLanguage::Html, "<!DOCTYPE html><p class=\"x\">hi &amp; bye</p><!-- note -->"),
            (CodeLanguage::JavaScript, "const body = \"x\"; // comment\nawait fetch(url, { method: \"POST\" });"),
            (CodeLanguage::GraphQl, "query User($id: ID!) { user(id: $id) # trailing\n { name } }"),
            (CodeLanguage::Shell, "curl -X GET \"https://api.example.com\" -H 'Accept: application/json'"),
            (CodeLanguage::Rust, "pub fn main() { let count = 3; /* comment */ println!(\"{count}\"); }"),
            (CodeLanguage::Python, "def fetch(url):\n    return requests.get(url)  # comment"),
            (CodeLanguage::Go, "package main\nfunc main() { fmt.Println(\"hi\") }"),
            (CodeLanguage::PowerShell, "Invoke-RestMethod -Uri \"https://example.com\" -Method Get"),
            (CodeLanguage::Http, "GET /users HTTP/1.1\nAccept: application/json\n\n{\"a\": 1}"),
            (CodeLanguage::Yaml, "name: demo\nvalues:\n  - 12\n  - true\n"),
            (CodeLanguage::Plain, "call {{base}}/users and https://example.com"),
        ] {
            let (job, _) = highlight(language, source);
            let mut seen: Vec<Color32> = colours(&job).into_iter().map(|(_, color)| color).collect();
            seen.sort_by_key(|color| color.to_array());
            seen.dedup();
            assert!(seen.len() > 1, "{:?} only painted one colour: {seen:?}", language.label());
        }
    }

    #[test]
    fn http_and_yaml_colour_keys_at_the_start_of_a_line() {
        let (job, palette) = highlight(CodeLanguage::Http, "GET / HTTP/1.1\nContent-Type: application/json");
        assert_eq!(colour_of(&job, "Content-Type"), Some(palette.code_key));
        assert_eq!(colour_of(&job, "GET"), Some(palette.code_keyword));
        let (yaml, palette) = highlight(CodeLanguage::Yaml, "name: demo\n");
        assert_eq!(colour_of(&yaml, "name"), Some(palette.code_key));
    }

    #[test]
    fn plain_text_tints_variables_and_urls() {
        let (job, palette) = highlight(CodeLanguage::Plain, "fetch {{base}}/users then https://example.com/a");
        assert_eq!(colour_of(&job, "{{base}}"), Some(palette.code_variable));
        assert_eq!(colour_of(&job, "https://example.com/a"), Some(palette.code_string));
    }

    #[test]
    fn malformed_and_truncated_input_never_panics() {
        let samples =
            ["\"unterminated", "<div class=\"x", "{{", "{{unclosed", "```", "&amp", "query { user(id: $", "\u{1F600}<p>\u{1F600} \"\u{1F600}\"", ""];
        for language in [
            CodeLanguage::Plain,
            CodeLanguage::Json,
            CodeLanguage::Xml,
            CodeLanguage::Html,
            CodeLanguage::JavaScript,
            CodeLanguage::GraphQl,
            CodeLanguage::Yaml,
            CodeLanguage::Shell,
            CodeLanguage::Http,
            CodeLanguage::Python,
            CodeLanguage::Go,
            CodeLanguage::Rust,
            CodeLanguage::PowerShell,
        ] {
            for sample in samples {
                let (job, _) = highlight(language, sample);
                assert_eq!(job.text, sample, "the job keeps the whole body");
                for section in &job.sections {
                    assert!(slice(&job, section).is_some(), "sections are on char boundaries");
                }
            }
        }
    }

    #[test]
    fn search_matches_are_tinted_without_losing_the_token_colours() {
        let palette = Palette::LIGHT;
        let mut job = highlight_job(CodeLanguage::Json, "{\"status\": \"settled\"}", &palette, FontId::monospace(13.0), f32::INFINITY);
        let before = colours(&job);
        tint_matches(&mut job, "settled", palette.warning.gamma_multiply(0.28));
        let tinted: Vec<&egui::text::LayoutSection> =
            job.sections.iter().filter(|section| section.format.background == palette.warning.gamma_multiply(0.28)).collect();
        assert_eq!(tinted.len(), 1, "one match, one tinted section");
        assert_eq!(tinted.first().and_then(|section| slice(&job, section)), Some("settled"));
        let after = colours(&job);
        assert_eq!(after.len(), before.len() + 2, "the match splits the string into three sections");
        assert_eq!(job.text, "{\"status\": \"settled\"}");
    }

    #[test]
    fn an_empty_or_non_ascii_search_leaves_the_job_alone() {
        let palette = Palette::LIGHT;
        let mut job = highlight_job(CodeLanguage::Plain, "plain", &palette, FontId::monospace(13.0), f32::INFINITY);
        let sections = job.sections.len();
        tint_matches(&mut job, "   ", palette.warning);
        tint_matches(&mut job, "\u{00E9}", palette.warning);
        assert_eq!(job.sections.len(), sections);
    }

    #[test]
    fn the_viewer_keeps_its_language_and_search_while_paginating() {
        let view = LargeTextView::new(Arc::from("{\"a\": 1}\n{\"b\": 2}")).language(CodeLanguage::Json).search("b");
        assert_eq!(view.language, CodeLanguage::Json);
        assert_eq!(view.search, "b");
        assert_eq!(view.text(), "{\"a\": 1}\n{\"b\": 2}");
    }
}
