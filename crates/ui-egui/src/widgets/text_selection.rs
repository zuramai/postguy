//! Selection model for the read-only response viewer: several carets and ranges over one text,
//! addressed by byte offsets that always sit on char boundaries. Everything here is pure logic
//! over `&str` so it can be tested without a UI, and total: no indexing, no panics on stale
//! offsets (the text can change between frames).

use std::ops::Range;

/// One selection: `anchor` is where it started, `head` is the caret end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sel {
    pub anchor: usize,
    pub head: usize,
}

impl Sel {
    pub(crate) fn caret(at: usize) -> Sel {
        Sel { anchor: at, head: at }
    }

    pub(crate) fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// All selections; the last one is the primary (the one Cmd/Ctrl+D extends from).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Selections {
    pub sels: Vec<Sel>,
}

impl Selections {
    pub(crate) fn primary(&self) -> Option<Sel> {
        self.sels.last().copied()
    }

    /// Replace everything with one selection.
    pub(crate) fn set(&mut self, sel: Sel) {
        self.sels.clear();
        self.sels.push(sel);
    }

    fn dedupe(&mut self) {
        let mut seen: Vec<Sel> = Vec::with_capacity(self.sels.len());
        for sel in self.sels.drain(..) {
            if !seen.contains(&sel) {
                seen.push(sel);
            }
        }
        self.sels = seen;
    }
}

/// Where the caret can move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Motion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
}

