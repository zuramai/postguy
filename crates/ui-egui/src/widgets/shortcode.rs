//! Colouring for `{{% base64(file('x.jpg')) %}}` body shortcodes: blue when the shortcode
//! parses and every asset it names exists, red otherwise. The parse comes from
//! [`postguy_vars::shortcode::scan`]; existence is checked against a cached [`AssetIndex`].

use std::collections::BTreeSet;
use std::ops::Range;

use egui::Color32;
use egui::text::{ByteIndex, LayoutJob};

use crate::theme::Palette;

/// The asset paths of one collection, for checking `file('…')` references without I/O.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetIndex {
    pub paths: BTreeSet<String>,
    /// False while the list is still being read; path existence is not checked until then.
    pub loaded: bool,
}

/// One shortcode in `text`: its byte range, whether it is valid, and why not when it is not.
pub type ShortcodeMark = (Range<usize>, bool, Option<String>);

/// Every shortcode in `text`. With a loaded `assets` index an unknown path is invalid; with
/// `None` (or still loading) only syntax counts.
pub fn shortcode_marks(text: &str, assets: Option<&AssetIndex>) -> Vec<ShortcodeMark> {
    if !text.contains("{{%") {
        return Vec::new();
    }
    postguy_vars::shortcode::scan(text)
        .into_iter()
        .map(|span| {
            let range = span.start..span.end;
            match span.check {
                Err(reason) => (range, false, Some(reason)),
                Ok(paths) => {
                    let missing = assets.filter(|index| index.loaded).and_then(|index| paths.iter().find(|path| !index.paths.contains(*path)));
                    match missing {
                        Some(path) => (range, false, Some(format!("asset `{path}` not found in this collection"))),
                        None => (range, true, None),
                    }
                }
            }
        })
        .collect()
}

/// Text and chip-background colours for a shortcode mark.
pub fn mark_colors(valid: bool, palette: &Palette) -> (Color32, Color32) {
    let base = if valid { palette.accent } else { palette.danger };
    (base, base.gamma_multiply(0.15))
}

/// The first invalid mark's reason, for the line under a body editor.
pub fn first_problem(marks: &[ShortcodeMark]) -> Option<&str> {
    marks.iter().find(|(_, valid, _)| !valid).and_then(|(_, _, reason)| reason.as_deref())
}

/// Recolour the parts of `job` covered by `marks`, splitting sections at the mark bounds. Only
/// `color` and `background` change, so the language highlighter's other formatting survives.
pub fn overlay(job: &mut LayoutJob, marks: &[ShortcodeMark], palette: &Palette) {
    let ranges: Vec<(Range<usize>, (Color32, Color32))> =
        marks.iter().map(|(range, valid, _)| (range.clone(), mark_colors(*valid, palette))).collect();
    overlay_colors(job, &ranges);
}

/// [`overlay`] for arbitrary `(byte range, (text, background))` colours. `ranges` must be sorted
/// by start and must not overlap.
pub fn overlay_colors(job: &mut LayoutJob, ranges: &[(Range<usize>, (Color32, Color32))]) {
    if ranges.is_empty() {
        return;
    }
    let sections = std::mem::take(&mut job.sections);
    for section in sections {
        let range = section.byte_range.start.0..section.byte_range.end.0;
        let mut cursor = range.start;
        let push = |job: &mut LayoutJob, start: usize, end: usize, colors: Option<(Color32, Color32)>| {
            if end <= start {
                return;
            }
            let mut format = section.format.clone();
            if let Some((color, background)) = colors {
                format.color = color;
                format.background = background;
            }
            job.sections.push(egui::text::LayoutSection {
                leading_space: if start == range.start { section.leading_space } else { 0.0 },
                byte_range: ByteIndex(start)..ByteIndex(end),
                format,
            });
        };
        for (mark, colors) in ranges {
            let start = mark.start.max(cursor);
            let end = mark.end.min(range.end);
            if end <= start {
                continue;
            }
            push(job, cursor, start, None);
            push(job, start, end, Some(*colors));
            cursor = end;
        }
        push(job, cursor, range.end, None);
        if range.is_empty() {
            // An empty section (the empty-text placeholder) must survive.
            job.sections.push(egui::text::LayoutSection {
                leading_space: section.leading_space,
                byte_range: ByteIndex(range.start)..ByteIndex(range.end),
                format: section.format.clone(),
            });
        }
    }
}
