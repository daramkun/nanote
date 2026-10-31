//! VS Code-style multi-cursor commands, built on the editor's public selection API
//! (`set_selected_range` to start a selection set, `add_selection` to extend it).
//!
//! The editor offers no way to list its selections or drop a single one, so the commands that
//! build on earlier selections (`Ctrl+D`, `Ctrl+K Ctrl+D`, keyboard column selection) remember
//! what they created in [`Doc::occurrences`] / [`Doc::column`]. The editor's "active" selection
//! is always the *first* one of a set (added cursors are appended behind it), so that memory is
//! only trusted while the active selection still equals the first range remembered.

use std::ops::Range;

use gpui_kit::*;

use super::*;

actions!(
    nanote,
    [
        SelectNextOccurrence,
        SkipOccurrence,
        SelectAllOccurrences,
        AddCursorsToLineEnds,
        ColumnSelectUp,
        ColumnSelectDown,
        ColumnSelectLeft,
        ColumnSelectRight,
        ColumnSelectPageUp,
        ColumnSelectPageDown,
    ]
);

/// Upper bound for "select all occurrences", to keep the editor responsive.
const MAX_SELECTIONS: usize = 10_000;
/// Lines moved by `Ctrl+Shift+Alt+PageUp/PageDown`.
pub const PAGE_LINES: isize = 20;

/// State of a keyboard column selection: a rectangle between two (line, column) corners,
/// columns counted in characters.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnState {
    anchor: (usize, usize),
    active: (usize, usize),
    /// The first (= active) range of the selection set after our last update.
    last: Range<usize>,
}

// ------------------------------------------------------------------ pure helpers

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The word touching `offset` (a cursor right before or after a word counts).
pub fn word_range_at(text: &str, offset: usize) -> Option<Range<usize>> {
    let offset = offset.min(text.len());
    if !text.is_char_boundary(offset) {
        return None;
    }
    let before = text[..offset].chars().next_back();
    let after = text[offset..].chars().next();
    if !after.is_some_and(is_word_char) && !before.is_some_and(is_word_char) {
        return None;
    }
    let mut start = offset;
    for c in text[..offset].chars().rev() {
        if !is_word_char(c) {
            break;
        }
        start -= c.len_utf8();
    }
    let mut end = offset;
    for c in text[offset..].chars() {
        if !is_word_char(c) {
            break;
        }
        end += c.len_utf8();
    }
    (start < end).then_some(start..end)
}

fn is_whole_word(text: &str, range: &Range<usize>) -> bool {
    let before = text[..range.start].chars().next_back();
    let after = text[range.end..].chars().next();
    !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
}

/// Every (non-overlapping) occurrence of `needle`, optionally only as whole words.
pub fn all_occurrences(text: &str, needle: &str, whole_word: bool) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    text.match_indices(needle)
        .map(|(i, m)| i..i + m.len())
        .filter(|r| !whole_word || is_whole_word(text, r))
        .collect()
}

/// The first occurrence at or after `from` that is not `taken`, wrapping around to the start.
pub fn next_occurrence(
    text: &str,
    needle: &str,
    from: usize,
    whole_word: bool,
    taken: &dyn Fn(&Range<usize>) -> bool,
) -> Option<Range<usize>> {
    let all: Vec<Range<usize>> = all_occurrences(text, needle, whole_word)
        .into_iter()
        .filter(|r| !taken(r))
        .collect();
    all.iter()
        .find(|r| r.start >= from)
        .or_else(|| all.first())
        .cloned()
}

/// Cursor offsets for "insert cursor at end of each selected line". A last line that the
/// selection only touches at its very start is left out, like in VS Code.
pub fn line_end_cursors(text: &str, selection: Range<usize>) -> Vec<usize> {
    let first_line_start = text[..selection.start].rfind('\n').map_or(0, |i| i + 1);
    let mut ends = Vec::new();
    let mut line_start = first_line_start;
    loop {
        if line_start != first_line_start && line_start >= selection.end {
            break;
        }
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |i| line_start + i);
        ends.push(line_end);
        if line_end >= text.len() {
            break;
        }
        line_start = line_end + 1;
    }
    ends
}

