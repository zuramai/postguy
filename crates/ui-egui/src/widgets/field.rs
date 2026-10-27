//! A single-line field with `{{variable}}` chips: the text edit is styled by a layouter that
//! tints variable spans with the accent and gives them a subtle background, so the chips move
//! with the text instead of being a separate overlay. The field is also the base for the URL
//! bar (autocomplete and cURL paste are layered on by the builder).

use std::ops::Range;

use egui::text::LayoutJob;
use egui::text_edit::TextEditOutput;
use egui::{Color32, FontId, Response, TextEdit, TextFormat, Ui};

use crate::theme::{Palette, Theme};
use crate::widgets::shortcode::{AssetIndex, ShortcodeMark, mark_colors, shortcode_marks};

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
        if bytes.get(index) == Some(&b'{') && bytes.get(index + 1) == Some(&b'{') && bytes.get(index + 2) != Some(&b'%') {
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
    variable_field_with(ui, id, text, hint, &[], None)
}

/// The colours a `{{var}}` chip is painted with: a name that resolves in a visible scope, or a
/// known dynamic `$name`, takes the accent (blue); anything else the danger tint (red), so a
/// broken request reads as broken before Send.
pub fn chip_tint(name: &str, variables: &[VariableInfo], palette: &Palette) -> (Color32, Color32) {
    let base = if is_resolvable(name, variables) { palette.accent } else { palette.danger };
    (base, base.gamma_multiply(0.15))
}

/// Whether `{{name}}` resolves: a variable in the visible scopes or a known dynamic `$name`.
fn is_resolvable(name: &str, variables: &[VariableInfo]) -> bool {
    variables.iter().any(|variable| variable.name == name) || (name.starts_with('$') && postguy_vars::dynamic::is_known(name))
}

/// The `{{var}}` spans of `text` with their chip colours, in byte order. Spans inside a
/// `{{% … %}}` shortcode are never produced (`variable_spans` skips those).
pub fn variable_marks(text: &str, variables: &[VariableInfo], palette: &Palette) -> Vec<(Range<usize>, (Color32, Color32))> {
    variable_spans(text)
        .into_iter()
        .map(|(start, end)| {
            let name = text.get(start + 2..end.saturating_sub(2)).unwrap_or_default().trim();
            (start..end, chip_tint(name, variables, palette))
        })
        .collect()
}

/// One single-line layout of `text` in `font`: shortcode `marks` and `{{var}}` chips coloured,
/// everything else in the plain text colour. Never wraps (a long value scrolls inside its field).
pub fn variable_job(text: &str, font: &FontId, palette: &Palette, variables: &[VariableInfo], marks: &[ShortcodeMark]) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    job.wrap.max_rows = 1;
    // Shortcodes first; a `{{var}}` inside one belongs to its colour, not a chip.
    let mut segments: Vec<(usize, usize, (Color32, Color32))> =
        marks.iter().map(|(range, valid, _)| (range.start, range.end, mark_colors(*valid, palette))).collect();
    for (range, colors) in variable_marks(text, variables, palette) {
        if !marks.iter().any(|(mark, _, _)| range.start < mark.end && range.end > mark.start) {
            segments.push((range.start, range.end, colors));
        }
    }
    segments.sort_by_key(|segment| segment.0);
    let mut cursor = 0usize;
    for (start, end, (color, background)) in segments {
        if start < cursor {
            continue;
        }
        if start > cursor {
            job.append(
                text.get(cursor..start).unwrap_or_default(),
                0.0,
                TextFormat { font_id: font.clone(), color: palette.text, ..Default::default() },
            );
        }
        job.append(text.get(start..end).unwrap_or_default(), 0.0, TextFormat { font_id: font.clone(), color, background, ..Default::default() });
        cursor = end;
    }
    if cursor < text.len() {
        job.append(text.get(cursor..).unwrap_or_default(), 0.0, TextFormat { font_id: font.clone(), color: palette.text, ..Default::default() });
    }
    if text.is_empty() {
        // An empty job has zero height, which misplaces the caret and the hint text.
        job.append("", 0.0, TextFormat { font_id: font.clone(), color: palette.text, ..Default::default() });
    }
    job
}

/// The same field with variable metadata: hovering a `{{chip}}` explains its scope and value
/// (secrets masked unless Cmd/Ctrl is held).
pub fn variable_field_with(
    ui: &mut Ui,
    id: egui::Id,
    text: &mut String,
    hint: &str,
    variables: &[VariableInfo],
    assets: Option<&AssetIndex>,
) -> Response {
    variable_field_impl(ui, id, text, hint, variables, assets, true)
}

/// [`variable_field_with`] without its own border or fill, for a field that sits inside a
/// container (the URL bar) which draws them.
pub fn variable_field_frameless(
    ui: &mut Ui,
    id: egui::Id,
    text: &mut String,
    hint: &str,
    variables: &[VariableInfo],
    assets: Option<&AssetIndex>,
) -> Response {
    variable_field_impl(ui, id, text, hint, variables, assets, false)
}

