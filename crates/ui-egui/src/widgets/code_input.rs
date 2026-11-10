//! Typing aids for the editable [`CodeEditor`](super::CodeEditor): auto-indent on Enter,
//! auto-closing pairs, Tab / Shift+Tab indentation and a second Home / End that crosses a soft
//! wrap. The operations are pure functions over a `String` and a **byte** selection and are
//! total: stale or out-of-range offsets are clamped to char boundaries and nothing here indexes
//! or panics (AGENTS.md). The glue at the bottom applies them to this frame's input events
//! before `TextEdit` sees them.

use std::ops::Range;

use egui::text::{CCursor, CCursorRange};
use egui::{Event, Key, Modifiers};

use super::editor::{byte_of_char, char_of_byte};
use super::text_selection::{floor_boundary, line_at, line_content_end};

/// One indentation step; matches Beautify (`serde_json::to_string_pretty`).
const INDENT: &str = "  ";

/// The selection after an edit (byte offsets on char boundaries; `start == end` is a caret) and
/// whether the text changed (a step-over only moves the caret).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Applied {
    pub sel: Range<usize>,
    pub edited: bool,
}

fn normalize(text: &str, sel: Range<usize>) -> Range<usize> {
    let (a, b) = if sel.start <= sel.end { (sel.start, sel.end) } else { (sel.end, sel.start) };
    floor_boundary(text, a)..floor_boundary(text, b)
}

fn closer(open: char) -> Option<char> {
    match open {
        '{' => Some('}'),
        '[' => Some(']'),
        '(' => Some(')'),
        '"' => Some('"'),
        _ => None,
    }
}

fn is_blank(c: char) -> bool {
    matches!(c, ' ' | '\t')
}

fn may_close_before(next: Option<char>) -> bool {
    next.is_none_or(|c| c.is_whitespace() || matches!(c, '}' | ']' | ')' | ',' | ':' | ';'))
}

fn prev_char(text: &str, at: usize) -> Option<char> {
    text.get(..at)?.chars().next_back()
}

fn next_char(text: &str, at: usize) -> Option<char> {
    text.get(at..)?.chars().next()
}

/// Start offsets of the lines a selection touches. A line that begins exactly at a non-empty
/// selection's end is not touched; an empty selection touches one line.
fn touched_line_starts(text: &str, sel: Range<usize>) -> Vec<usize> {
    let mut starts = vec![line_at(text, sel.start).start];
    for (i, byte) in text.as_bytes().iter().enumerate().skip(sel.start).take(sel.end.saturating_sub(sel.start)) {
        if *byte == b'\n' && i + 1 < sel.end {
            starts.push(i + 1);
        }
    }
    starts
}

/// The logical line containing `at`, without its newline.
pub(crate) fn line_content(text: &str, at: usize) -> Range<usize> {
    line_at(text, at).start..line_content_end(text, at)
}

/// Enter: replace the selection with a newline that keeps the line's indent, adds one level
/// after an opener and splits an empty pair onto three lines.
pub(crate) fn newline(text: &mut String, sel: Range<usize>) -> Applied {
    let sel = normalize(text, sel);
    text.replace_range(sel.clone(), "");
    let at = sel.start;
    let line_start = line_at(text, at).start;
    let before = text.get(line_start..at).unwrap_or("");
    let indent: String = before.chars().take_while(|c| is_blank(*c)).collect();
    let prev = before.trim_end_matches(is_blank).chars().next_back();
    let after = text.get(at..line_content_end(text, at)).unwrap_or("");
    let rest = after.trim_start_matches(is_blank);
    let gap = after.len().saturating_sub(rest.len());
    let next = rest.chars().next();

    let opened = prev.and_then(|p| if matches!(p, '{' | '[' | '(') { closer(p) } else { None });
    let (insert, caret) = if let Some(close) = opened {
        let inner = format!("\n{indent}{INDENT}");
        let caret = at + inner.len();
        if next == Some(close) { (format!("{inner}\n{indent}"), caret) } else { (inner, caret) }
    } else {
        let line = format!("\n{indent}");
        let caret = at + line.len();
        (line, caret)
    };
    // Whitespace between the caret and a closer that moves down with it is dropped.
    let end = if opened.is_some() && next == opened { at + gap } else { at };
    text.replace_range(at..end, &insert);
    Applied { sel: caret..caret, edited: true }
}

