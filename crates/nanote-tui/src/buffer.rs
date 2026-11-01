//! The text of one document: lines, cursor, selection, edits and undo.
//!
//! Columns are counted in characters (not bytes, not screen cells); `ui` converts to cells.

/// (row, column in characters)
pub type Pos = (usize, usize);

/// One cursor with its selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sel {
    pub cursor: Pos,
    /// The other end of the selection, if there is one.
    pub anchor: Option<Pos>,
    /// Column to return to when moving up and down across short lines.
    pub want_col: Option<usize>,
}

#[derive(Clone)]
struct Snap {
    lines: Vec<String>,
    cursor: Pos,
    anchor: Option<Pos>,
    extra: Vec<Sel>,
    rev: u64,
}

#[derive(PartialEq, Clone, Copy)]
enum Kind {
    Typing,
    Other,
}

const MAX_UNDO: usize = 200;

pub struct Buffer {
    pub lines: Vec<String>,
    /// The primary cursor: the one the screen follows (the last one added).
    pub cursor: Pos,
    /// The other end of the primary selection, if there is one.
    pub anchor: Option<Pos>,
    want_col: Option<usize>,
    /// More cursors besides the primary one (multi-cursor editing).
    pub extra: Vec<Sel>,
    /// The range selected as a whole word by `add_next_occurrence`, so that repeating it keeps
    /// matching whole words.
    word_sel: Option<(Pos, Pos)>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    last_kind: Option<Kind>,
    rev: u64,
    next_rev: u64,
    saved_rev: u64,
    /// First line changed since the highlighter last looked (`usize::MAX`: none).
    pub changed_from: usize,
}

fn byte_at(line: &str, col: usize) -> usize {
    line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
}