fn floor_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Pull every offset inside the text and onto a char boundary (the text may have changed since
/// the selections were stored), then drop duplicates.
pub(crate) fn clamp_to_text(text: &str, sels: &mut Selections) {
    for sel in &mut sels.sels {
        sel.anchor = floor_boundary(text, sel.anchor);
        sel.head = floor_boundary(text, sel.head);
    }
    sels.dedupe();
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// The word around `at`; empty (at `at`) when neither neighbour is a word character.
pub(crate) fn word_at(text: &str, at: usize) -> Range<usize> {
    let at = floor_boundary(text, at);
    let before = text.get(..at).unwrap_or_default();
    let after = text.get(at..).unwrap_or_default();
    let back: usize = before.chars().rev().take_while(|ch| is_word(*ch)).map(char::len_utf8).sum();
    let forward: usize = after.chars().take_while(|ch| is_word(*ch)).map(char::len_utf8).sum();
    at.saturating_sub(back)..at.saturating_add(forward).min(text.len())
}

/// The physical line containing `at`, including its trailing newline.
pub(crate) fn line_at(text: &str, at: usize) -> Range<usize> {
    let at = floor_boundary(text, at);
    let start = text.get(..at).and_then(|before| before.rfind('\n')).map_or(0, |index| index + 1);
    let end = text.get(at..).and_then(|after| after.find('\n')).map_or(text.len(), |index| at + index + 1);
    start..end
}

/// End of the line's content (before the newline) for the line containing `at`.
fn line_content_end(text: &str, at: usize) -> usize {
    let line = line_at(text, at);
    if text.get(..line.end).is_some_and(|slice| slice.ends_with('\n')) { line.end.saturating_sub(1) } else { line.end }
}

/// Cmd/Ctrl+D: an empty primary selects its word; otherwise the next occurrence of the selected
/// text (searching on from the primary, then wrapping to the top) joins the selections as the
/// new primary. Occurrences that are already selected are skipped.
pub(crate) fn add_next_occurrence(text: &str, sels: &mut Selections) {
    let Some(primary) = sels.primary() else { return };
    if primary.is_empty() {
        let word = word_at(text, primary.head);
        if word.is_empty() {
            return;
        }
        if let Some(last) = sels.sels.last_mut() {
            *last = Sel { anchor: word.start, head: word.end };
        }
        sels.dedupe();
        return;
    }
    let range = primary.range();
    let Some(needle) = text.get(range.clone()) else { return };
    let taken = |found: &Range<usize>| sels.sels.iter().any(|sel| sel.range() == *found);
    let forward = text
        .get(range.end..)
        .into_iter()
        .flat_map(|rest| rest.match_indices(needle))
        .map(|(index, _)| range.end + index..range.end + index + needle.len())
        .find(|found| !taken(found));
    let wrapped = || {
        text.match_indices(needle)
            .map(|(index, _)| index..index + needle.len())
            .take_while(|found| found.start < range.start)
            .find(|found| !taken(found))
    };
    if let Some(found) = forward.or_else(wrapped) {
        sels.sels.push(Sel { anchor: found.start, head: found.end });
    }
}

/// The text of every non-empty selection in document order, joined by newlines.
pub(crate) fn copy_text(text: &str, sels: &Selections) -> String {
    let mut ranges: Vec<Range<usize>> = sels.sels.iter().filter(|sel| !sel.is_empty()).map(Sel::range).collect();
    ranges.sort_by_key(|range| range.start);
    ranges.iter().filter_map(|range| text.get(range.clone())).collect::<Vec<_>>().join("\n")
}

/// Move `columns` chars from `start`, never past `end`.
fn advance_columns(text: &str, start: usize, end: usize, columns: usize) -> usize {
    let line = text.get(start..end).unwrap_or_default();
    line.char_indices().nth(columns).map_or(end, |(index, _)| start + index)
}

/// The caret offset after `motion` from `at`.
pub(crate) fn move_caret(text: &str, at: usize, motion: Motion) -> usize {
    let at = floor_boundary(text, at);
    match motion {
        Motion::Left => text.get(..at).and_then(|before| before.chars().next_back()).map_or(0, |ch| at - ch.len_utf8()),
        Motion::Right => text.get(at..).and_then(|after| after.chars().next()).map_or(text.len(), |ch| at + ch.len_utf8()),
        Motion::LineStart => line_at(text, at).start,
        Motion::LineEnd => line_content_end(text, at),
        Motion::DocStart => 0,
        Motion::DocEnd => text.len(),
        Motion::Up => {
            let line_start = line_at(text, at).start;
            if line_start == 0 {
                return 0;
            }
            let column = text.get(line_start..at).map_or(0, |before| before.chars().count());
            let previous_end = line_start.saturating_sub(1);
            let previous_start = line_at(text, previous_end).start;
            advance_columns(text, previous_start, previous_end, column)
        }
        Motion::Down => {
            let line_start = line_at(text, at).start;
            let content_end = line_content_end(text, at);
            if content_end >= text.len() {
                return text.len();
            }
            let column = text.get(line_start..at).map_or(0, |before| before.chars().count());
            let next_start = content_end + 1;
            let next_end = line_content_end(text, next_start);
            advance_columns(text, next_start, next_end, column)
        }
    }
}

/// Apply `motion` to every selection. With `extend` only the heads move; without it each
/// selection collapses to a caret (Left/Right on a range collapse to its edge, as in any editor).
pub(crate) fn apply_motion(text: &str, sels: &mut Selections, motion: Motion, extend: bool) {
    for sel in &mut sels.sels {
        if extend {
            sel.head = move_caret(text, sel.head, motion);
        } else {
            let at = match motion {
                Motion::Left if !sel.is_empty() => sel.range().start,
                Motion::Right if !sel.is_empty() => sel.range().end,
                _ => move_caret(text, sel.head, motion),
            };
            *sel = Sel::caret(at);
        }
    }
    sels.dedupe();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, needle: &str) -> usize {
        text.find(needle).expect("needle in text")
    }

    #[test]
    fn next_occurrence_selects_word_then_adds_matches_and_wraps() {
        let text = "a foo b foo c foo";
        let mut sels = Selections::default();
        sels.set(Sel::caret(3));
        add_next_occurrence(text, &mut sels);
        assert_eq!(sels.sels, vec![Sel { anchor: 2, head: 5 }], "the first press selects the word");
        add_next_occurrence(text, &mut sels);
        add_next_occurrence(text, &mut sels);
        let ranges: Vec<_> = sels.sels.iter().map(Sel::range).collect();
        assert_eq!(ranges, vec![2..5, 8..11, 14..17]);
        add_next_occurrence(text, &mut sels);
        assert_eq!(sels.sels.len(), 3, "every occurrence is selected: nothing more to add");

        // Wrapping: start from the last occurrence only.
        let mut sels = Selections::default();
        sels.set(Sel { anchor: 14, head: 17 });
        add_next_occurrence(text, &mut sels);
        assert_eq!(sels.primary().map(|sel| sel.range()), Some(2..5), "the search wraps to the top");
    }

    #[test]
    fn next_occurrence_ignores_non_words_and_empty_selection_lists() {
        let mut sels = Selections::default();
        add_next_occurrence("abc", &mut sels);
        assert!(sels.sels.is_empty());
        sels.set(Sel::caret(1));
        add_next_occurrence("a  b", &mut sels);
        assert_eq!(sels.sels, vec![Sel { anchor: 0, head: 1 }], "the caret right after a word selects it");
        sels.set(Sel::caret(2));
        add_next_occurrence("a  b", &mut sels);
        assert_eq!(sels.sels, vec![Sel::caret(2)], "between spaces there is no word");
    }

    #[test]
    fn copy_joins_selections_in_text_order() {
        let text = "one two three";
        let mut sels = Selections::default();
        sels.sels.push(Sel { anchor: at(text, "three"), head: text.len() });
        sels.sels.push(Sel { anchor: 0, head: 3 });
        sels.sels.push(Sel::caret(5));
        assert_eq!(copy_text(text, &sels), "one\nthree");
    }

    #[test]
    fn offsets_stay_on_char_boundaries_with_multibyte_text() {
        let text = "héllo wörld\nsecond ✓ line";
        let motions = [Motion::Left, Motion::Right, Motion::Up, Motion::Down, Motion::LineStart, Motion::LineEnd, Motion::DocStart, Motion::DocEnd];
        for offset in 0..=text.len() + 2 {
            for motion in motions {
                let moved = move_caret(text, offset, motion);
                assert!(text.is_char_boundary(moved), "{motion:?} from {offset} landed on {moved}");
                assert!(moved <= text.len());
            }
            let word = word_at(text, offset);
            assert!(text.is_char_boundary(word.start) && text.is_char_boundary(word.end));
            let line = line_at(text, offset);
            assert!(text.is_char_boundary(line.start) && text.is_char_boundary(line.end));
        }
    }

    #[test]
    fn vertical_motion_keeps_the_column_and_clamps_to_short_lines() {
        let text = "abcdef\nab\nabcdef";
        assert_eq!(move_caret(text, 5, Motion::Down), 9, "clamped to the end of the short line");
        assert_eq!(move_caret(text, 9, Motion::Down), 12, "column 2 on the last line");
        assert_eq!(move_caret(text, 12, Motion::Up), 9);
        assert_eq!(move_caret(text, 3, Motion::Up), 0, "up on the first line goes to the start");
        assert_eq!(move_caret(text, 14, Motion::Down), text.len(), "down on the last line goes to the end");
        assert_eq!(move_caret(text, 4, Motion::LineEnd), 6, "the end of a line is before its newline");
    }

    #[test]
    fn clamp_pulls_stale_offsets_into_the_text() {
        let mut sels = Selections::default();
        sels.sels.push(Sel { anchor: 1, head: 99 });
        sels.sels.push(Sel { anchor: 99, head: 100 });
        sels.sels.push(Sel { anchor: 2, head: 2 });
        // "é" is two bytes, so offset 2 in "aé" is inside the character.
        clamp_to_text("aé", &mut sels);
        assert_eq!(sels.sels, vec![Sel { anchor: 1, head: 3 }, Sel { anchor: 3, head: 3 }, Sel { anchor: 1, head: 1 }]);
    }

    #[test]
    fn plain_arrows_collapse_ranges_and_shift_extends_them() {
        let text = "hello world";
        let mut sels = Selections::default();
        sels.set(Sel { anchor: 0, head: 5 });
        apply_motion(text, &mut sels, Motion::Right, false);
        assert_eq!(sels.sels, vec![Sel::caret(5)]);
        apply_motion(text, &mut sels, Motion::Right, true);
        assert_eq!(sels.sels, vec![Sel { anchor: 5, head: 6 }]);
    }
}
