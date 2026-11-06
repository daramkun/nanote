//! Drawing: the menu bar, the tabs of the open files (when there are several), the text with
//! line numbers, and the status line (or a prompt).

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::UnicodeWidthChar;

use crate::{
    app::{App, Prompt},
    doc::Doc,
    menu::{self, Entry, MENUS},
};

pub const TAB_WIDTH: usize = 4;

/// Where the pieces of the screen are: the menu bar on row 0, the tabs on row 1 when there is
/// more than one file, then the text, and the status line on the last row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    /// Width of the line-number column, separator included.
    pub gutter: u16,
    /// Cells available for text.
    pub text_w: usize,
    /// Rows available for text.
    pub text_h: usize,
    /// The screen row of the first text row.
    pub top: u16,
}

pub fn metrics(lines: usize, width: u16, height: u16, tabs: bool) -> Metrics {
    let digits = lines.max(1).to_string().len() as u16;
    let gutter = (digits + 1).max(4).min(width / 2);
    let top = if tabs { 2 } else { 1 };
    Metrics {
        gutter,
        text_w: width.saturating_sub(gutter).max(1) as usize,
        text_h: height.saturating_sub(top + 1).max(1) as usize,
        top,
    }
}

/// The text of a file's tab.
pub fn tab_label(doc: &Doc) -> String {
    format!(" {}{} ", doc.name(), if doc.is_dirty() { "*" } else { "" })
}

/// The tab at screen column `x`.
pub fn tab_at(labels: &[String], x: u16) -> Option<usize> {
    let mut start = 0usize;
    for (i, label) in labels.iter().enumerate() {
        let w = label.chars().count();
        if (x as usize) >= start && (x as usize) < start + w {
            return Some(i);
        }
        start += w;
    }
    None
}

/// What a character looks like on screen: its text and its width in cells.
fn glyph(c: char, at_cell: usize) -> (String, usize) {
    match c {
        '\t' => {
            let n = TAB_WIDTH - at_cell % TAB_WIDTH;
            (" ".repeat(n), n)
        }
        c if c.is_control() => ("·".into(), 1),
        c => match UnicodeWidthChar::width(c) {
            Some(w) if w > 0 => (c.to_string(), w),
            // combining marks have no width of their own: they are drawn with their base
            _ => (c.to_string(), 0),
        },
    }
}

/// The screen cell (from the line start) where character `col` begins.
pub fn cell_of(line: &str, col: usize) -> usize {
    let mut cell = 0;
    for c in line.chars().take(col) {
        cell += glyph(c, cell).1;
    }
    cell
}

/// The character position for screen cell `cell` (clamped to the line end); the right half of
/// a wide character or a tab counts as the position after it.
pub fn col_at(line: &str, cell: usize) -> usize {
    let mut at = 0;
    for (i, c) in line.chars().enumerate() {
        let w = glyph(c, at).1;
        if cell < at + w {
            return if w > 1 && cell >= at + w / 2 { i + 1 } else { i };
        }
        at += w;
    }
    line.chars().count()
}

/// The spans of one line within the horizontal window `[scroll, scroll + width)`.
pub fn line_spans(
    line: &str,
    styles: &[Style],
    selected: &[(usize, usize)],
    carets: &[usize],
    scroll: usize,
    width: usize,
) -> Vec<Span<'static>> {
    let mut out: Vec<(String, Style)> = Vec::new();
    let mut push = |text: &str, style: Style| {
        if let Some((last, s)) = out.last_mut() {
            if *s == style {
                last.push_str(text);
                return;
            }
        }
        out.push((text.to_owned(), style));
    };
    let mut cell = 0;
    for (i, c) in line.chars().enumerate() {
        if cell >= scroll + width {
            break;
        }
        let (text, w) = glyph(c, cell);
        let mut style = styles.get(i).copied().unwrap_or_default();
        if selected.iter().any(|&(s, e)| i >= s && i < e) || carets.contains(&i) {
            style = style.add_modifier(Modifier::REVERSED);
        }
        if c.is_control() && c != '\t' {
            style = style.fg(Color::DarkGray);
        }
        if cell + w <= scroll {
            cell += w;
            continue;
        }
        if cell < scroll {
            // a wide character cut by the left edge
            push(&" ".repeat(cell + w - scroll), style);
        } else if cell + w > scroll + width {
            push(&" ".repeat(scroll + width - cell), style);
        } else {
            push(&text, style);
        }
        cell += w;
    }
    // an extra cursor after the last character is drawn as a highlighted blank
    let len = line.chars().count();
    if carets.contains(&len) && cell >= scroll && cell < scroll + width {
        out.push((" ".into(), Style::default().add_modifier(Modifier::REVERSED)));
    }
    out.into_iter().map(|(t, s)| Span::styled(t, s)).collect()
}

