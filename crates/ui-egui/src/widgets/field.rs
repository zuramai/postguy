//! A single-line field with `{{variable}}` chips: the text edit is styled by a layouter that
//! tints variable spans with the accent and gives them a subtle background, so the chips move
//! with the text instead of being a separate overlay. The field is also the base for the URL
//! bar (autocomplete and cURL paste arrive in M3.4).

use egui::text::LayoutJob;
use egui::{FontId, Response, TextEdit, TextFormat, Ui};

use crate::theme::Theme;

/// The `{{name}}` spans in `text` (byte offsets, delimiter included). A span stops at the first
/// `}`; an unterminated `{{` is not a variable.
pub fn variable_spans(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        if bytes.get(index) == Some(&b'{') && bytes.get(index + 1) == Some(&b'{') {
            let start = index;
            let mut end = index + 2;
            while end < bytes.len() && bytes.get(end) != Some(&b'}') {
                end += 1;
            }
            if end + 1 < bytes.len() && bytes.get(end + 1) == Some(&b'}') {
                spans.push((start, end + 2));
                index = end + 2;
                continue;
            }
        }
        index += 1;
    }
    spans
}

/// A `{{var}}`-aware single-line text field. Returns the edit response; `text` is updated in
/// place. `hint` is the placeholder.
pub fn variable_field(ui: &mut Ui, id: egui::Id, text: &mut String, hint: &str) -> Response {
    let theme = Theme::current(ui.ctx());
    let palette = theme.palette;
    let font = FontId::proportional(theme.metrics.ui_font);
    let mut layouter = move |ui: &Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
        let text = buffer.as_str();
        let mut job = LayoutJob::default();
        job.wrap.max_width = wrap_width;
        let spans = variable_spans(text);
        let mut cursor = 0usize;
        for (start, end) in spans {
            if start > cursor {
                job.append(&text[cursor..start], 0.0, TextFormat { font_id: font.clone(), color: palette.text, ..Default::default() });
            }
            job.append(
                text.get(start..end).unwrap_or_default(),
                0.0,
                TextFormat { font_id: font.clone(), color: palette.accent, background: palette.accent.gamma_multiply(0.12), ..Default::default() },
            );
            cursor = end;
        }
        if cursor < text.len() {
            job.append(&text[cursor..], 0.0, TextFormat { font_id: font.clone(), color: palette.text, ..Default::default() });
        }
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    ui.add(TextEdit::singleline(text).id(id).hint_text(hint).desired_width(f32::INFINITY).layouter(&mut layouter))
}

#[cfg(test)]
mod tests {
    use super::variable_spans;

    #[test]
    fn spans_are_exact_and_ignore_unterminated_braces() {
        let spans = variable_spans("a {{one}} b {{two}} {{unterminated");
        let captured: Vec<&str> = spans.iter().filter_map(|(start, end)| "a {{one}} b {{two}} {{unterminated".get(*start..*end)).collect();
        assert_eq!(captured, vec!["{{one}}", "{{two}}"]);
        assert!(variable_spans("{{}}").len() == 1, "an empty name is still a chip-shaped span");
        assert!(variable_spans("plain text").is_empty());
    }
}