/// A typed character. `None` means the editor should insert it normally.
pub(crate) fn type_char(text: &mut String, sel: Range<usize>, ch: char) -> Option<Applied> {
    let sel = normalize(text, sel);
    if sel.start != sel.end {
        return None;
    }
    let at = sel.start;
    let prev = prev_char(text, at);
    let next = next_char(text, at);
    let step_over = |at: usize| Applied { sel: at + 1..at + 1, edited: false };
    match ch {
        '}' | ']' | ')' => {
            if next == Some(ch) {
                return Some(step_over(at));
            }
            let line_start = line_at(text, at).start;
            let before = text.get(line_start..at)?;
            if before.is_empty() || !before.chars().all(is_blank) {
                return None;
            }
            let cut = if before.ends_with('\t') {
                1
            } else {
                let spaces = before.len().saturating_sub(before.trim_end_matches(' ').len());
                spaces.min(INDENT.len())
            };
            let from = at.checked_sub(cut)?;
            let mut buf = [0u8; 4];
            text.replace_range(from..at, ch.encode_utf8(&mut buf));
            let caret = from + 1;
            Some(Applied { sel: caret..caret, edited: true })
        }
        '"' => {
            if next == Some('"') && prev != Some('\\') {
                return Some(step_over(at));
            }
            if prev.is_some_and(|p| p.is_alphanumeric() || p == '"' || p == '\\') || !may_close_before(next) {
                return None;
            }
            text.insert_str(at, "\"\"");
            Some(Applied { sel: at + 1..at + 1, edited: true })
        }
        '{' | '[' | '(' => {
            let close = closer(ch)?;
            if !may_close_before(next) {
                return None;
            }
            let mut pair = String::with_capacity(2);
            pair.push(ch);
            pair.push(close);
            text.insert_str(at, &pair);
            Some(Applied { sel: at + 1..at + 1, edited: true })
        }
        _ => None,
    }
}

/// Backspace between an empty pair (`{|}`) removes both characters.
pub(crate) fn backspace_pair(text: &mut String, sel: Range<usize>) -> Option<Applied> {
    let sel = normalize(text, sel);
    if sel.start != sel.end {
        return None;
    }
    let at = sel.start;
    let prev = prev_char(text, at)?;
    if closer(prev)? != next_char(text, at)? {
        return None;
    }
    // Both characters are ASCII pair characters, so `at ± 1` are char boundaries.
    let from = at.checked_sub(1)?;
    text.replace_range(from..at + 1, "");
    Some(Applied { sel: from..from, edited: true })
}

/// Tab: insert one indent step at a caret, or indent every selected line.
pub(crate) fn indent(text: &mut String, sel: Range<usize>) -> Applied {
    let sel = normalize(text, sel);
    let multiline = text.get(sel.clone()).is_some_and(|selected| selected.contains('\n'));
    if !multiline {
        text.replace_range(sel.clone(), INDENT);
        let caret = sel.start + INDENT.len();
        return Applied { sel: caret..caret, edited: true };
    }
    let starts = touched_line_starts(text, sel.clone());
    for start in starts.iter().rev() {
        text.insert_str(*start, INDENT);
    }
    let first = starts.first().copied().unwrap_or(sel.start);
    let start = if sel.start == first { sel.start } else { sel.start + INDENT.len() };
    let end = sel.end + INDENT.len() * starts.len();
    Applied { sel: start..end, edited: true }
}

/// Shift+Tab: remove one indent step (a tab, or up to two spaces) from every touched line.
pub(crate) fn outdent(text: &mut String, sel: Range<usize>) -> Applied {
    let sel = normalize(text, sel);
    let removals: Vec<(usize, usize)> = touched_line_starts(text, sel.clone())
        .into_iter()
        .map(|start| {
            let line = text.get(start..).unwrap_or("");
            let len = if line.starts_with('\t') { 1 } else { line.bytes().take_while(|b| *b == b' ').count().min(INDENT.len()) };
            (start, len)
        })
        .collect();
    let shift =
        |at: usize| removals.iter().filter(|(start, _)| *start < at).fold(at, |pos, (start, len)| pos.saturating_sub((*len).min(at - *start)));
    let selection = shift(sel.start)..shift(sel.end);
    let total: usize = removals.iter().map(|(_, len)| len).sum();
    for (start, len) in removals.iter().rev() {
        text.replace_range(*start..*start + *len, "");
    }
    Applied { sel: selection, edited: total > 0 }
}

/// Modifiers that leave Enter / Tab free for the editor (Cmd/Ctrl+Enter are app shortcuts).
fn plain(modifiers: &Modifiers) -> bool {
    !(modifiers.command || modifiers.ctrl || modifiers.alt)
}