/// (line, column in chars) of a byte offset.
pub fn line_col_of(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, text[line_start..offset].chars().count())
}

/// The byte ranges of a column (rectangular) selection: one per line between the two corners,
/// each clipped to the length of its line.
pub fn column_ranges(text: &str, a: (usize, usize), b: (usize, usize)) -> Vec<Range<usize>> {
    let mut lines: Vec<(usize, &str)> = Vec::new();
    let mut offset = 0;
    for line in text.split('\n') {
        lines.push((offset, line));
        offset += line.len() + 1;
    }
    let last = lines.len().saturating_sub(1);
    let (first_line, last_line) = (a.0.min(b.0).min(last), a.0.max(b.0).min(last));
    let (col_lo, col_hi) = (a.1.min(b.1), a.1.max(b.1));
    let byte_at = |line: &str, col: usize| {
        line.char_indices()
            .nth(col)
            .map_or(line.len(), |(i, _)| i)
    };
    (first_line..=last_line)
        .map(|i| {
            let (start, line) = lines[i];
            start + byte_at(line, col_lo)..start + byte_at(line, col_hi)
        })
        .collect()
}

// ------------------------------------------------------------------------ commands

impl NanoteApp {
    /// Make `ranges` the editor's selections (the first replaces everything, the rest are added).
    fn apply_selections(&self, idx: usize, ranges: &[Range<usize>], cx: &mut Context<Self>) {
        let Some((first, rest)) = ranges.split_first() else {
            return;
        };
        self.docs[idx].editor.update(cx, |st, cx| {
            st.set_selected_range(first.clone(), cx);
            for r in rest {
                st.add_selection(r.clone(), cx);
            }
        });
    }

    fn selection_context(&self, cx: &App) -> (usize, String, Range<usize>) {
        let idx = self.active;
        let st = self.docs[idx].editor.read(cx);
        (idx, st.value().to_string(), st.selected_range())
    }

    /// `Ctrl+D`: select the word at the cursor; again: add the next occurrence.
    pub fn select_next_occurrence(&mut self, cx: &mut Context<Self>) {
        let (idx, text, sel) = self.selection_context(cx);
        let doc = &mut self.docs[idx];
        if doc.occurrences.first() != Some(&sel) {
            doc.occurrences.clear();
            doc.occurrence_whole_word = false;
        }
        if sel.is_empty() {
            let Some(word) = word_range_at(&text, sel.start) else {
                return;
            };
            doc.occurrences = vec![word.clone()];
            doc.occurrence_whole_word = true;
            self.apply_selections(idx, &[word], cx);
            return;
        }
        if doc.occurrences.is_empty() {
            doc.occurrences.push(sel.clone());
        }
        let chain = doc.occurrences.clone();
        // Continue after the most recently added occurrence, not after the active one.
        let from = chain.last().map_or(sel.end, |r| r.end);
        let Some(next) = next_occurrence(
            &text,
            &text[sel.clone()],
            from,
            doc.occurrence_whole_word,
            &|r| chain.contains(r),
        ) else {
            return;
        };
        doc.occurrences.push(next.clone());
        self.docs[idx]
            .editor
            .update(cx, |st, cx| st.add_selection(next, cx));
    }

    /// `Ctrl+K Ctrl+D`: drop the last added occurrence and move on to the next one.
    pub fn skip_occurrence(&mut self, cx: &mut Context<Self>) {
        let (idx, text, sel) = self.selection_context(cx);
        let doc = &mut self.docs[idx];
        if sel.is_empty() || doc.occurrences.first() != Some(&sel) {
            // Nothing to skip yet: behave like `Ctrl+D`.
            return self.select_next_occurrence(cx);
        }
        let mut chain = doc.occurrences.clone();
        let removed = chain.pop().unwrap_or(sel.clone());
        let Some(next) = next_occurrence(
            &text,
            &text[removed.clone()],
            removed.end,
            doc.occurrence_whole_word,
            &|r| *r == removed || chain.contains(r),
        ) else {
            return;
        };
        chain.push(next);
        doc.occurrences = chain.clone();
        self.apply_selections(idx, &chain, cx);
    }