pub const HELP_TEXT: &[&str] = &[
    "nanote-tui - keys",
    "",
    "Menus      F10 or Alt+F / Alt+E / Alt+S / Alt+O / Alt+H (File, Edit, Search, Format, Help)",
    "           arrows choose, Enter runs, Esc closes; the mouse works too",
    "File       Ctrl+S save          F2 save as          Ctrl+P open a file",
    "           Ctrl+N new file      Ctrl+W close file   Ctrl+Q quit",
    "           Alt+Left/Right or Ctrl+PageUp/PageDown: switch between files",
    "Edit       Ctrl+Z undo          Ctrl+Y redo         Ctrl+A select all",
    "           Ctrl+C copy          Ctrl+X cut          Ctrl+V paste",
    "           Ctrl+K cut line      Tab / Shift+Tab indent / unindent",
    "           Ctrl+Backspace / Ctrl+Delete delete a word",
    "Move       arrows, Home (indent / start), End, PageUp/PageDown",
    "           Ctrl+Left/Right by word    Ctrl+Home/End start / end of file",
    "           Shift + any of these (or the mouse) selects",
    "Cursors    Ctrl+D select the word, then add the next occurrence",
    "           Ctrl+L cursors on all occurrences   Esc back to one cursor",
    "           Alt+Click add a cursor   Ctrl+Alt+Up/Down (or Alt+Shift+Up/Down)",
    "           Alt+Shift+I cursors at the ends of the selected lines",
    "Search     Ctrl+F find          F3 / Shift+F3 next / previous",
    "           Ctrl+R replace all   Ctrl+G go to line[:column]",
    "Format     Ctrl+E encoding to save with; the Format menu also has",
    "           reopen with an encoding, BOM, LF / CRLF and syntax",
    "",
    "Copying also sends the text to the terminal's clipboard (OSC 52).",
    "Press any key to close this help.",
];

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 10 || area.height < 4 {
        return;
    }
    app.follow_hit = None;
    let m = metrics(app.doc().buf.line_count(), area.width, area.height, app.docs.len() > 1);

    // ---- menu bar
    let bar = Style::default().add_modifier(Modifier::REVERSED);
    let mut spans: Vec<Span> = Vec::new();
    let mut used = 0usize;
    for (i, m) in MENUS.iter().enumerate() {
        let open = app.menu.is_some_and(|s| s.menu == i);
        let base = if open { Style::default().add_modifier(Modifier::BOLD) } else { bar };
        spans.push(Span::styled(" ", base));
        for c in m.title.chars() {
            let style = if c.to_ascii_lowercase() == m.mnemonic {
                base.add_modifier(Modifier::UNDERLINED)
            } else {
                base
            };
            spans.push(Span::styled(c.to_string(), style));
        }
        spans.push(Span::styled(" ", base));
        used += m.title.chars().count() + 2;
    }
    spans.push(Span::styled(
        " ".repeat((area.width as usize).saturating_sub(used)),
        bar,
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), Rect::new(0, 0, area.width, 1));

    // ---- tabs of the open files
    if m.top == 2 {
        let spans: Vec<Span> = app
            .docs
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let style = if i == app.current {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default().add_modifier(Modifier::DIM)
                };
                Span::styled(tab_label(d), style)
            })
            .collect();
        frame.render_widget(Paragraph::new(Line::from(spans)), Rect::new(0, 1, area.width, 1));
    }

    // ---- text
    let hl = &app.hl;
    let doc = &mut app.docs[app.current];
    let selections = doc.buf.selections();
    let carets = doc.buf.extra_carets();
    let cursor = doc.buf.cursor;
    let mut rows: Vec<Line> = Vec::with_capacity(m.text_h);
    for screen_row in 0..m.text_h {
        let row = doc.scroll_row + screen_row;
        if row >= doc.buf.lines.len() {
            rows.push(Line::from(Span::styled(
                "~",
                Style::default().fg(Color::DarkGray),
            )));
            continue;
        }
        let number_style = if row == cursor.0 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let number = format!(
            "{:>w$} ",
            row + 1,
            w = m.gutter as usize - 1
        );
        let sel: Vec<(usize, usize)> = selections
            .iter()
            .filter(|(s, e)| row >= s.0 && row <= e.0)
            .map(|(s, e)| {
                (
                    if row == s.0 { s.1 } else { 0 },
                    if row == e.0 { e.1 } else { usize::MAX },
                )
            })
            .collect();
        let row_carets: Vec<usize> = carets
            .iter()
            .filter(|c| c.0 == row)
            .map(|c| c.1)
            .collect();
        let styles = doc.hl.line(hl, &doc.buf.lines, row).to_vec();
        let mut spans = vec![Span::styled(number, number_style)];
        spans.extend(line_spans(
            &doc.buf.lines[row],
            &styles,
            &sel,
            &row_carets,
            doc.scroll_col,
            m.text_w,
        ));
        rows.push(Line::from(spans));
    }
    frame.render_widget(
        Paragraph::new(rows),
        Rect::new(0, m.top, area.width, m.text_h as u16),
    );

    // ---- status line or prompt
    let status_rect = Rect::new(0, area.height - 1, area.width, 1);
    let mut cursor_at: Option<(u16, u16)> = None;
    let doc = &app.docs[app.current];
    if let Some(prompt) = &app.prompt {
        let label = format!("{} ", prompt.label);
        let shown: String = prompt.input.iter().collect();
        let line = Line::from(vec![
            Span::styled(
                label.clone(),
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED),
            ),
            Span::raw(format!(" {shown}")),
        ]);
        frame.render_widget(Paragraph::new(line), status_rect);
        if prompt.takes_text() {
            let before: String = prompt.input.iter().take(prompt.cursor).collect();
            let x = label.chars().count() as u16 + 1 + cell_of(&before, before.chars().count()) as u16;
            cursor_at = Some((x.min(area.width - 1), area.height - 1));
        }
    } else if !app.status.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {}", app.status),
                Style::default().fg(Color::Yellow),
            ))),
            status_rect,
        );
    } else {
        let bom = if doc.bom { " BOM" } else { "" };
        let eol = if doc.crlf { "CRLF" } else { "LF" };
        let place = doc
            .path
            .as_ref()
            .map_or_else(|| "Untitled".to_owned(), |p| p.display().to_string());
        let left = format!(
            " {}{}",
            place,
            if doc.is_dirty() { " [modified]" } else { "" }
        );
        let cursors = match doc.buf.cursor_count() {
            1 => String::new(),
            n => format!("{n} cursors  "),
        };
        let follow = if doc.follow { "[x] Follow  " } else { "[ ] Follow  " };
        let right = format!(
            "{follow}{cursors}Ln {}/{}, Col {}  {}  {}{} {} ",
            cursor.0 + 1,
            doc.buf.line_count(),
            cursor.1 + 1,
            doc.syntax(),
            doc.encoding,
            bom,
            eol
        );
        // A long path gives way to the right part (position, encoding, the Follow switch).
        let room = (area.width as usize).saturating_sub(right.chars().count());
        let left = if left.chars().count() > room {
            let keep = room.saturating_sub(2);
            let mut tail: Vec<char> = left.chars().rev().take(keep).collect();
            tail.reverse();
            format!(" …{}", tail.into_iter().collect::<String>())
        } else {
            left
        };
        let pad = (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
        let text = format!("{left}{}{right}", " ".repeat(pad));
        // the switch is clickable: remember where it is
        let from = (left.chars().count() + pad) as u16;
        app.follow_hit = Some((from, from + follow.trim_end().chars().count() as u16));
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().add_modifier(Modifier::REVERSED),
            ))),
            status_rect,
        );
    }

    // ---- cursor
    if cursor_at.is_none() && !app.show_help && app.menu.is_none() {
        let line = &doc.buf.lines[cursor.0];
        let cell = cell_of(line, cursor.1);
        if cursor.0 >= doc.scroll_row
            && cursor.0 < doc.scroll_row + m.text_h
            && cell >= doc.scroll_col
            && cell < doc.scroll_col + m.text_w
        {
            cursor_at = Some((
                m.gutter + (cell - doc.scroll_col) as u16,
                m.top + (cursor.0 - doc.scroll_row) as u16,
            ));
        }
    }

    // ---- the open menu
    if let Some(state) = app.menu {
        let entries = MENUS[state.menu].entries;
        let (x, _) = menu::title_span(state.menu);
        let width = menu::dropdown_width(state.menu).min(area.width.saturating_sub(x));
        let height = (entries.len() as u16 + 2).min(area.height.saturating_sub(1));
        let rect = Rect::new(x, 1, width, height);
        let inner = width.saturating_sub(2) as usize;
        let lines: Vec<Line> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| match e {
                Entry::Separator => Line::from("\u{2500}".repeat(inner)),
                Entry::Item { label, keys, command } => {
                    let label = match command {
                        crate::menu::Command::ToggleBom => {
                            format!("{label} ({})", if app.doc().bom { "on" } else { "off" })
                        }
                        crate::menu::Command::ToggleLineEnding => {
                            format!("{label} ({})", if app.doc().crlf { "CRLF" } else { "LF" })
                        }
                        crate::menu::Command::Follow => {
                            format!("{label} ({})", if app.doc().follow { "on" } else { "off" })
                        }
                        _ => (*label).to_owned(),
                    };
                    let gap = inner.saturating_sub(label.chars().count() + keys.chars().count() + 2);
                    let text = format!(" {label}{}{keys} ", " ".repeat(gap));
                    let style = if i == state.entry {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    Line::from(Span::styled(text, style))
                }
            })
            .collect();
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL)),
            rect,
        );
        cursor_at = None;
    }

    // ---- help overlay
    if app.show_help {
        let w = (HELP_TEXT.iter().map(|l| l.chars().count()).max().unwrap_or(40) as u16 + 4)
            .min(area.width);
        let h = (HELP_TEXT.len() as u16 + 2).min(area.height);
        let rect = Rect::new(
            (area.width - w) / 2,
            (area.height - h) / 2,
            w,
            h,
        );
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(HELP_TEXT.iter().map(|l| Line::from(*l)).collect::<Vec<_>>())
                .block(Block::default().borders(Borders::ALL))
                .alignment(Alignment::Left),
            rect,
        );
        cursor_at = None;
    }
    if let Some((x, y)) = cursor_at {
        frame.set_cursor_position((x, y));
    }
}

impl Prompt {
    pub fn takes_text(&self) -> bool {
        self.kind.takes_text()
    }
}