/// Events that change the text or the caret, and so must keep their order relative to the
/// edits applied here.
fn touches_text(event: &Event) -> bool {
    match event {
        Event::Text(text) => text.chars().any(|c| !matches!(c, '\n' | '\r')),
        Event::Paste(_) | Event::Cut | Event::Ime(_) => true,
        Event::PointerButton { pressed, .. } => *pressed,
        Event::Key { key, pressed: true, modifiers, .. } => {
            modifiers.command
                || modifiers.ctrl
                || matches!(
                    key,
                    Key::Backspace
                        | Key::Delete
                        | Key::Enter
                        | Key::Tab
                        | Key::ArrowLeft
                        | Key::ArrowRight
                        | Key::ArrowUp
                        | Key::ArrowDown
                        | Key::Home
                        | Key::End
                        | Key::PageUp
                        | Key::PageDown
                )
        }
        _ => false,
    }
}

/// Apply the typing aids for this frame's events before `TextEdit` sees them; returns whether
/// the text changed (the caller marks the response changed).
pub(crate) fn before_edit(ui: &egui::Ui, id: egui::Id, text: &mut String) -> bool {
    if !ui.memory(|memory| memory.has_focus(id)) {
        return false;
    }
    let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) else { return false };
    let Some(range) = state.cursor.char_range() else { return false };
    let chars = range.as_sorted_char_range();
    let mut sel = byte_of_char(text, chars.start.0)..byte_of_char(text, chars.end.0);
    let mut edited = false;
    let mut owned_any = false;
    let mut blocked = false;
    ui.ctx().input_mut(|input| {
        input.events.retain(|event| {
            if blocked {
                return true;
            }
            let applied = match event {
                Event::Key { key: Key::Enter, pressed: true, modifiers, .. } if plain(modifiers) => Some(newline(text, sel.clone())),
                Event::Key { key: Key::Tab, pressed: true, modifiers, .. } if plain(modifiers) => {
                    Some(if modifiers.shift { outdent(text, sel.clone()) } else { indent(text, sel.clone()) })
                }
                Event::Key { key: Key::Backspace, pressed: true, modifiers, .. } if modifiers.is_none() => backspace_pair(text, sel.clone()),
                Event::Text(typed) => {
                    let mut chars = typed.chars();
                    match (chars.next(), chars.next()) {
                        (Some(ch), None) => type_char(text, sel.clone(), ch),
                        _ => None,
                    }
                }
                _ => None,
            };
            match applied {
                Some(applied) => {
                    edited |= applied.edited;
                    sel = applied.sel;
                    owned_any = true;
                    false
                }
                None => {
                    blocked = touches_text(event);
                    true
                }
            }
        });
    });
    if owned_any {
        let range = CCursorRange::two(CCursor::new(char_of_byte(text, sel.start)), CCursor::new(char_of_byte(text, sel.end)));
        state.cursor.set_char_range(Some(range));
        state.store(ui.ctx(), id);
    }
    edited
}