    /// `Ctrl+Shift+L` / `Ctrl+F2`: select every occurrence of the selection (or current word).
    pub fn select_all_occurrences(&mut self, cx: &mut Context<Self>) {
        let (idx, text, sel) = self.selection_context(cx);
        let (needle, whole_word) = if sel.is_empty() {
            match word_range_at(&text, sel.start) {
                Some(w) => (text[w].to_string(), true),
                None => return,
            }
        } else {
            (text[sel].to_string(), false)
        };
        let mut ranges = all_occurrences(&text, &needle, whole_word);
        ranges.truncate(MAX_SELECTIONS);
        if ranges.is_empty() {
            return;
        }
        let doc = &mut self.docs[idx];
        doc.occurrences = ranges.clone();
        doc.occurrence_whole_word = whole_word;
        self.apply_selections(idx, &ranges, cx);
    }

    /// `Shift+Alt+I`: one cursor at the end of every line of the selection.
    pub fn add_cursors_to_line_ends(&mut self, cx: &mut Context<Self>) {
        let (idx, text, sel) = self.selection_context(cx);
        let cursors: Vec<Range<usize>> = line_end_cursors(&text, sel)
            .into_iter()
            .take(MAX_SELECTIONS)
            .map(|o| o..o)
            .collect();
        self.docs[idx].occurrences.clear();
        self.apply_selections(idx, &cursors, cx);
    }

    /// `Ctrl+Shift+Alt+arrows`: grow a rectangular selection with the keyboard.
    pub fn column_select(&mut self, d_line: isize, d_col: isize, cx: &mut Context<Self>) {
        let idx = self.active;
        let (text, sel, cursor) = {
            let st = self.docs[idx].editor.read(cx);
            (st.value().to_string(), st.selected_range(), st.cursor())
        };
        let doc = &mut self.docs[idx];
        let mut state = match doc.column.take() {
            Some(s) if s.last == sel => s,
            _ => {
                let corner = line_col_of(&text, cursor);
                ColumnState {
                    anchor: corner,
                    active: corner,
                    last: sel,
                }
            }
        };
        let last_line = text.matches('\n').count();
        state.active.0 = state
            .active
            .0
            .saturating_add_signed(d_line)
            .min(last_line);
        state.active.1 = state.active.1.saturating_add_signed(d_col);
        let ranges = column_ranges(&text, state.anchor, state.active);
        if let Some(first) = ranges.first() {
            state.last = first.clone();
        }
        doc.column = Some(state);
        doc.occurrences.clear();
        self.apply_selections(idx, &ranges, cx);
    }
}

pub fn register_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-d", SelectNextOccurrence, None),
        KeyBinding::new("ctrl-k ctrl-d", SkipOccurrence, None),
        KeyBinding::new("ctrl-shift-l", SelectAllOccurrences, None),
        KeyBinding::new("ctrl-f2", SelectAllOccurrences, None),
        KeyBinding::new("shift-alt-i", AddCursorsToLineEnds, None),
        KeyBinding::new("ctrl-shift-alt-up", ColumnSelectUp, None),
        KeyBinding::new("ctrl-shift-alt-down", ColumnSelectDown, None),
        KeyBinding::new("ctrl-shift-alt-left", ColumnSelectLeft, None),
        KeyBinding::new("ctrl-shift-alt-right", ColumnSelectRight, None),
        KeyBinding::new("ctrl-shift-alt-pageup", ColumnSelectPageUp, None),
        KeyBinding::new("ctrl-shift-alt-pagedown", ColumnSelectPageDown, None),
    ]);
}

