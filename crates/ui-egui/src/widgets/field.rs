//! A single-line field with `{{variable}}` chips: the text edit is styled by a layouter that
//! tints variable spans with the accent and gives them a subtle background, so the chips move
//! with the text instead of being a separate overlay. The field is also the base for the URL
//! bar (autocomplete and cURL paste arrive in M3.4).

use egui::text::LayoutJob;
use egui::{FontId, Response, TextEdit, TextFormat, Ui};

use crate::theme::Theme;

/// One variable available to a field: enough for autocomplete and the per-chip hover popover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableInfo {
    pub name: String,
    pub value: String,
    pub secret: bool,
    pub scope: String,
}

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
    variable_field_with(ui, id, text, hint, &[])
}

/// The colours a `{{var}}` chip is painted with: resolved names take the accent tint, dynamic
/// `$names` the patch tint, and anything not in a visible scope the danger tint (so a broken URL
/// reads as broken before Send).
pub fn chip_tint(name: &str, variables: &[VariableInfo], palette: &crate::theme::Palette) -> (egui::Color32, egui::Color32) {
    if variables.iter().any(|variable| variable.name == name) {
        (palette.accent, palette.accent.gamma_multiply(0.15))
    } else if name.starts_with('$') {
        (palette.patch, palette.patch.gamma_multiply(0.15))
    } else {
        (palette.danger, palette.danger.gamma_multiply(0.15))
    }
}

/// The same field with variable metadata: hovering a `{{chip}}` explains its scope and value
/// (secrets masked unless Cmd/Ctrl is held).
pub fn variable_field_with(ui: &mut Ui, id: egui::Id, text: &mut String, hint: &str, variables: &[VariableInfo]) -> Response {
    let theme = Theme::current(ui.ctx());
    let palette = theme.palette;
    let font = FontId::proportional(theme.metrics.ui_font);
    let scoped: Vec<VariableInfo> = variables.to_vec();
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
            let name = text.get(start + 2..end.saturating_sub(2)).unwrap_or_default().trim();
            let (color, background) = chip_tint(name, &scoped, &palette);
            job.append(text.get(start..end).unwrap_or_default(), 0.0, TextFormat { font_id: font.clone(), color, background, ..Default::default() });
            cursor = end;
        }
        if cursor < text.len() {
            job.append(&text[cursor..], 0.0, TextFormat { font_id: font.clone(), color: palette.text, ..Default::default() });
        }
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    let output = TextEdit::singleline(text)
        .id(id)
        .hint_text(hint)
        .margin(egui::Margin::symmetric(10, 6))
        .desired_width(f32::INFINITY)
        .layouter(&mut layouter)
        .show(ui);
    let response = output.response.response.clone();
    if response.hovered()
        && let Some(pointer) = ui.input(|input| input.pointer.hover_pos())
        && output.text_clip_rect.contains(pointer)
    {
        let cursor = output.galley.cursor_from_pos(pointer - output.galley_pos);
        if let Some((name, span)) = variable_at_char(text, cursor.index.0) {
            let info = variables.iter().find(|variable| variable.name == name);
            let reveal = ui.input(|input| input.modifiers.command);
            egui::Tooltip::always_open(ui.ctx().clone(), response.layer_id, response.id, egui::PopupAnchor::Pointer).show(|ui| {
                ui.set_max_width(320.0);
                match info {
                    Some(info) => {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format!("{{{{{}}}}}", info.name)).strong());
                            ui.label(egui::RichText::new(&info.scope).small().color(theme.palette.text_muted));
                        });
                        let value =
                            if info.secret && !reveal { "•••••• (hold Cmd/Ctrl to reveal)".to_string() } else { info.value.clone() };
                        ui.label(egui::RichText::new(value).monospace().color(theme.palette.text));
                        if info.secret {
                            ui.label(egui::RichText::new("Secret · masked in logs and history").small().color(theme.palette.warning));
                        }
                    }
                    None => {
                        ui.label(egui::RichText::new(format!("{{{{{name}}}}}")).color(theme.palette.danger).strong());
                        ui.label(egui::RichText::new("Unresolved in the visible scopes").small().color(theme.palette.text_muted));
                    }
                }
            });
            let _ = span;
        }
    }
    response
}

/// The variable name covering character index `index`, when the cursor sits inside a well-formed
/// `{{name}}` span.
fn variable_at_char(text: &str, index: usize) -> Option<(String, (usize, usize))> {
    let byte =
        text.char_indices().nth(index).map(|(byte, _)| byte).or_else(|| if index >= text.chars().count() { Some(text.len()) } else { None })?;
    variable_spans(text).into_iter().find(|(start, end)| byte >= *start && byte < *end).and_then(|(start, end)| {
        let inner = text.get(start + 2..end).and_then(|inner| inner.strip_suffix("}}"))?;
        Some((inner.trim().to_string(), (start, end)))
    })
}

#[cfg(test)]
mod tests {
    use super::{VariableInfo, chip_tint, variable_at_char, variable_spans};
    use crate::theme::Palette;

    fn info(name: &str) -> VariableInfo {
        VariableInfo { name: name.to_string(), value: "v".into(), secret: false, scope: "Environment".into() }
    }

    #[test]
    fn chips_tint_by_whether_they_resolve() {
        let palette = Palette::LIGHT;
        let variables = vec![info("baseUrl")];
        assert_eq!(chip_tint("baseUrl", &variables, &palette).0, palette.accent, "a resolvable name is the accent chip");
        assert_eq!(chip_tint("$guid", &variables, &palette).0, palette.patch, "a dynamic name is the patch chip");
        assert_eq!(chip_tint("missing", &variables, &palette).0, palette.danger, "an unknown name is the danger chip");
        assert_eq!(chip_tint("missing", &variables, &palette).1, palette.danger.gamma_multiply(0.15), "the chip background matches its tint");
    }

    #[test]
    fn the_hover_lookup_finds_the_variable_under_the_cursor() {
        let text = "{{base}}/users/{{id}}";
        assert_eq!(variable_at_char(text, 2).map(|(name, _)| name), Some("base".to_string()));
        assert_eq!(variable_at_char(text, 17).map(|(name, _)| name), Some("id".to_string()));
        assert_eq!(variable_at_char(text, 8), None, "outside a chip");
        assert_eq!(variable_at_char("plain", 3), None);
    }

    #[test]
    fn spans_are_exact_and_ignore_unterminated_braces() {
        let spans = variable_spans("a {{one}} b {{two}} {{unterminated");
        let captured: Vec<&str> = spans.iter().filter_map(|(start, end)| "a {{one}} b {{two}} {{unterminated".get(*start..*end)).collect();
        assert_eq!(captured, vec!["{{one}}", "{{two}}"]);
        assert!(variable_spans("{{}}").len() == 1, "an empty name is still a chip-shaped span");
        assert!(variable_spans("plain text").is_empty());
    }
}