/// A second End (or Cmd+Right on Mac) that left the caret where it was (the end of a wrapped
/// row) moves it to the end of the logical line; Home / Cmd+Left likewise to its start.
pub(crate) fn after_edit(ui: &egui::Ui, id: egui::Id, text: &str, before: Option<CCursorRange>, output: &mut egui::text_edit::TextEditOutput) {
    let (Some(before), Some(after)) = (before, output.cursor_range) else { return };
    if before.primary.index != after.primary.index {
        return;
    }
    let key = ui.input(|input| {
        input.events.iter().rev().find_map(|event| match event {
            Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
            _ => None,
        })
    });
    let Some((key, modifiers)) = key else { return };
    let to_end = match key {
        Key::End if !modifiers.ctrl => true,
        Key::ArrowRight if modifiers.mac_cmd => true,
        Key::Home if !modifiers.ctrl => false,
        Key::ArrowLeft if modifiers.mac_cmd => false,
        _ => return,
    };
    let line = line_content(text, byte_of_char(text, after.primary.index.0));
    let target = char_of_byte(text, if to_end { line.end } else { line.start });
    if target == after.primary.index.0 {
        return;
    }
    let primary = CCursor::new(target);
    let secondary = if modifiers.shift { after.secondary } else { primary };
    let range = CCursorRange { primary, secondary, h_pos: None };
    output.state.cursor.set_char_range(Some(range));
    output.state.clone().store(ui.ctx(), id);
    output.cursor_range = Some(range);
    ui.ctx().request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `|` marks the caret (one bar) or the selection ends (two bars).
    fn parse(marked: &str) -> (String, Range<usize>) {
        let bars: Vec<usize> = marked.match_indices('|').map(|(i, _)| i).collect();
        let text = marked.replace('|', "");
        let start = bars.first().copied().unwrap_or(0);
        let end = bars.get(1).map_or(start, |second| second - 1);
        (text, start..end)
    }

    fn show(text: &str, sel: &Range<usize>) -> String {
        let mut out = String::new();
        out.push_str(text.get(..sel.start).unwrap_or(""));
        out.push('|');
        if sel.end > sel.start {
            out.push_str(text.get(sel.start..sel.end).unwrap_or(""));
            out.push('|');
        }
        out.push_str(text.get(sel.end..).unwrap_or(""));
        out
    }

    fn enter(marked: &str) -> String {
        let (mut text, sel) = parse(marked);
        let applied = newline(&mut text, sel);
        show(&text, &applied.sel)
    }

    fn typed(marked: &str, ch: char) -> Option<(String, bool)> {
        let (mut text, sel) = parse(marked);
        type_char(&mut text, sel, ch).map(|applied| (show(&text, &applied.sel), applied.edited))
    }

    fn tab(marked: &str, shift: bool) -> (String, bool) {
        let (mut text, sel) = parse(marked);
        let applied = if shift { outdent(&mut text, sel) } else { indent(&mut text, sel) };
        (show(&text, &applied.sel), applied.edited)
    }

    #[test]
    fn enter_keeps_indent_and_opens_blocks() {
        assert_eq!(enter("{|}"), "{\n  |\n}");
        assert_eq!(enter("  \"a\": [|]"), "  \"a\": [\n    |\n  ]");
        assert_eq!(enter("  \"a\": 1,|"), "  \"a\": 1,\n  |");
        assert_eq!(enter("{|\"x\""), "{\n  |\"x\"");
        assert_eq!(enter("{|   }"), "{\n  |\n}");
        assert_eq!(enter("|"), "\n|");
        assert_eq!(enter("{|abc|}"), "{\n  |\n}");
    }

    #[test]
    fn enter_with_a_stale_selection_appends_at_the_end() {
        let mut text = String::from("ab");
        let applied = newline(&mut text, 100..200);
        assert_eq!(text, "ab\n");
        assert_eq!(applied.sel, 3..3);
    }

    #[test]
    fn typing_openers_closes_them_only_before_a_boundary() {
        assert_eq!(typed("|", '{'), Some(("{|}".into(), true)));
        assert_eq!(typed("|x", '{'), None);
        assert_eq!(typed(": |", '"'), Some((": \"|\"".into(), true)));
        assert_eq!(typed("a|", '"'), None);
        assert_eq!(typed("\\|\"", '"'), None);
        assert_eq!(typed("{|abc|}", '['), None);
    }

    #[test]
    fn typing_a_closer_steps_over_or_dedents() {
        assert_eq!(typed("{|}", '}'), Some(("{}|".into(), false)));
        assert_eq!(typed("\"a|\"", '"'), Some(("\"a\"|".into(), false)));
        assert_eq!(typed("    |", '}'), Some(("  }|".into(), true)));
        assert_eq!(typed("\t|", ')'), Some((")|".into(), true)));
        assert_eq!(typed("x |", '}'), None);
    }

    #[test]
    fn multibyte_text_stays_on_char_boundaries() {
        assert_eq!(typed("é|", '{'), Some(("é{|}".into(), true)));
        let mut text = String::from("é");
        // Offset 1 is inside the two-byte char; it is floored to 0.
        let applied = newline(&mut text, 1..1);
        assert_eq!(text, "\né");
        assert_eq!(applied.sel, 1..1);
    }

    #[test]
    fn backspace_removes_an_empty_pair_only() {
        let (mut text, sel) = parse("[|]");
        let applied = backspace_pair(&mut text, sel);
        assert_eq!(text, "");
        assert_eq!(applied.map(|a| a.sel), Some(0..0));
        let (mut text, sel) = parse("[|x]");
        assert_eq!(backspace_pair(&mut text, sel), None);
    }

    #[test]
    fn tab_indents_a_caret_and_every_selected_line() {
        assert_eq!(tab("a|b", false), ("a  |b".into(), true));
        assert_eq!(tab("|a\nb\nc|", false), ("|  a\n  b\n  c|".into(), true));
        assert_eq!(tab("a|b\nc|d", false), ("  a|b\n  c|d".into(), true));
        // A line that starts exactly at the selection end is left alone.
        assert_eq!(tab("|a\nb\n|c", false), ("|  a\n  b\n|c".into(), true));
    }

    #[test]
    fn shift_tab_removes_one_level() {
        assert_eq!(tab("|\tx", true), ("|x".into(), true));
        assert_eq!(tab(" |x", true), ("|x".into(), true));
        assert_eq!(tab("   |x", true), (" |x".into(), true));
        assert_eq!(tab("|x", true), ("|x".into(), false));
        assert_eq!(tab("|  a\n    b|", true), ("|a\n  b|".into(), true));
        // The caret never crosses into the previous line.
        assert_eq!(tab("a\n |b", true), ("a\n|b".into(), true));
    }
}