#[cfg(test)]
mod tests {
    use super::{
        all_occurrences, column_ranges, line_col_of, line_end_cursors, next_occurrence,
        word_range_at,
    };

    #[test]
    fn finds_the_word_around_a_cursor() {
        let t = "foo bar_baz 한글 x";
        assert_eq!(word_range_at(t, 0), Some(0..3)); // right before the word
        assert_eq!(word_range_at(t, 2), Some(0..3)); // inside
        assert_eq!(word_range_at(t, 3), Some(0..3)); // right after
        assert_eq!(word_range_at(t, 8), Some(4..11)); // underscore belongs to the word
        let hangul = t.find("한글").unwrap();
        assert_eq!(word_range_at(t, hangul + 3), Some(hangul..hangul + 6)); // multibyte
        assert_eq!(word_range_at("a  b", 2), None); // between two spaces
        assert_eq!(word_range_at("", 0), None);
    }

    #[test]
    fn occurrences_respect_whole_word() {
        let t = "foo foobar foo";
        assert_eq!(all_occurrences(t, "foo", false), vec![0..3, 4..7, 11..14]);
        assert_eq!(all_occurrences(t, "foo", true), vec![0..3, 11..14]);
        assert!(all_occurrences(t, "", false).is_empty());
    }

    #[test]
    fn next_occurrence_wraps_and_skips_taken() {
        let t = "foo bar foo baz foo";
        let none = |_: &std::ops::Range<usize>| false;
        assert_eq!(next_occurrence(t, "foo", 3, true, &none), Some(8..11));
        assert_eq!(next_occurrence(t, "foo", 12, true, &none), Some(16..19));
        // Past the last one it wraps to the first.
        assert_eq!(next_occurrence(t, "foo", 19, true, &none), Some(0..3));
        // Already-selected ranges are not offered again.
        let taken = |r: &std::ops::Range<usize>| r.start == 8 || r.start == 16;
        assert_eq!(next_occurrence(t, "foo", 3, true, &taken), Some(0..3));
        let all_taken = |_: &std::ops::Range<usize>| true;
        assert_eq!(next_occurrence(t, "foo", 0, true, &all_taken), None);
    }

    #[test]
    fn line_end_cursors_follow_vscode() {
        let t = "ab\ncd\nef\n";
        // Whole text selected: the empty last line is not included.
        assert_eq!(line_end_cursors(t, 0..t.len()), vec![2, 5, 8]);
        // Selection ending at column 0 of a line leaves that line out.
        assert_eq!(line_end_cursors(t, 1..3), vec![2]);
        // Selection reaching into the next line includes it.
        assert_eq!(line_end_cursors(t, 1..4), vec![2, 5]);
        // A caret alone: its own line.
        assert_eq!(line_end_cursors(t, 4..4), vec![5]);
        // No trailing newline: the last line ends at the text end.
        assert_eq!(line_end_cursors("a\nb", 0..3), vec![1, 3]);
    }

    #[test]
    fn column_math() {
        let t = "abcd\nef\nghij";
        assert_eq!(line_col_of(t, 0), (0, 0));
        assert_eq!(line_col_of(t, 7), (1, 2));
        assert_eq!(line_col_of(t, 10), (2, 2));
        // A rectangle two columns wide over three lines; the short line is clipped.
        assert_eq!(column_ranges(t, (0, 1), (2, 3)), vec![1..3, 6..7, 9..11]);
        // Zero width: plain cursors in one column.
        assert_eq!(column_ranges(t, (0, 2), (2, 2)), vec![2..2, 7..7, 10..10]);
        // Corner order does not matter, and lines past the end are clamped.
        assert_eq!(column_ranges(t, (2, 3), (0, 1)), column_ranges(t, (0, 1), (2, 3)));
        assert_eq!(column_ranges(t, (1, 0), (9, 1)).len(), 2);
    }
}