pub fn char_len(line: &str) -> usize {
    line.chars().count()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        Self {
            lines: split_lines(text),
            cursor: (0, 0),
            anchor: None,
            want_col: None,
            extra: Vec::new(),
            word_sel: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_kind: None,
            rev: 0,
            next_rev: 0,
            saved_rev: 0,
            changed_from: 0,
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn is_dirty(&self) -> bool {
        self.rev != self.saved_rev
    }

    pub fn mark_saved(&mut self) {
        self.saved_rev = self.rev;
        self.last_kind = None;
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn line_len(&self, row: usize) -> usize {
        char_len(&self.lines[row])
    }

    // ------------------------------------------------------------------ undo

    /// Called before every edit. Consecutive typing on one line is one undo step.
    fn begin_edit(&mut self, kind: Kind) {
        let coalesce = kind == Kind::Typing && self.last_kind == Some(Kind::Typing);
        if !coalesce {
            self.undo.push(Snap {
                lines: self.lines.clone(),
                cursor: self.cursor,
                anchor: self.anchor,
                extra: self.extra.clone(),
                rev: self.rev,
            });
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_kind = Some(kind);
        self.next_rev += 1;
        self.rev = self.next_rev;
    }

    fn touch(&mut self, row: usize) {
        self.changed_from = self.changed_from.min(row);
    }

    fn restore(&mut self, snap: Snap) {
        self.lines = snap.lines;
        self.cursor = snap.cursor;
        self.extra = snap.extra;
        self.rev = snap.rev;
        self.anchor = snap.anchor;
        self.want_col = None;
        self.last_kind = None;
        self.changed_from = 0;
    }

    pub fn undo(&mut self) -> bool {
        let Some(snap) = self.undo.pop() else {
            return false;
        };
        self.redo.push(Snap {
            lines: self.lines.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
            extra: self.extra.clone(),
            rev: self.rev,
        });
        self.restore(snap);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(snap) = self.redo.pop() else {
            return false;
        };
        self.undo.push(Snap {
            lines: self.lines.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
            extra: self.extra.clone(),
            rev: self.rev,
        });
        self.restore(snap);
        true
    }

    // ------------------------------------------------------------- selection

    /// The selection, ordered, if it is not empty.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        if a == self.cursor {
            return None;
        }
        Some(if a < self.cursor {
            (a, self.cursor)
        } else {
            (self.cursor, a)
        })
    }

    /// The selected text; with several cursors, the selections in order, one per line.
    pub fn selected_text(&self) -> Option<String> {
        if self.extra.is_empty() {
            let (s, e) = self.selection()?;
            return Some(self.text_between(s, e));
        }
        let mut ranges = self.selections();
        ranges.sort();
        if ranges.is_empty() {
            return None;
        }
        let texts: Vec<String> = ranges.into_iter().map(|(s, e)| self.text_between(s, e)).collect();
        Some(texts.join("\n"))
    }

    fn text_between(&self, s: Pos, e: Pos) -> String {
        if s.0 == e.0 {
            let line = &self.lines[s.0];
            return line[byte_at(line, s.1)..byte_at(line, e.1)].to_owned();
        }
        let first = &self.lines[s.0];
        let mut out = first[byte_at(first, s.1)..].to_owned();
        for row in s.0 + 1..e.0 {
            out.push('\n');
            out.push_str(&self.lines[row]);
        }
        out.push('\n');
        let last = &self.lines[e.0];
        out.push_str(&last[..byte_at(last, e.1)]);
        out
    }

    pub fn select_all(&mut self) {
        self.extra.clear();
        self.anchor = Some((0, 0));
        let last = self.lines.len() - 1;
        self.cursor = (last, self.line_len(last));
        self.want_col = None;
    }

    pub fn select_range(&mut self, start: Pos, end: Pos) {
        self.extra.clear();
        self.anchor = Some(start);
        self.cursor = end;
        self.want_col = None;
    }

    /// Remove the selection (the edit itself is the caller's undo step).
    fn remove_selection(&mut self) -> bool {
        let Some((s, e)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let tail = {
            let last = &self.lines[e.0];
            last[byte_at(last, e.1)..].to_owned()
        };
        let head = {
            let first = &self.lines[s.0];
            first[..byte_at(first, s.1)].to_owned()
        };
        self.lines.splice(s.0..=e.0, [head + &tail]);
        self.cursor = s;
        self.anchor = None;
        self.touch(s.0);
        true
    }

    /// Delete the selection as one undo step; returns its text.
    pub fn cut_selection(&mut self) -> Option<String> {
        if !self.extra.is_empty() {
            let text = self.selected_text()?;
            self.edit_each(Kind::Other, |_, s, _| {
                let (a, b) = range_of(s);
                (a != b).then(|| (a, b, String::new()))
            });
            return Some(text);
        }
        let text = self.selected_text()?;
        self.begin_edit(Kind::Other);
        self.remove_selection();
        self.want_col = None;
        Some(text)
    }

    /// Cut the whole current line (with its line break).
    pub fn cut_line(&mut self) -> String {
        if !self.extra.is_empty() {
            return self.cut_lines();
        }
        self.begin_edit(Kind::Other);
        let row = self.cursor.0;
        self.anchor = None;
        self.want_col = None;
        self.touch(row);
        if self.lines.len() == 1 {
            let text = std::mem::take(&mut self.lines[0]);
            self.cursor = (0, 0);
            return text;
        }
        let text = self.lines.remove(row);
        let row = row.min(self.lines.len() - 1);
        self.cursor = (row, self.cursor.1.min(self.line_len(row)));
        text + "\n"
    }

    // ---------------------------------------------------------------- typing

    pub fn insert_char(&mut self, c: char) {
        if !self.extra.is_empty() {
            return self.multi_insert(&c.to_string());
        }
        let had_selection = self.selection().is_some();
        let kind = if had_selection || c == '\n' {
            Kind::Other
        } else {
            Kind::Typing
        };
        self.begin_edit(kind);
        self.remove_selection();
        let (row, col) = self.cursor;
        let line = &mut self.lines[row];
        let at = byte_at(line, col);
        line.insert(at, c);
        self.cursor.1 += 1;
        self.want_col = None;
        self.touch(row);
    }

    /// Insert text (paste); line breaks in it split lines.
    pub fn insert_str(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if text.is_empty() {
            return;
        }
        if !self.extra.is_empty() {
            return self.multi_paste(&text);
        }
        self.begin_edit(Kind::Other);
        self.remove_selection();
        let (row, col) = self.cursor;
        let (head, tail) = {
            let line = &self.lines[row];
            let at = byte_at(line, col);
            (line[..at].to_owned(), line[at..].to_owned())
        };
        let mut parts = text.split('\n');
        let first = parts.next().unwrap_or_default();
        let rest: Vec<&str> = parts.collect();
        if rest.is_empty() {
            self.lines[row] = format!("{head}{first}{tail}");
            self.cursor.1 = col + char_len(first);
        } else {
            let mut new_lines = vec![format!("{head}{first}")];
            let (last, middle) = rest.split_last().unwrap();
            new_lines.extend(middle.iter().map(|s| (*s).to_owned()));
            new_lines.push(format!("{last}{tail}"));
            let count = new_lines.len();
            self.lines.splice(row..=row, new_lines);
            self.cursor = (row + count - 1, char_len(last));
        }
        self.want_col = None;
        self.touch(row);
    }

    /// Enter: break the line and keep the indentation.
    pub fn newline(&mut self) {
        if !self.extra.is_empty() {
            return self.edit_each(Kind::Other, |b, s, _| {
                let (a, e) = range_of(s);
                let indent: String = b.lines[a.0]
                    .chars()
                    .take(a.1)
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                Some((a, e, format!("\n{indent}")))
            });
        }
        self.begin_edit(Kind::Other);
        self.remove_selection();
        let (row, col) = self.cursor;
        let line = &self.lines[row];
        let at = byte_at(line, col);
        let indent: String = line[..at]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let tail = line[at..].to_owned();
        let head = line[..at].to_owned();
        self.lines[row] = head;
        self.lines.insert(row + 1, format!("{indent}{tail}"));
        self.cursor = (row + 1, char_len(&indent));
        self.want_col = None;
        self.touch(row);
    }

    pub fn backspace(&mut self) {
        if !self.extra.is_empty() {
            return self.edit_each(Kind::Other, |b, s, _| {
                let (a, e) = range_of(s);
                if a != e {
                    return Some((a, e, String::new()));
                }
                let (row, col) = s.cursor;
                match (row, col) {
                    (0, 0) => None,
                    (_, 0) => Some(((row - 1, b.line_len(row - 1)), s.cursor, String::new())),
                    _ => Some((((row, col - 1)), s.cursor, String::new())),
                }
            });
        }
        if self.selection().is_some() {
            self.begin_edit(Kind::Other);
            self.remove_selection();
            return;
        }
        let (row, col) = self.cursor;
        if (row, col) == (0, 0) {
            return;
        }
        self.begin_edit(Kind::Other);
        self.want_col = None;
        if col == 0 {
            let line = self.lines.remove(row);
            self.cursor = (row - 1, self.line_len(row - 1));
            self.lines[row - 1].push_str(&line);
            self.touch(row - 1);
        } else {
            let line = &mut self.lines[row];
            let start = byte_at(line, col - 1);
            let end = byte_at(line, col);
            line.replace_range(start..end, "");
            self.cursor.1 -= 1;
            self.touch(row);
        }
    }

    pub fn delete(&mut self) {
        if !self.extra.is_empty() {
            return self.edit_each(Kind::Other, |b, s, _| {
                let (a, e) = range_of(s);
                if a != e {
                    return Some((a, e, String::new()));
                }
                let (row, col) = s.cursor;
                if col < b.line_len(row) {
                    Some((s.cursor, (row, col + 1), String::new()))
                } else if row + 1 < b.lines.len() {
                    Some((s.cursor, (row + 1, 0), String::new()))
                } else {
                    None
                }
            });
        }
        if self.selection().is_some() {
            self.begin_edit(Kind::Other);
            self.remove_selection();
            return;
        }
        let (row, col) = self.cursor;
        let len = self.line_len(row);
        if col == len && row + 1 == self.lines.len() {
            return;
        }
        self.begin_edit(Kind::Other);
        self.want_col = None;
        if col == len {
            let next = self.lines.remove(row + 1);
            self.lines[row].push_str(&next);
        } else {
            let line = &mut self.lines[row];
            let start = byte_at(line, col);
            let end = byte_at(line, col + 1);
            line.replace_range(start..end, "");
        }
        self.touch(row);
    }

    pub fn delete_word_back(&mut self) {
        if !self.extra.is_empty() {
            return self.edit_each(Kind::Other, |b, s, _| {
                let (a, e) = range_of(s);
                if a != e {
                    return Some((a, e, String::new()));
                }
                let from = b.word_left_from(s.cursor);
                (from != s.cursor).then(|| (from, s.cursor, String::new()))
            });
        }
        if self.selection().is_some() || self.cursor.1 == 0 {
            return self.backspace();
        }
        let from = self.word_left_from(self.cursor);
        self.anchor = Some(from);
        self.begin_edit(Kind::Other);
        self.remove_selection();
    }

    pub fn delete_word_forward(&mut self) {
        if !self.extra.is_empty() {
            return self.edit_each(Kind::Other, |b, s, _| {
                let (a, e) = range_of(s);
                if a != e {
                    return Some((a, e, String::new()));
                }
                let to = b.word_right_from(s.cursor);
                (to != s.cursor).then(|| (s.cursor, to, String::new()))
            });
        }
        if self.selection().is_some() || self.cursor.1 == self.line_len(self.cursor.0) {
            return self.delete();
        }
        let to = self.word_right_from(self.cursor);
        self.anchor = Some(to);
        self.begin_edit(Kind::Other);
        self.remove_selection();
    }

    /// Tab: spaces up to the next multiple of `width`; over a selection, indent its lines.
    pub fn tab(&mut self, width: usize) {
        if !self.extra.is_empty() {
            let spans_lines = self.sels().iter().any(|s| {
                let (a, b) = range_of(s);
                a.0 != b.0
            });
            if spans_lines {
                return self.shift_rows(width, true);
            }
            return self.edit_each(Kind::Other, move |_, s, _| {
                let (a, e) = range_of(s);
                Some((a, e, " ".repeat(width - a.1 % width)))
            });
        }
        if let Some((s, e)) = self.selection() {
            if s.0 != e.0 {
                self.indent_lines(s.0, e.0, width);
                return;
            }
        }
        let col = self.cursor.1;
        let n = width - col % width;
        self.insert_str(&" ".repeat(n));
    }

    pub fn back_tab(&mut self, width: usize) {
        if !self.extra.is_empty() {
            return self.shift_rows(width, false);
        }
        let (a, b) = match self.selection() {
            Some((s, e)) => (s.0, e.0),
            None => (self.cursor.0, self.cursor.0),
        };
        self.begin_edit(Kind::Other);
        for row in a..=b {
            let line = &mut self.lines[row];
            let strip: usize = if line.starts_with('\t') {
                1
            } else {
                line.chars().take(width).take_while(|c| *c == ' ').count()
            };
            line.replace_range(..strip, "");
            if row == self.cursor.0 {
                self.cursor.1 = self.cursor.1.saturating_sub(strip);
            }
            if let Some(anchor) = &mut self.anchor {
                if anchor.0 == row {
                    anchor.1 = anchor.1.saturating_sub(strip);
                }
            }
        }
        self.touch(a);
    }

    fn indent_lines(&mut self, a: usize, b: usize, width: usize) {
        self.begin_edit(Kind::Other);
        let pad = " ".repeat(width);
        for row in a..=b {
            if !self.lines[row].is_empty() {
                self.lines[row].insert_str(0, &pad);
            }
            if row == self.cursor.0 && !self.lines[row].is_empty() {
                self.cursor.1 += width;
            }
            if let Some(anchor) = &mut self.anchor {
                if anchor.0 == row && !self.lines[row].is_empty() {
                    anchor.1 += width;
                }
            }
        }
        self.touch(a);
    }

    // -------------------------------------------------------------- movement

    fn set_cursor(&mut self, pos: Pos, extend: bool, keep_want: bool) {
        // Moving ends a run of typing: the next typing is a new undo step.
        self.last_kind = None;
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = pos;
        if !keep_want {
            self.want_col = None;
        }
    }

    pub fn move_to(&mut self, pos: Pos, extend: bool) {
        if !extend {
            self.extra.clear();
        }
        let row = pos.0.min(self.lines.len() - 1);
        let col = pos.1.min(self.line_len(row));
        self.set_cursor((row, col), extend, false);
    }

    fn left1(&mut self, extend: bool) {
        if !extend {
            if let Some((s, _)) = self.selection() {
                return self.set_cursor(s, false, false);
            }
        }
        let (row, col) = self.cursor;
        let pos = if col > 0 {
            (row, col - 1)
        } else if row > 0 {
            (row - 1, self.line_len(row - 1))
        } else {
            (0, 0)
        };
        self.set_cursor(pos, extend, false);
    }

    fn right1(&mut self, extend: bool) {
        if !extend {
            if let Some((_, e)) = self.selection() {
                return self.set_cursor(e, false, false);
            }
        }
        let (row, col) = self.cursor;
        let pos = if col < self.line_len(row) {
            (row, col + 1)
        } else if row + 1 < self.lines.len() {
            (row + 1, 0)
        } else {
            (row, col)
        };
        self.set_cursor(pos, extend, false);
    }

    fn vertical1(&mut self, delta: isize, extend: bool) {
        let (row, col) = self.cursor;
        let want = self.want_col.unwrap_or(col);
        let target = row as isize + delta;
        let pos = if target < 0 {
            (0, 0)
        } else if target as usize >= self.lines.len() {
            let last = self.lines.len() - 1;
            (last, self.line_len(last))
        } else {
            let r = target as usize;
            (r, want.min(self.line_len(r)))
        };
        let at_edge = target < 0 || target as usize >= self.lines.len();
        self.set_cursor(pos, extend, !at_edge);
        if !at_edge {
            self.want_col = Some(want);
        }
    }

    /// Home: the first non-blank character, or the start of the line when already there.
    fn home1(&mut self, extend: bool) {
        let (row, col) = self.cursor;
        let indent = self.lines[row]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .count();
        let target = if col == indent || indent == self.line_len(row) {
            0
        } else {
            indent
        };
        self.set_cursor((row, target), extend, false);
    }

    fn end1(&mut self, extend: bool) {
        let row = self.cursor.0;
        self.set_cursor((row, self.line_len(row)), extend, false);
    }

    fn doc_start1(&mut self, extend: bool) {
        self.set_cursor((0, 0), extend, false);
    }

    fn doc_end1(&mut self, extend: bool) {
        let last = self.lines.len() - 1;
        self.set_cursor((last, self.line_len(last)), extend, false);
    }

    fn word_left_from(&self, from: Pos) -> Pos {
        let (row, mut col) = from;
        if col == 0 {
            return if row == 0 {
                (0, 0)
            } else {
                (row - 1, self.line_len(row - 1))
            };
        }
        let chars: Vec<char> = self.lines[row].chars().collect();
        while col > 0 && chars[col - 1].is_whitespace() {
            col -= 1;
        }
        if col > 0 {
            let class = is_word(chars[col - 1]);
            while col > 0 && !chars[col - 1].is_whitespace() && is_word(chars[col - 1]) == class {
                col -= 1;
            }
        }
        (row, col)
    }

    fn word_right_from(&self, from: Pos) -> Pos {
        let (row, mut col) = from;
        let chars: Vec<char> = self.lines[row].chars().collect();
        if col >= chars.len() {
            return if row + 1 < self.lines.len() {
                (row + 1, 0)
            } else {
                (row, chars.len())
            };
        }
        if !chars[col].is_whitespace() {
            let class = is_word(chars[col]);
            while col < chars.len() && !chars[col].is_whitespace() && is_word(chars[col]) == class {
                col += 1;
            }
        }
        while col < chars.len() && chars[col].is_whitespace() {
            col += 1;
        }
        (row, col)
    }

    fn word_left1(&mut self, extend: bool) {
        let pos = self.word_left_from(self.cursor);
        self.set_cursor(pos, extend, false);
    }

    fn word_right1(&mut self, extend: bool) {
        let pos = self.word_right_from(self.cursor);
        self.set_cursor(pos, extend, false);
    }

    pub fn goto_line(&mut self, line: usize, col: usize) {
        self.extra.clear();
        let row = line.saturating_sub(1).min(self.lines.len() - 1);
        let col = col.saturating_sub(1).min(self.line_len(row));
        self.set_cursor((row, col), false, false);
    }

    // ------------------------------------------------------- multiple cursors

    /// Every cursor, the primary one first.
    fn sels(&self) -> Vec<Sel> {
        let mut all = vec![Sel {
            cursor: self.cursor,
            anchor: self.anchor,
            want_col: self.want_col,
        }];
        all.extend(self.extra.iter().copied());
        all
    }

    /// Make `sels` the cursors (the first is primary). Cursors that touch or overlap become one.
    fn put_sels(&mut self, sels: Vec<Sel>) {
        let mut items: Vec<(Sel, bool)> = sels
            .into_iter()
            .enumerate()
            .map(|(i, s)| (s, i == 0))
            .collect();
        items.sort_by_key(|(s, _)| range_of(s));
        let mut merged: Vec<(Sel, bool)> = Vec::new();
        for item in items {
            if let Some(last) = merged.last_mut() {
                if overlaps(&last.0, &item.0) {
                    *last = merge(*last, item);
                    continue;
                }
            }
            merged.push(item);
        }
        let primary = merged.iter().position(|m| m.1).unwrap_or(0);
        let (p, _) = merged.remove(primary);
        self.cursor = p.cursor;
        self.anchor = p.anchor;
        self.want_col = p.want_col;
        self.extra = merged.into_iter().map(|m| m.0).collect();
    }

    pub fn cursor_count(&self) -> usize {
        1 + self.extra.len()
    }

    /// The non-empty selections of all cursors, each ordered.
    pub fn selections(&self) -> Vec<(Pos, Pos)> {
        self.sels()
            .iter()
            .map(range_of)
            .filter(|(a, b)| a != b)
            .collect()
    }

    /// Where the extra cursors without a selection are (the primary one is the terminal's).
    pub fn extra_carets(&self) -> Vec<Pos> {
        self.extra
            .iter()
            .filter(|s| range_of(s).0 == range_of(s).1)
            .map(|s| s.cursor)
            .collect()
    }

    /// Replace `s..e` by `text`; returns where the inserted text ends.
    fn replace_range(&mut self, s: Pos, e: Pos, text: &str) -> Pos {
        let head = {
            let line = &self.lines[s.0];
            line[..byte_at(line, s.1)].to_owned()
        };
        let tail = {
            let line = &self.lines[e.0];
            line[byte_at(line, e.1)..].to_owned()
        };
        let parts: Vec<&str> = text.split('\n').collect();
        let mut new_lines = Vec::with_capacity(parts.len());
        let end = if parts.len() == 1 {
            new_lines.push(format!("{head}{}{tail}", parts[0]));
            (s.0, s.1 + char_len(parts[0]))
        } else {
            new_lines.push(format!("{head}{}", parts[0]));
            new_lines.extend(parts[1..parts.len() - 1].iter().map(|p| (*p).to_owned()));
            let last = parts[parts.len() - 1];
            new_lines.push(format!("{last}{tail}"));
            (s.0 + parts.len() - 1, char_len(last))
        };
        self.lines.splice(s.0..=e.0, new_lines);
        self.touch(s.0);
        end
    }

    /// Edit at every cursor, from the first to the last in the text. `f` says what to replace
    /// at a cursor as `(from, to, text)` (it sees the text as the earlier edits left it, and
    /// the cursor's rank in the text); `None` leaves that cursor alone.
    fn edit_each(
        &mut self,
        kind: Kind,
        f: impl Fn(&Buffer, &Sel, usize) -> Option<(Pos, Pos, String)>,
    ) {
        let mut sels = self.sels();
        let mut order: Vec<usize> = (0..sels.len()).collect();
        order.sort_by_key(|&i| range_of(&sels[i]));
        self.begin_edit(kind);
        for (rank, &i) in order.iter().enumerate() {
            let sel = sels[i];
            let Some((s, e, text)) = f(self, &sel, rank) else {
                continue;
            };
            let end = self.replace_range(s, e, &text);
            for (j, other) in sels.iter_mut().enumerate() {
                if j == i {
                    continue;
                }
                other.cursor = map_pos(other.cursor, e, end);
                if let Some(a) = other.anchor.as_mut() {
                    *a = map_pos(*a, e, end);
                }
            }
            sels[i] = Sel {
                cursor: end,
                anchor: None,
                want_col: None,
            };
        }
        self.put_sels(sels);
    }

    fn multi_insert(&mut self, text: &str) {
        let one_char = text.chars().count() == 1 && text != "\n";
        let typing = one_char && self.selections().is_empty();
        let text = text.to_owned();
        let kind = if typing { Kind::Typing } else { Kind::Other };
        self.edit_each(kind, move |_, s, _| {
            let (a, b) = range_of(s);
            Some((a, b, text.clone()))
        });
    }

    /// Paste: as many lines as cursors means one line each, otherwise the text goes to all.
    fn multi_paste(&mut self, text: &str) {
        let lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        let each = lines.len() == self.cursor_count();
        let whole = text.to_owned();
        self.edit_each(Kind::Other, move |_, s, rank| {
            let (a, b) = range_of(s);
            let piece = if each { lines[rank].clone() } else { whole.clone() };
            Some((a, b, piece))
        });
    }

    /// Cut the line of every cursor.
    fn cut_lines(&mut self) -> String {
        let sels = self.sels();
        let rows: std::collections::BTreeSet<usize> = sels.iter().map(|s| s.cursor.0).collect();
        self.begin_edit(Kind::Other);
        let mut text = String::new();
        for &row in &rows {
            text.push_str(&self.lines[row]);
            text.push('\n');
        }
        let first = *rows.iter().next().unwrap_or(&0);
        for &row in rows.iter().rev() {
            self.lines.remove(row);
        }
        if self.lines.is_empty() {
            self.lines.push(String::new());
            text.pop();
        }
        let moved: Vec<Sel> = sels
            .iter()
            .map(|s| {
                let removed_before = rows.range(..s.cursor.0).count();
                let row = (s.cursor.0 - removed_before).min(self.lines.len() - 1);
                Sel {
                    cursor: (row, s.cursor.1.min(char_len(&self.lines[row]))),
                    anchor: None,
                    want_col: None,
                }
            })
            .collect();
        self.touch(first);
        self.put_sels(moved);
        text
    }

    /// Indent (or unindent) every line that a cursor or selection touches.
    fn shift_rows(&mut self, width: usize, indent: bool) {
        let mut sels = self.sels();
        let mut rows = std::collections::BTreeSet::new();
        for s in &sels {
            let (a, b) = range_of(s);
            rows.extend(a.0..=b.0);
        }
        self.begin_edit(Kind::Other);
        for &row in &rows {
            let delta: isize = if indent {
                if self.lines[row].is_empty() {
                    continue;
                }
                self.lines[row].insert_str(0, &" ".repeat(width));
                width as isize
            } else {
                let line = &mut self.lines[row];
                let strip = if line.starts_with('\t') {
                    1
                } else {
                    line.chars().take(width).take_while(|c| *c == ' ').count()
                };
                line.replace_range(..strip, "");
                -(strip as isize)
            };
            let shift = |p: &mut Pos| {
                if p.0 == row {
                    p.1 = (p.1 as isize + delta).max(0) as usize;
                }
            };
            for s in &mut sels {
                shift(&mut s.cursor);
                if let Some(a) = s.anchor.as_mut() {
                    shift(a);
                }
            }
        }
        self.touch(rows.iter().next().copied().unwrap_or(0));
        self.put_sels(sels);
    }

    /// Run a movement for every cursor.
    fn each(&mut self, f: impl Fn(&mut Buffer)) {
        let mut all = vec![Sel {
            cursor: self.cursor,
            anchor: self.anchor,
            want_col: self.want_col,
        }];
        all.extend(std::mem::take(&mut self.extra));
        for s in all.iter_mut() {
            self.cursor = s.cursor;
            self.anchor = s.anchor;
            self.want_col = s.want_col;
            f(self);
            *s = Sel {
                cursor: self.cursor,
                anchor: self.anchor,
                want_col: self.want_col,
            };
        }
        self.put_sels(all);
    }

    pub fn left(&mut self, extend: bool) {
        self.each_or(|b| b.left1(extend));
    }
    pub fn right(&mut self, extend: bool) {
        self.each_or(|b| b.right1(extend));
    }
    pub fn vertical(&mut self, delta: isize, extend: bool) {
        self.each_or(|b| b.vertical1(delta, extend));
    }
    pub fn home(&mut self, extend: bool) {
        self.each_or(|b| b.home1(extend));
    }
    pub fn end(&mut self, extend: bool) {
        self.each_or(|b| b.end1(extend));
    }
    pub fn doc_start(&mut self, extend: bool) {
        self.each_or(|b| b.doc_start1(extend));
    }
    pub fn doc_end(&mut self, extend: bool) {
        self.each_or(|b| b.doc_end1(extend));
    }
    pub fn word_left(&mut self, extend: bool) {
        self.each_or(|b| b.word_left1(extend));
    }
    pub fn word_right(&mut self, extend: bool) {
        self.each_or(|b| b.word_right1(extend));
    }

    fn each_or(&mut self, f: impl Fn(&mut Buffer)) {
        if self.extra.is_empty() {
            f(self);
        } else {
            self.each(f);
        }
    }

    /// Alt+click: another cursor, which becomes the primary one.
    pub fn add_cursor(&mut self, pos: Pos) {
        let row = pos.0.min(self.lines.len() - 1);
        let col = pos.1.min(self.line_len(row));
        self.push_primary(Sel {
            cursor: (row, col),
            anchor: None,
            want_col: None,
        });
    }

    fn push_primary(&mut self, sel: Sel) {
        let mut sels = self.sels();
        sels.insert(0, sel);
        self.put_sels(sels);
        self.last_kind = None;
    }

    /// A cursor on the line above (`-1`) the first cursor, or below (`1`) the last one.
    pub fn add_cursor_vertical(&mut self, delta: isize) -> bool {
        let sels = self.sels();
        let base = if delta < 0 {
            sels.iter().min_by_key(|s| s.cursor)
        } else {
            sels.iter().max_by_key(|s| s.cursor)
        };
        let Some(base) = base else {
            return false;
        };
        let row = base.cursor.0 as isize + delta;
        if row < 0 || row as usize >= self.lines.len() {
            return false;
        }
        let row = row as usize;
        let want = base.want_col.unwrap_or(base.cursor.1);
        self.push_primary(Sel {
            cursor: (row, want.min(self.line_len(row))),
            anchor: None,
            want_col: Some(want),
        });
        true
    }

    /// The word around (or ending at) a position.
    fn word_at(&self, (row, col): Pos) -> Option<(Pos, Pos)> {
        let chars: Vec<char> = self.lines[row].chars().collect();
        let on = |i: usize| chars.get(i).is_some_and(|c| is_word(*c));
        let probe = if on(col) {
            col
        } else if col > 0 && on(col - 1) {
            col - 1
        } else {
            return None;
        };
        let mut start = probe;
        while start > 0 && on(start - 1) {
            start -= 1;
        }
        let mut end = probe;
        while on(end) {
            end += 1;
        }
        Some(((row, start), (row, end)))
    }

    fn is_whole_word(&self, (s, e): (Pos, Pos)) -> bool {
        let chars: Vec<char> = self.lines[s.0].chars().collect();
        let before = s.1 > 0 && is_word(chars[s.1 - 1]);
        let after = chars.get(e.1).is_some_and(|c| is_word(*c));
        !before && !after
    }

    /// The text the occurrence commands look for, and whether it must be a whole word: the
    /// selection, or the word at the cursor when nothing is selected.
    fn occurrence_target(&self) -> Option<(Vec<char>, bool)> {
        match self.selection() {
            Some((s, e)) if s.0 == e.0 => {
                let text: Vec<char> = self.text_between(s, e).chars().collect();
                Some((text, self.word_sel == Some((s, e))))
            }
            Some(_) => None,
            None => {
                let (s, e) = self.word_at(self.cursor)?;
                Some((self.text_between(s, e).chars().collect(), true))
            }
        }
    }

    /// Ctrl+D: select the word at the cursor; again: add the next occurrence as a cursor.
    pub fn add_next_occurrence(&mut self) -> bool {
        if self.selection().is_none() {
            let Some((s, e)) = self.word_at(self.cursor) else {
                return false;
            };
            self.select_in_place(s, e);
            self.word_sel = Some((s, e));
            return true;
        }
        let Some((needle, whole)) = self.occurrence_target() else {
            return false;
        };
        let (_, from) = self.selection().unwrap();
        let taken: Vec<(Pos, Pos)> = self.selections();
        let rows = self.lines.len();
        for step in 0..=rows {
            let row = (from.0 + step) % rows;
            for i in self.line_matches(row, &needle, true) {
                let range = ((row, i), (row, i + needle.len()));
                let in_order = match step {
                    0 => i >= from.1,
                    n if n == rows => i < from.1,
                    _ => true,
                };
                if !in_order || taken.contains(&range) || (whole && !self.is_whole_word(range)) {
                    continue;
                }
                self.push_primary(Sel {
                    cursor: range.1,
                    anchor: Some(range.0),
                    want_col: None,
                });
                self.word_sel = whole.then_some(range);
                return true;
            }
        }
        false
    }

    fn select_in_place(&mut self, s: Pos, e: Pos) {
        self.anchor = Some(s);
        self.cursor = e;
        self.want_col = None;
        self.last_kind = None;
    }

    /// Ctrl+L: a cursor on every occurrence of the selection (or of the word at the cursor).
    /// Returns how many there are.
    pub fn select_all_occurrences(&mut self) -> usize {
        let Some((needle, whole)) = self.occurrence_target() else {
            return 0;
        };
        let current = self.selection().or_else(|| self.word_at(self.cursor));
        let mut found: Vec<(Pos, Pos)> = Vec::new();
        for row in 0..self.lines.len() {
            for i in self.line_matches(row, &needle, true) {
                let range = ((row, i), (row, i + needle.len()));
                if !whole || self.is_whole_word(range) {
                    found.push(range);
                }
            }
        }
        if found.is_empty() {
            return 0;
        }
        let primary = found
            .iter()
            .position(|r| Some(*r) == current)
            .unwrap_or(found.len() - 1);
        let to_sel = |r: &(Pos, Pos)| Sel {
            cursor: r.1,
            anchor: Some(r.0),
            want_col: None,
        };
        let mut sels = vec![to_sel(&found[primary])];
        sels.extend(
            found
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != primary)
                .map(|(_, r)| to_sel(r)),
        );
        let count = sels.len();
        self.put_sels(sels);
        self.word_sel = whole.then_some(found[primary]);
        self.last_kind = None;
        count
    }

    /// Alt+Shift+I: with a selection over several lines, one cursor at the end of each line.
    pub fn cursors_at_line_ends(&mut self) -> bool {
        let Some((s, e)) = self.selection() else {
            return false;
        };
        if s.0 == e.0 {
            return false;
        }
        let ends: Vec<Sel> = (s.0..=e.0)
            .rev()
            .map(|row| Sel {
                cursor: (row, self.line_len(row)),
                anchor: None,
                want_col: None,
            })
            .collect();
        // the last line's cursor is the primary one
        self.put_sels(ends);
        self.last_kind = None;
        true
    }

    /// Esc: back to one cursor.
    pub fn clear_extra_cursors(&mut self) -> bool {
        let had = !self.extra.is_empty();
        self.extra.clear();
        had
    }

    // ---------------------------------------------------------------- search

    /// Start columns of the matches of `needle` in a row.
    fn line_matches(&self, row: usize, needle: &[char], sensitive: bool) -> Vec<usize> {
        let hay: Vec<char> = self.lines[row].chars().collect();
        if hay.len() < needle.len() {
            return Vec::new();
        }
        (0..=hay.len() - needle.len())
            .filter(|&i| {
                hay[i..i + needle.len()]
                    .iter()
                    .zip(needle)
                    .all(|(h, n)| (if sensitive { *h } else { fold(*h) }) == *n)
            })
            .collect()
    }

    /// The next match of `query` after (or, backwards, before) the cursor or selection,
    /// wrapping around. Lower-case queries ignore case.
    pub fn find(&self, query: &str, forward: bool) -> Option<(Pos, Pos)> {
        if query.is_empty() {
            return None;
        }
        let sensitive = query.chars().any(char::is_uppercase);
        let needle: Vec<char> = query
            .chars()
            .map(|c| if sensitive { c } else { fold(c) })
            .collect();
        let len = needle.len();
        let rows = self.lines.len();
        // From the end of a found match (or its start, backwards), so that repeating moves on.
        let origin = match (self.selection(), forward) {
            (Some((s, _)), false) => s,
            (Some((_, e)), true) => e,
            _ => self.cursor,
        };
        for step in 0..=rows {
            if forward {
                let row = (origin.0 + step) % rows;
                let hit = self
                    .line_matches(row, &needle, sensitive)
                    .into_iter()
                    .find(|&i| match step {
                        0 => i >= origin.1,
                        n if n == rows => i < origin.1,
                        _ => true,
                    });
                if let Some(i) = hit {
                    return Some(((row, i), (row, i + len)));
                }
            } else {
                let row = (origin.0 + rows - step % rows) % rows;
                let hit = self
                    .line_matches(row, &needle, sensitive)
                    .into_iter()
                    .rev()
                    .find(|&i| match step {
                        0 => i + len <= origin.1,
                        n if n == rows => i + len > origin.1,
                        _ => true,
                    });
                if let Some(i) = hit {
                    return Some(((row, i), (row, i + len)));
                }
            }
        }
        None
    }

    /// Replace every match of `query`; returns how many.
    pub fn replace_all(&mut self, query: &str, with: &str) -> usize {
        if query.is_empty() {
            return 0;
        }
        let sensitive = query.chars().any(char::is_uppercase);
        let needle: Vec<char> = query
            .chars()
            .map(|c| if sensitive { c } else { fold(c) })
            .collect();
        let mut count = 0;
        let mut out: Vec<String> = Vec::with_capacity(self.lines.len());
        let mut first_changed = usize::MAX;
        for (row, line) in self.lines.iter().enumerate() {
            let hay: Vec<char> = line.chars().collect();
            let mut s = String::new();
            let mut i = 0;
            let mut changed = false;
            while i < hay.len() {
                let hit = hay.len() >= i + needle.len()
                    && hay[i..i + needle.len()]
                        .iter()
                        .zip(&needle)
                        .all(|(h, n)| (if sensitive { *h } else { fold(*h) }) == *n);
                if hit {
                    s.push_str(with);
                    i += needle.len();
                    count += 1;
                    changed = true;
                } else {
                    s.push(hay[i]);
                    i += 1;
                }
            }
            if changed {
                first_changed = first_changed.min(row);
            }
            out.push(s);
        }
        if count > 0 {
            self.begin_edit(Kind::Other);
            self.lines = out;
            self.extra.clear();
            self.anchor = None;
            self.cursor.0 = self.cursor.0.min(self.lines.len() - 1);
            self.cursor.1 = self.cursor.1.min(self.line_len(self.cursor.0));
            self.touch(first_changed);
        }
        count
    }
}

/// The ordered range a cursor covers (empty for a bare cursor).
fn range_of(s: &Sel) -> (Pos, Pos) {
    match s.anchor {
        Some(a) if a != s.cursor => (a.min(s.cursor), a.max(s.cursor)),
        _ => (s.cursor, s.cursor),
    }
}

/// Whether two cursors (sorted by position) cover the same place; touching selections do not.
fn overlaps(a: &Sel, b: &Sel) -> bool {
    let (a_start, a_end) = range_of(a);
    let (b_start, b_end) = range_of(b);
    if a_start != a_end && b_start != b_end {
        b_start < a_end
    } else {
        b_start <= a_end
    }
}

fn merge(a: (Sel, bool), b: (Sel, bool)) -> (Sel, bool) {
    let (a_start, a_end) = range_of(&a.0);
    let (b_start, b_end) = range_of(&b.0);
    let (start, end) = (a_start.min(b_start), a_end.max(b_end));
    let keep = if b.1 { &b.0 } else { &a.0 };
    let forward = keep.anchor.is_none_or(|an| an <= keep.cursor);
    let (cursor, anchor) = if start == end {
        (start, None)
    } else if forward {
        (end, Some(start))
    } else {
        (start, Some(end))
    };
    (
        Sel {
            cursor,
            anchor,
            want_col: None,
        },
        a.1 || b.1,
    )
}

/// Where a position goes when the text before `old_end` is replaced by text ending at `new_end`.
fn map_pos(p: Pos, old_end: Pos, new_end: Pos) -> Pos {
    if p < old_end {
        p
    } else if p.0 == old_end.0 {
        (new_end.0, new_end.1 + p.1 - old_end.1)
    } else {
        ((p.0 as isize + new_end.0 as isize - old_end.0 as isize) as usize, p.1)
    }
}

/// Lines of a text; the text after the last line break is a line of its own (possibly empty).
pub fn split_lines(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> Buffer {
        Buffer::new(text)
    }

    #[test]
    fn typing_is_one_undo_step_and_undo_restores_cleanliness() {
        let mut b = buf("");
        for c in "hello".chars() {
            b.insert_char(c);
        }
        assert_eq!(b.text(), "hello");
        assert!(b.is_dirty());
        assert!(b.undo());
        assert_eq!(b.text(), "");
        assert!(!b.is_dirty());
        assert!(b.redo());
        assert_eq!(b.text(), "hello");
    }

    #[test]
    fn enter_keeps_indentation() {
        let mut b = buf("    foo");
        b.doc_end(false);
        b.newline();
        b.insert_char('x');
        assert_eq!(b.lines, vec!["    foo", "    x"]);
    }

    #[test]
    fn backspace_and_delete_join_lines() {
        let mut b = buf("ab\ncd");
        b.move_to((1, 0), false);
        b.backspace();
        assert_eq!(b.text(), "abcd");
        assert_eq!(b.cursor, (0, 2));
        b.delete();
        assert_eq!(b.text(), "abd");
    }

    #[test]
    fn multi_byte_characters_are_columns() {
        let mut b = buf("가나다");
        b.move_to((0, 1), false);
        b.insert_char('x');
        assert_eq!(b.text(), "가x나다");
        b.backspace();
        b.backspace();
        assert_eq!(b.text(), "나다");
    }

    #[test]
    fn selection_cut_and_paste_across_lines() {
        let mut b = buf("one\ntwo\nthree");
        b.select_range((0, 2), (2, 2));
        assert_eq!(b.selected_text().unwrap(), "e\ntwo\nth");
        assert_eq!(b.cut_selection().unwrap(), "e\ntwo\nth");
        assert_eq!(b.text(), "onree");
        b.insert_str("1\n2");
        assert_eq!(b.text(), "on1\n2ree");
        assert_eq!(b.cursor, (1, 1));
        assert!(b.undo());
        assert_eq!(b.text(), "onree");
        assert!(b.undo());
        assert_eq!(b.text(), "one\ntwo\nthree");
    }

    #[test]
    fn vertical_movement_remembers_the_column() {
        let mut b = buf("abcdef\nab\nabcdef");
        b.move_to((0, 5), false);
        b.vertical(1, false);
        assert_eq!(b.cursor, (1, 2));
        b.vertical(1, false);
        assert_eq!(b.cursor, (2, 5));
    }

    #[test]
    fn words_and_home() {
        let mut b = buf("  foo.bar baz");
        b.doc_end(false);
        b.word_left(false);
        assert_eq!(b.cursor.1, 10);
        b.word_left(false);
        assert_eq!(b.cursor.1, 6);
        b.word_left(false);
        assert_eq!(b.cursor.1, 5);
        b.word_right(false);
        assert_eq!(b.cursor.1, 6);
        b.home(false);
        assert_eq!(b.cursor.1, 2);
        b.home(false);
        assert_eq!(b.cursor.1, 0);
    }

    #[test]
    fn find_wraps_and_goes_both_ways() {
        let mut b = buf("foo bar\nBaz foo");
        let (s, e) = b.find("foo", true).unwrap();
        assert_eq!((s, e), ((0, 0), (0, 3)));
        b.select_range(s, e);
        let (s, e) = b.find("foo", true).unwrap();
        assert_eq!(s, (1, 4));
        b.select_range(s, e);
        let (s, e) = b.find("foo", true).unwrap();
        assert_eq!(s, (0, 0)); // wrapped
        b.select_range(s, e);
        let (s2, e2) = b.find("foo", false).unwrap();
        b.select_range(s2, e2);
        assert_eq!(s2, (1, 4)); // backwards from (0,0) wraps to the last
        let (s3, _) = b.find("foo", false).unwrap();
        assert_eq!(s3, (0, 0));
        // lower case ignores case, an upper case letter does not
        assert!(b.find("baz", true).is_some());
        assert!(b.find("BAZ", true).is_none());
        assert!(b.find("zzz", true).is_none());
    }

    #[test]
    fn replace_all_counts_and_is_one_undo_step() {
        let mut b = buf("a b a\na");
        assert_eq!(b.replace_all("a", "xyz"), 3);
        assert_eq!(b.text(), "xyz b xyz\nxyz");
        assert!(b.undo());
        assert_eq!(b.text(), "a b a\na");
        assert_eq!(b.replace_all("q", "z"), 0);
    }

    #[test]
    fn tab_and_back_tab() {
        let mut b = buf("a
b");
        b.tab(4);
        assert_eq!(b.lines[0], "    a");
        b.select_range((0, 0), (1, 1));
        b.tab(4);
        assert_eq!(b.lines, vec!["        a", "    b"]);
        b.back_tab(4);
        assert_eq!(b.lines, vec!["    a", "b"]);
    }

    #[test]
    fn cut_line_removes_it() {
        let mut b = buf("one\ntwo\nthree");
        b.move_to((1, 1), false);
        assert_eq!(b.cut_line(), "two\n");
        assert_eq!(b.text(), "one\nthree");
        let mut single = buf("only");
        assert_eq!(single.cut_line(), "only");
        assert_eq!(single.text(), "");
    }

    #[test]
    fn goto_clamps() {
        let mut b = buf("one\ntwo");
        b.goto_line(99, 99);
        assert_eq!(b.cursor, (1, 3));
        b.goto_line(1, 2);
        assert_eq!(b.cursor, (0, 1));
    }
}

#[cfg(test)]
mod multi_cursor_tests {
    use super::*;

    fn buf(text: &str) -> Buffer {
        Buffer::new(text)
    }

    #[test]
    fn ctrl_d_selects_the_word_then_adds_each_next_occurrence() {
        let mut b = buf("foo bar foo baz foo");
        b.move_to((0, 1), false);
        assert!(b.add_next_occurrence());
        assert_eq!(b.selection(), Some(((0, 0), (0, 3))));
        assert_eq!(b.cursor_count(), 1);
        assert!(b.add_next_occurrence());
        assert_eq!(b.cursor_count(), 2);
        assert_eq!(b.selection(), Some(((0, 8), (0, 11)))); // the newest is the primary one
        assert!(b.add_next_occurrence());
        assert_eq!(b.cursor_count(), 3);
        assert!(!b.add_next_occurrence()); // all taken
        b.insert_char('X');
        assert_eq!(b.text(), "X bar X baz X");
        assert_eq!(b.cursor_count(), 3);
        // one undo step brings the text and the three cursors back
        assert!(b.undo());
        assert_eq!(b.text(), "foo bar foo baz foo");
        assert_eq!(b.cursor_count(), 3);
        assert_eq!(b.selections().len(), 3);
    }

    #[test]
    fn a_word_started_from_a_bare_cursor_matches_whole_words_only() {
        let mut b = buf("foo foobar foo");
        assert!(b.add_next_occurrence());
        assert!(b.add_next_occurrence());
        assert_eq!(b.selection(), Some(((0, 11), (0, 14)))); // skipped the one inside foobar
        // a selection made by hand matches inside words too
        let mut b = buf("foo foobar foo");
        b.select_range((0, 0), (0, 3));
        assert!(b.add_next_occurrence());
        assert_eq!(b.selection(), Some(((0, 4), (0, 7))));
    }

    #[test]
    fn typing_at_several_cursors_on_one_line_keeps_them_in_place() {
        let mut b = buf("a a a");
        b.move_to((0, 0), false);
        assert_eq!(b.select_all_occurrences(), 3);
        b.insert_str("bb");
        assert_eq!(b.text(), "bb bb bb");
        b.insert_char('!');
        assert_eq!(b.text(), "bb! bb! bb!");
        let mut cursors: Vec<Pos> = b.sels().iter().map(|s| s.cursor).collect();
        cursors.sort();
        assert_eq!(cursors, vec![(0, 3), (0, 7), (0, 11)]);
    }

    #[test]
    fn backspace_and_delete_work_at_every_cursor_and_join_lines() {
        let mut b = buf("a\nb\nc");
        b.move_to((1, 0), false);
        b.add_cursor((2, 0));
        b.backspace();
        assert_eq!(b.text(), "abc");
        assert_eq!(b.cursor_count(), 2);

        let mut b = buf("xa\nxb\nxc");
        b.move_to((0, 0), false);
        b.add_cursor((1, 0));
        b.add_cursor((2, 0));
        b.delete();
        assert_eq!(b.text(), "a\nb\nc");
        // the cursor at the start of the document does nothing on backspace
        let mut b = buf("ab\nab");
        b.move_to((0, 0), false);
        b.add_cursor((1, 1));
        b.backspace();
        assert_eq!(b.text(), "ab\nb");
    }

    #[test]
    fn enter_keeps_each_lines_indentation() {
        let mut b = buf("  x\n    y");
        b.move_to((0, 3), false);
        b.add_cursor((1, 5));
        b.newline();
        assert_eq!(b.lines, vec!["  x", "  ", "    y", "    "]);
        b.insert_char('z');
        assert_eq!(b.lines, vec!["  x", "  z", "    y", "    z"]);
    }

    #[test]
    fn cursors_can_be_added_above_and_below() {
        let mut b = buf("abc\nabc\nabc");
        b.move_to((0, 1), false);
        assert!(b.add_cursor_vertical(1));
        assert!(b.add_cursor_vertical(1));
        assert!(!b.add_cursor_vertical(1)); // nothing below the last line
        assert_eq!(b.cursor_count(), 3);
        b.insert_char('X');
        assert_eq!(b.text(), "aXbc\naXbc\naXbc");
        assert!(!b.add_cursor_vertical(-1)); // the first cursor is on the first line already
    }

    #[test]
    fn short_lines_do_not_lose_the_column() {
        let mut b = buf("abcdef\nab\nabcdef");
        b.move_to((0, 5), false);
        assert!(b.add_cursor_vertical(1)); // on the short line: column 2, remembers 5
        assert!(b.add_cursor_vertical(1)); // the long line again: column 5
        let mut cursors: Vec<Pos> = b.sels().iter().map(|s| s.cursor).collect();
        cursors.sort();
        assert_eq!(cursors, vec![(0, 5), (1, 2), (2, 5)]);
    }

    #[test]
    fn movement_moves_every_cursor_and_merges_those_that_meet() {
        let mut b = buf("abc\ndef");
        b.move_to((0, 1), false);
        b.add_cursor((1, 1));
        b.right(false);
        let mut cursors: Vec<Pos> = b.sels().iter().map(|s| s.cursor).collect();
        cursors.sort();
        assert_eq!(cursors, vec![(0, 2), (1, 2)]);
        b.home(false);
        b.home(false);
        b.end(false);
        assert_eq!(b.cursor_count(), 2);
        // two cursors side by side become one when they reach the same place
        let mut b = buf("abc");
        b.move_to((0, 0), false);
        b.add_cursor((0, 1));
        b.left(false);
        assert_eq!(b.cursor_count(), 1);
        assert_eq!(b.cursor, (0, 0));
    }

    #[test]
    fn shift_movement_extends_every_selection() {
        let mut b = buf("ab cd\nef gh");
        b.move_to((0, 0), false);
        b.add_cursor((1, 0));
        b.word_right(true);
        assert_eq!(b.selected_text().unwrap(), "ab \nef ");
        b.left(false); // collapses each to its start
        assert!(b.selection().is_none());
        assert_eq!(b.cursor_count(), 2);
    }

    #[test]
    fn paste_gives_each_cursor_a_line_when_the_counts_match() {
        let mut b = buf("\n\n");
        b.move_to((0, 0), false);
        b.add_cursor((1, 0));
        b.add_cursor((2, 0));
        b.insert_str("1\n2\n3");
        assert_eq!(b.text(), "1\n2\n3");
        // another count: the whole text at each cursor
        let mut b = buf("\n\n");
        b.move_to((0, 0), false);
        b.add_cursor((1, 0));
        b.add_cursor((2, 0));
        b.insert_str("x");
        assert_eq!(b.text(), "x\nx\nx");
        let mut b = buf("\n\n");
        b.move_to((0, 0), false);
        b.add_cursor((1, 0));
        b.add_cursor((2, 0));
        b.insert_str("a\nb");
        assert_eq!(b.text(), "a\nb\na\nb\na\nb");
    }

    #[test]
    fn copy_and_cut_use_all_selections() {
        let mut b = buf("one two one");
        b.move_to((0, 0), false);
        assert!(b.add_next_occurrence());
        assert!(b.add_next_occurrence());
        assert_eq!(b.selected_text().unwrap(), "one\none");
        assert_eq!(b.cut_selection().unwrap(), "one\none");
        assert_eq!(b.text(), " two ");
        assert_eq!(b.cursor_count(), 2);
    }

    #[test]
    fn cursors_at_the_ends_of_the_selected_lines() {
        let mut b = buf("abc\ndef\nghi");
        b.select_range((0, 1), (2, 1));
        assert!(b.cursors_at_line_ends());
        assert_eq!(b.cursor_count(), 3);
        b.insert_char(';');
        assert_eq!(b.text(), "abc;\ndef;\nghi;");
        // a selection inside one line does nothing
        let mut b = buf("abc");
        b.select_range((0, 0), (0, 2));
        assert!(!b.cursors_at_line_ends());
    }

    #[test]
    fn escape_goes_back_to_one_cursor_and_a_click_does_too() {
        let mut b = buf("a a");
        b.move_to((0, 0), false);
        b.add_cursor((0, 2));
        assert_eq!(b.cursor_count(), 2);
        assert!(b.clear_extra_cursors());
        assert_eq!(b.cursor_count(), 1);
        assert!(!b.clear_extra_cursors());
        b.add_cursor((0, 1));
        b.move_to((0, 0), false);
        assert_eq!(b.cursor_count(), 1);
    }

    #[test]
    fn tab_and_untab_at_several_cursors() {
        let mut b = buf("a\nbb");
        b.move_to((0, 0), false);
        b.add_cursor((1, 2));
        b.tab(4);
        assert_eq!(b.lines, vec!["    a", "bb  "]);
        // selections over lines indent whole lines, then take it back
        let mut b = buf("a\nb\nc");
        b.select_range((0, 0), (1, 1));
        b.add_cursor((2, 0));
        b.tab(4);
        assert_eq!(b.lines, vec!["    a", "    b", "    c"]);
        b.back_tab(4);
        assert_eq!(b.lines, vec!["a", "b", "c"]);
    }

    #[test]
    fn cut_line_cuts_the_line_of_every_cursor() {
        let mut b = buf("one\ntwo\nthree\nfour");
        b.move_to((0, 1), false);
        b.add_cursor((2, 2));
        assert_eq!(b.cut_line(), "one\nthree\n");
        assert_eq!(b.text(), "two\nfour");
        assert_eq!(b.cursor_count(), 2);
    }

    #[test]
    fn delete_word_works_at_every_cursor() {
        let mut b = buf("ab cd\nef gh");
        b.move_to((0, 5), false);
        b.add_cursor((1, 5));
        b.delete_word_back();
        assert_eq!(b.text(), "ab \nef ");
    }

    #[test]
    fn typing_after_moving_is_a_new_undo_step() {
        let mut b = buf("");
        b.insert_char('a');
        b.left(false);
        b.right(false);
        b.insert_char('b');
        assert_eq!(b.text(), "ab");
        assert!(b.undo());
        assert_eq!(b.text(), "a");
    }

    #[test]
    fn select_all_occurrences_needs_a_word_and_reports_the_count() {
        let mut b = buf("x foo y foo");
        b.move_to((0, 0), false); // on `x`: one occurrence
        assert_eq!(b.select_all_occurrences(), 1);
        let mut b = buf("   ");
        assert_eq!(b.select_all_occurrences(), 0);
        let mut b = buf("foo\nbar foo");
        b.move_to((0, 1), false);
        assert_eq!(b.select_all_occurrences(), 2);
        assert_eq!(b.selection(), Some(((0, 0), (0, 3)))); // the one at the cursor is primary
    }
}