fn variable_field_impl(
    ui: &mut Ui,
    id: egui::Id,
    text: &mut String,
    hint: &str,
    variables: &[VariableInfo],
    assets: Option<&AssetIndex>,
    frame: bool,
) -> Response {
    let theme = Theme::current(ui.ctx());
    let palette = theme.palette;
    let font = FontId::proportional(theme.metrics.ui_font);
    let mut layouter = move |ui: &Ui, buffer: &dyn egui::TextBuffer, _wrap_width: f32| {
        let text = buffer.as_str();
        let job = variable_job(text, &font, &palette, variables, &shortcode_marks(text, assets));
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    let mut edit =
        TextEdit::singleline(text).id(id).hint_text(hint).margin(egui::Margin::symmetric(10, 6)).desired_width(f32::INFINITY).layouter(&mut layouter);
    if !frame {
        // Embedded in a container that draws the border (the URL bar): fill its height.
        edit = edit
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 6)))
            .min_size(egui::vec2(0.0, theme.metrics.url_height))
            .vertical_align(egui::Align::Center);
    }
    let output = edit.show(ui);
    variable_tooltip(ui, &output, text, variables);
    output.response.response
}

/// While the pointer rests on a `{{chip}}` of a just-shown text edit, explain it: its scope and
/// value (secrets masked unless Cmd/Ctrl is held), or that it does not resolve.
pub fn variable_tooltip(ui: &Ui, output: &TextEditOutput, text: &str, variables: &[VariableInfo]) {
    let response = &output.response.response;
    if !response.hovered() {
        return;
    }
    let Some(pointer) = ui.input(|input| input.pointer.hover_pos()) else { return };
    if !output.text_clip_rect.contains(pointer) {
        return;
    }
    let cursor = output.galley.cursor_from_pos(pointer - output.galley_pos);
    let Some((name, _)) = variable_at_char(text, cursor.index.0) else { return };
    let theme = Theme::current(ui.ctx());
    let info = variables.iter().find(|variable| variable.name == name);
    let reveal = ui.input(|input| input.modifiers.command);
    egui::Tooltip::always_open(ui.ctx().clone(), response.layer_id, response.id, egui::PopupAnchor::Pointer).show(|ui| {
        ui.set_max_width(320.0);
        match info {
            Some(info) => {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("{{{{{}}}}}", info.name)).strong().color(theme.palette.accent));
                    ui.label(egui::RichText::new(&info.scope).small().color(theme.palette.text_muted));
                });
                let value = if info.secret && !reveal { "•••••• (hold Cmd/Ctrl to reveal)".to_string() } else { info.value.clone() };
                ui.label(egui::RichText::new(value).monospace().color(theme.palette.text));
                if info.secret {
                    ui.label(egui::RichText::new("Secret · masked in logs and history").small().color(theme.palette.warning));
                }
            }
            None if is_resolvable(&name, variables) => {
                ui.label(egui::RichText::new(format!("{{{{{name}}}}}")).color(theme.palette.accent).strong());
                ui.label(
                    egui::RichText::new("Dynamic variable · a fresh value is generated when the request is sent")
                        .small()
                        .color(theme.palette.text_muted),
                );
            }
            None => {
                ui.label(egui::RichText::new(format!("{{{{{name}}}}}")).color(theme.palette.danger).strong());
                ui.label(egui::RichText::new("Unresolved in the visible scopes").small().color(theme.palette.text_muted));
            }
        }
    });
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
        assert_eq!(chip_tint("baseUrl", &variables, &palette).0, palette.accent, "a resolvable name is blue");
        assert_eq!(chip_tint("$guid", &variables, &palette).0, palette.accent, "a known dynamic name is blue");
        assert_eq!(chip_tint("$notADynamicName", &variables, &palette).0, palette.danger, "an unknown dynamic name is red");
        assert_eq!(chip_tint("missing", &variables, &palette).0, palette.danger, "an unknown name is red");
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

    fn section_color(job: &egui::text::LayoutJob, needle: &str) -> Option<egui::Color32> {
        job.sections.iter().find(|section| job.text.get(section.byte_range.start.0..section.byte_range.end.0) == Some(needle)).map(|s| s.format.color)
    }

    #[test]
    fn header_and_query_values_colour_each_chip_by_resolution() {
        let palette = Palette::LIGHT;
        let variables = vec![info("token")];
        let text = "Bearer {{token}} {{gone}} {{$guid}}";
        let job = super::variable_job(text, &egui::FontId::proportional(14.0), &palette, &variables, &[]);
        assert_eq!(section_color(&job, "{{token}}"), Some(palette.accent));
        assert_eq!(section_color(&job, "{{gone}}"), Some(palette.danger));
        assert_eq!(section_color(&job, "{{$guid}}"), Some(palette.accent));
        assert_eq!(section_color(&job, "Bearer "), Some(palette.text), "text around chips stays plain");
    }

    #[test]
    fn body_overlay_recolours_variables_inside_json_strings() {
        let palette = Palette::LIGHT;
        let variables = vec![info("id")];
        let text = r#"{"a": "{{id}}", "b": "{{nope}}"}"#;
        let mut job = crate::widgets::editor::highlight_job(crate::widgets::CodeLanguage::Json, text, &palette, egui::FontId::monospace(13.0), 400.0);
        crate::widgets::shortcode::overlay_colors(&mut job, &super::variable_marks(text, &variables, &palette));
        assert_eq!(section_color(&job, "{{id}}"), Some(palette.accent));
        assert_eq!(section_color(&job, "{{nope}}"), Some(palette.danger));
        assert_eq!(job.text, text, "recolouring never changes the text");
    }
}
