//! The editor: open files, key and mouse handling, prompts (find, go to, save as, ...).

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    layout::Size,
};

use crate::{
    doc::{Disk, Doc},
    encoding::ENCODINGS,
    highlight::Highlighter,
    menu::{self, Command, MENUS, MenuState},
    ui::{self, Metrics, TAB_WIDTH},
};

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PromptKind {
    Find,
    ReplaceFind,
    /// The text to look for was typed; ask what to replace it with.
    ReplaceWith(String),
    Goto,
    SaveAs,
    Open,
    Encoding,
    Reopen,
    Syntax,
    /// Unsaved changes while quitting: y / n / Esc.
    ConfirmQuit,
    /// Unsaved changes while closing one file: y / n / Esc.
    ConfirmClose,
    /// The file changed on disk and has unsaved edits here: reload (y) or keep mine (n).
    ConfirmReload(usize),
    /// The file changed on disk since it was read: overwrite it (y)? `Some` is a Save As target.
    ConfirmOverwrite(Option<PathBuf>),
}

impl PromptKind {
    pub fn takes_text(&self) -> bool {
        !matches!(
            self,
            PromptKind::ConfirmQuit
                | PromptKind::ConfirmClose
                | PromptKind::ConfirmReload(_)
                | PromptKind::ConfirmOverwrite(_)
        )
    }
}

pub struct Prompt {
    pub kind: PromptKind,
    pub label: String,
    pub input: Vec<char>,
    /// Cursor in `input`, in characters.
    pub cursor: usize,
}

impl Prompt {
    fn new(kind: PromptKind, label: &str, initial: &str) -> Self {
        let input: Vec<char> = initial.chars().collect();
        Self {
            kind,
            label: label.to_owned(),
            cursor: input.len(),
            input,
        }
    }

    fn text(&self) -> String {
        self.input.iter().collect()
    }
}

pub struct App {
    pub docs: Vec<Doc>,
    pub current: usize,
    pub hl: Highlighter,
    pub prompt: Option<Prompt>,
    pub status: String,
    pub show_help: bool,
    /// The open menu of the menu bar.
    pub menu: Option<MenuState>,
    pub quit: bool,
    /// Text for the terminal's clipboard (OSC 52), written by the main loop.
    pub osc52: Option<String>,
    clipboard: String,
    last_find: String,
    size: Size,
    /// When the files on disk were last looked at.
    last_check: Instant,
    /// Where the "Follow" switch of the status line is on screen: `(first column, end column)`
    /// of the last row (drawn by `ui`).
    pub follow_hit: Option<(u16, u16)>,
}

fn expand_home(path: &str) -> PathBuf {
    let path = path.trim().trim_matches('"');
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

impl App {
    pub fn new(hl: Highlighter, size: Size) -> Self {
        let first = Doc::untitled(&hl);
        Self {
            docs: vec![first],
            current: 0,
            hl,
            prompt: None,
            status: String::new(),
            show_help: false,
            menu: None,
            quit: false,
            osc52: None,
            clipboard: String::new(),
            last_find: String::new(),
            size,
            last_check: Instant::now(),
            follow_hit: None,
        }
    }

    pub fn doc(&self) -> &Doc {
        &self.docs[self.current]
    }

    fn doc_mut(&mut self) -> &mut Doc {
        &mut self.docs[self.current]
    }

    pub fn metrics(&self) -> Metrics {
        ui::metrics(
            self.doc().buf.line_count(),
            self.size.width,
            self.size.height,
            self.docs.len() > 1,
        )
    }

    pub fn resize(&mut self, size: Size) {
        self.size = size;
        self.settle();
    }

    /// Open a file in a new buffer (or switch to it when already open). The very first file
    /// replaces the empty buffer the editor starts with.
    pub fn open(&mut self, path: &str, forced: Option<&str>) {
        let target = std::path::absolute(expand_home(path)).unwrap_or_else(|_| expand_home(path));
        if forced.is_none() {
            if let Some(i) = self.docs.iter().position(|d| d.path.as_deref() == Some(&target)) {
                self.current = i;
                return self.settle();
            }
        }
        match Doc::open(&target, forced, &self.hl) {
            Ok(doc) => {
                let replace_empty = forced.is_none()
                    && self.docs.len() == 1
                    && self.docs[0].path.is_none()
                    && !self.docs[0].is_dirty();
                if forced.is_some() {
                    self.docs[self.current] = doc;
                } else if replace_empty {
                    self.docs[0] = doc;
                    self.current = 0;
                } else {
                    self.docs.push(doc);
                    self.current = self.docs.len() - 1;
                }
            }
            Err(e) => self.status = format!("Cannot open {}: {e}", target.display()),
        }
        self.settle();
    }

    /// After the files of the command line are open: show the first one.
    pub fn show_first(&mut self) {
        self.current = 0;
        self.settle();
    }

    /// `+LINE` on the command line.
    pub fn goto(&mut self, line: usize) {
        self.doc_mut().buf.goto_line(line, 1);
        self.settle();
    }

    /// Scroll so that the cursor is visible; bring the highlighter up to date.
    pub fn settle(&mut self) {
        let m = self.metrics();
        let doc = &mut self.docs[self.current];
        if doc.buf.changed_from != usize::MAX {
            doc.hl.invalidate(doc.buf.changed_from);
            doc.buf.changed_from = usize::MAX;
        }
        let (row, col) = doc.buf.cursor;
        if row < doc.scroll_row {
            doc.scroll_row = row;
        } else if row >= doc.scroll_row + m.text_h {
            doc.scroll_row = row + 1 - m.text_h;
        }
        doc.scroll_row = doc.scroll_row.min(doc.buf.line_count().saturating_sub(1));
        let cell = ui::cell_of(&doc.buf.lines[row], col);
        if cell < doc.scroll_col {
            doc.scroll_col = cell;
        } else if cell >= doc.scroll_col + m.text_w {
            doc.scroll_col = cell + 1 - m.text_w;
        }
    }

    fn copy_to_clipboard(&mut self, text: String) {
        self.osc52 = Some(text.clone());
        self.clipboard = text;
    }

    // ------------------------------------------------------------------ keys

    pub fn key(&mut self, key: KeyEvent) {
        self.status.clear();
        if self.show_help {
            self.show_help = false;
            return;
        }
        if self.menu.is_some() {
            self.menu_key(key);
        } else if self.prompt.is_some() {
            self.prompt_key(key);
        } else {
            self.editor_key(key);
        }
        self.settle();
    }

    // ------------------------------------------------------------------ menus

    fn open_menu(&mut self, index: usize) {
        self.menu = Some(MenuState::open(index));
    }

    fn menu_key(&mut self, key: KeyEvent) {
        let Some(state) = self.menu.as_mut() else {
            return;
        };
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc | KeyCode::F(10) => self.menu = None,
            KeyCode::Left => state.switch(-1),
            KeyCode::Right => state.switch(1),
            KeyCode::Up => state.step(-1),
            KeyCode::Down => state.step(1),
            KeyCode::Enter => {
                let command = state.command();
                self.menu = None;
                if let Some(command) = command {
                    self.run(command);
                }
            }
            KeyCode::Char(c) if alt => {
                let wanted = c.to_ascii_lowercase();
                match MENUS.iter().position(|m| m.mnemonic == wanted) {
                    Some(i) if i == state.menu => self.menu = None,
                    Some(i) => self.open_menu(i),
                    None => {}
                }
            }
            _ => {}
        }
    }

    fn editor_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let m = self.metrics();

        if let Some(command) = menu::shortcut(key) {
            return self.run(command);
        }
        match key.code {
            KeyCode::F(10) => self.open_menu(0),
            // Alt + a menu's letter opens it
            KeyCode::Char(c) if alt && !ctrl => {
                let wanted = c.to_ascii_lowercase();
                if let Some(i) = MENUS.iter().position(|m| m.mnemonic == wanted) {
                    self.open_menu(i);
                }
            }
            KeyCode::Esc => {
                // first back to one cursor, then drop the selection
                if !self.doc_mut().buf.clear_extra_cursors() {
                    self.doc_mut().buf.anchor = None;
                }
            }

            // ---- movement (Shift extends the selection)
            KeyCode::Left if ctrl => self.doc_mut().buf.word_left(shift),
            KeyCode::Right if ctrl => self.doc_mut().buf.word_right(shift),
            KeyCode::Left => self.doc_mut().buf.left(shift),
            KeyCode::Right => self.doc_mut().buf.right(shift),
            KeyCode::Up => self.doc_mut().buf.vertical(-1, shift),
            KeyCode::Down => self.doc_mut().buf.vertical(1, shift),
            KeyCode::PageUp => self.doc_mut().buf.vertical(-(m.text_h as isize - 1).max(1), shift),
            KeyCode::PageDown => self.doc_mut().buf.vertical((m.text_h as isize - 1).max(1), shift),
            KeyCode::Home if ctrl => self.doc_mut().buf.doc_start(shift),
            KeyCode::End if ctrl => self.doc_mut().buf.doc_end(shift),
            KeyCode::Home => self.doc_mut().buf.home(shift),
            KeyCode::End => self.doc_mut().buf.end(shift),

            // ---- editing
            KeyCode::Enter => self.doc_mut().buf.newline(),
            KeyCode::Backspace if ctrl || alt => self.doc_mut().buf.delete_word_back(),
            KeyCode::Backspace => self.doc_mut().buf.backspace(),
            KeyCode::Delete if ctrl => self.doc_mut().buf.delete_word_forward(),
            KeyCode::Delete => self.doc_mut().buf.delete(),
            KeyCode::Tab => self.doc_mut().buf.tab(TAB_WIDTH),
            KeyCode::BackTab => self.doc_mut().buf.back_tab(TAB_WIDTH),
            KeyCode::Char(c) if !ctrl && !alt => self.doc_mut().buf.insert_char(c),
            _ => {}
        }
    }

    /// Do what a menu entry or a shortcut asks for.
    pub fn run(&mut self, command: Command) {
        self.status.clear();
        match command {
            Command::Save => self.save(),
            Command::Quit => self.request_quit(),
            Command::Find => {
                let initial = self.search_seed();
                self.prompt = Some(Prompt::new(PromptKind::Find, "Find:", &initial));
            }
            Command::Replace => {
                let initial = self.search_seed();
                self.prompt = Some(Prompt::new(PromptKind::ReplaceFind, "Replace:", &initial));
            }
            Command::Goto => {
                self.prompt = Some(Prompt::new(PromptKind::Goto, "Go to line[:col]:", ""));
            }
            Command::Open => self.prompt = Some(Prompt::new(PromptKind::Open, "Open:", "")),
            Command::New => {
                self.docs.push(Doc::untitled(&self.hl));
                self.current = self.docs.len() - 1;
            }
            Command::Close => self.request_close(),
            Command::Undo => {
                if !self.doc_mut().buf.undo() {
                    self.status = "Nothing to undo".into();
                }
            }
            Command::Redo => {
                if !self.doc_mut().buf.redo() {
                    self.status = "Nothing to redo".into();
                }
            }
            Command::SelectAll => self.doc_mut().buf.select_all(),
            Command::Copy => match self.doc().buf.selected_text() {
                Some(t) => self.copy_to_clipboard(t),
                None => self.status = "Nothing selected".into(),
            },
            Command::Cut => {
                if let Some(t) = self.doc_mut().buf.cut_selection() {
                    self.copy_to_clipboard(t);
                } else {
                    self.status = "Nothing selected (Ctrl+K cuts the line)".into();
                }
            }
            Command::CutLine => {
                let t = self.doc_mut().buf.cut_line();
                self.copy_to_clipboard(t);
            }
            Command::Paste => {
                let text = self.clipboard.clone();
                self.doc_mut().buf.insert_str(&text);
            }
            Command::NextOccurrence => {
                if !self.doc_mut().buf.add_next_occurrence() {
                    self.status = "No more occurrences".into();
                }
            }
            Command::AllOccurrences => {
                let n = self.doc_mut().buf.select_all_occurrences();
                self.status = if n == 0 {
                    "Select a word or put the cursor on one first".into()
                } else {
                    format!("{n} occurrences selected")
                };
            }
            Command::LineEnds => {
                if !self.doc_mut().buf.cursors_at_line_ends() {
                    self.status = "Select several lines first".into();
                }
            }
            Command::CursorAbove => {
                self.doc_mut().buf.add_cursor_vertical(-1);
            }
            Command::CursorBelow => {
                self.doc_mut().buf.add_cursor_vertical(1);
            }
            Command::FindNext => self.find_again(true),
            Command::FindPrevious => self.find_again(false),
            Command::SaveAs => {
                let initial = self
                    .doc()
                    .path
                    .as_ref()
                    .map_or(String::new(), |p| p.display().to_string());
                self.prompt = Some(Prompt::new(PromptKind::SaveAs, "Save as:", &initial));
            }
            Command::Encoding => {
                let enc = self.doc().encoding.clone();
                self.prompt = Some(Prompt::new(PromptKind::Encoding, "Save with encoding:", &enc));
            }
            Command::Reopen => {
                if self.doc().path.is_none() || self.doc().is_dirty() {
                    self.status = "Save the file first to reopen it with another encoding".into();
                } else {
                    let enc = self.doc().encoding.clone();
                    self.prompt = Some(Prompt::new(PromptKind::Reopen, "Reopen with encoding:", &enc));
                }
            }
            Command::ToggleBom => {
                self.doc_mut().toggle_bom();
                self.status = format!("BOM {}", if self.doc().bom { "on" } else { "off" });
            }
            Command::ToggleLineEnding => {
                self.doc_mut().toggle_crlf();
                self.status = format!("Line ending: {}", if self.doc().crlf { "CRLF" } else { "LF" });
            }
            Command::Syntax => {
                let name = self.doc().syntax().to_owned();
                self.prompt = Some(Prompt::new(PromptKind::Syntax, "Syntax:", &name));
            }
            Command::Follow => self.toggle_follow(),
            Command::Keys => self.show_help = true,
            Command::PreviousFile => self.switch_doc(-1),
            Command::NextFile => self.switch_doc(1),
        }
        self.settle();
    }

    /// A single-line selection is offered as the text to search for.
    fn search_seed(&self) -> String {
        match self.doc().buf.selected_text() {
            Some(t) if !t.contains('\n') => t,
            _ => self.last_find.clone(),
        }
    }

    fn switch_doc(&mut self, delta: isize) {
        let n = self.docs.len() as isize;
        self.current = ((self.current as isize + delta).rem_euclid(n)) as usize;
    }

    // ---------------------------------------------------------------- search

    fn find_again(&mut self, forward: bool) {
        if self.last_find.is_empty() {
            self.status = "Nothing to find yet (Ctrl+F)".into();
            return;
        }
        let query = self.last_find.clone();
        match self.doc().buf.find(&query, forward) {
            Some((s, e)) => self.doc_mut().buf.select_range(s, e),
            None => self.status = format!("Not found: {query}"),
        }
    }

    // ----------------------------------------------------------- file actions

    fn save(&mut self) {
        if self.doc().path.is_none() {
            self.prompt = Some(Prompt::new(PromptKind::SaveAs, "Save as:", ""));
            return;
        }
        if self.doc().disk_differs() {
            self.ask_overwrite(None);
            return;
        }
        self.write_current(None);
    }

    fn ask_overwrite(&mut self, target: Option<PathBuf>) {
        let label = format!(
            "{} changed on disk. Overwrite it? (y/n)",
            self.doc().name()
        );
        self.prompt = Some(Prompt::new(PromptKind::ConfirmOverwrite(target), &label, ""));
    }

    /// Write the current file (to `target` for Save As).
    fn write_current(&mut self, target: Option<PathBuf>) {
        match self.doc_mut().save(target.as_deref()) {
            Ok(()) => {
                if let Some(target) = &target {
                    let first = self.doc().buf.lines[0].clone();
                    let syntax = self.hl.detect(Some(target), &first);
                    self.doc_mut().hl.set_syntax(&syntax);
                }
                self.status = format!("Saved {}", self.doc().name());
            }
            Err(e) => self.status = format!("Cannot save: {e}"),
        }
    }

    // ----------------------------------------------------- files changed outside

    /// Called from the main loop: about once a second, look at the files on disk.
    pub fn tick(&mut self) {
        if self.last_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_check = Instant::now();
        // A question is already on screen (or a menu is open): look again later.
        if self.prompt.is_some() || self.menu.is_some() {
            return;
        }
        for i in 0..self.docs.len() {
            match self.docs[i].check_disk() {
                Disk::Quiet | Disk::Appended => {}
                Disk::Reloaded => self.status = format!("Reloaded {}", self.docs[i].name()),
                Disk::Failed(e) => self.status = format!("Cannot reload {}: {e}", self.docs[i].name()),
                Disk::Conflict => {
                    self.current = i;
                    let label = format!(
                        "{} changed on disk. Reload it and lose your edits? (y/n)",
                        self.docs[i].name()
                    );
                    self.prompt = Some(Prompt::new(PromptKind::ConfirmReload(i), &label, ""));
                    break;
                }
            }
        }
        self.settle();
    }

    /// The "Follow" switch: keep up with text appended to the file and stay at its end.
    fn toggle_follow(&mut self) {
        if self.doc().path.is_none() {
            self.status = "Save the file first to follow its changes".into();
        } else if self.doc().follow {
            self.doc_mut().follow = false;
        } else if self.doc().is_dirty() {
            self.status = "Save or reload the file first to follow its changes".into();
        } else {
            // The switch on the status line shows the state, so there is nothing to announce.
            self.doc_mut().follow = true;
            if let Disk::Failed(e) = self.doc_mut().check_disk() {
                self.status = format!("Cannot reload: {e}");
            }
            self.doc_mut().buf.doc_end(false);
        }
    }

    fn request_quit(&mut self) {
        if self.docs.iter().any(Doc::is_dirty) {
            let n = self.docs.iter().filter(|d| d.is_dirty()).count();
            let label = format!("{n} file(s) modified. Save before quitting? (y/n, Esc cancels)");
            self.prompt = Some(Prompt::new(PromptKind::ConfirmQuit, &label, ""));
        } else {
            self.quit = true;
        }
    }

    fn request_close(&mut self) {
        if self.doc().is_dirty() {
            let label = format!("{} is modified. Save before closing? (y/n, Esc cancels)", self.doc().name());
            self.prompt = Some(Prompt::new(PromptKind::ConfirmClose, &label, ""));
        } else {
            self.close_current();
        }
    }

    fn close_current(&mut self) {
        if self.docs.len() == 1 {
            // the last file closes the editor
            self.quit = true;
            return;
        }
        self.docs.remove(self.current);
        self.current = self.current.min(self.docs.len() - 1);
    }

    // --------------------------------------------------------------- prompts

    fn prompt_key(&mut self, key: KeyEvent) {
        let Some(prompt) = self.prompt.as_mut() else {
            return;
        };
        if !prompt.kind.takes_text() {
            let answer = match key.code {
                KeyCode::Char(c) => Some(c.to_ascii_lowercase()),
                KeyCode::Esc => Some('\u{1b}'),
                _ => None,
            };
            let kind = prompt.kind.clone();
            match answer {
                Some('y') => {
                    self.prompt = None;
                    self.confirm(kind, true);
                }
                Some('n') => {
                    self.prompt = None;
                    self.confirm(kind, false);
                }
                Some('\u{1b}') => {
                    // Esc on "reload?" means "keep mine": do not ask again for this change.
                    if let PromptKind::ConfirmReload(i) = kind {
                        self.docs[i].keep_mine();
                    }
                    self.prompt = None;
                }
                _ => {}
            }
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Enter => {
                let prompt = self.prompt.take().unwrap();
                self.submit(prompt);
            }
            KeyCode::Left => prompt.cursor = prompt.cursor.saturating_sub(1),
            KeyCode::Right => prompt.cursor = (prompt.cursor + 1).min(prompt.input.len()),
            KeyCode::Home => prompt.cursor = 0,
            KeyCode::End => prompt.cursor = prompt.input.len(),
            KeyCode::Backspace => {
                if prompt.cursor > 0 {
                    prompt.cursor -= 1;
                    prompt.input.remove(prompt.cursor);
                }
            }
            KeyCode::Delete => {
                if prompt.cursor < prompt.input.len() {
                    prompt.input.remove(prompt.cursor);
                }
            }
            KeyCode::Char('u') if ctrl => {
                prompt.input.drain(..prompt.cursor);
                prompt.cursor = 0;
            }
            KeyCode::Char(c) if !ctrl => {
                prompt.input.insert(prompt.cursor, c);
                prompt.cursor += 1;
            }
            _ => {}
        }
    }

    /// Text pasted into the terminal while a prompt is open.
    pub fn paste(&mut self, text: &str) {
        self.status.clear();
        if let Some(prompt) = self.prompt.as_mut() {
            if prompt.kind.takes_text() {
                for c in text.chars().filter(|c| !c.is_control()) {
                    prompt.input.insert(prompt.cursor, c);
                    prompt.cursor += 1;
                }
            }
        } else if !self.show_help {
            self.doc_mut().buf.insert_str(text);
        }
        self.settle();
    }

    fn confirm(&mut self, kind: PromptKind, save: bool) {
        match kind {
            PromptKind::ConfirmQuit => {
                if save {
                    for i in 0..self.docs.len() {
                        if !self.docs[i].is_dirty() {
                            continue;
                        }
                        if self.docs[i].path.is_none() {
                            self.current = i;
                            self.status = "Save the untitled file first (Ctrl+S)".into();
                            return;
                        }
                        if self.docs[i].disk_differs() {
                            self.current = i;
                            self.status = format!(
                                "{} changed on disk: save it with Ctrl+S to decide",
                                self.docs[i].name()
                            );
                            return;
                        }
                        if let Err(e) = self.docs[i].save(None) {
                            self.current = i;
                            self.status = format!("Cannot save {}: {e}", self.docs[i].name());
                            return;
                        }
                    }
                }
                self.quit = true;
            }
            PromptKind::ConfirmClose => {
                if save {
                    if self.doc().path.is_none() {
                        self.status = "Save the untitled file first (Ctrl+S)".into();
                        return;
                    }
                    if self.doc().disk_differs() {
                        self.status = format!(
                            "{} changed on disk: save it with Ctrl+S to decide",
                            self.doc().name()
                        );
                        return;
                    }
                    if let Err(e) = self.doc_mut().save(None) {
                        self.status = format!("Cannot save: {e}");
                        return;
                    }
                }
                self.close_current();
            }
            PromptKind::ConfirmReload(i) => {
                if let Some(doc) = self.docs.get_mut(i) {
                    if save {
                        match doc.reload() {
                            Ok(()) => self.status = format!("Reloaded {}", doc.name()),
                            Err(e) => self.status = format!("Cannot reload: {e}"),
                        }
                    } else {
                        doc.keep_mine();
                    }
                }
            }
            PromptKind::ConfirmOverwrite(target) => {
                if save {
                    self.write_current(target);
                }
            }
            _ => {}
        }
    }

    fn submit(&mut self, prompt: Prompt) {
        let text = prompt.text();
        match prompt.kind {
            PromptKind::Find => {
                if !text.is_empty() {
                    self.last_find = text;
                    self.find_again(true);
                }
            }
            PromptKind::ReplaceFind => {
                if !text.is_empty() {
                    self.last_find = text.clone();
                    self.prompt = Some(Prompt::new(
                        PromptKind::ReplaceWith(text.clone()),
                        &format!("Replace \"{text}\" with:"),
                        "",
                    ));
                }
            }
            PromptKind::ReplaceWith(find) => {
                let n = self.doc_mut().buf.replace_all(&find, &text);
                self.status = format!("Replaced {n} occurrence(s)");
            }
            PromptKind::Goto => {
                let mut parts = text.split(':');
                let line = parts.next().and_then(|p| p.trim().parse::<usize>().ok());
                let col = parts.next().and_then(|p| p.trim().parse::<usize>().ok()).unwrap_or(1);
                match line {
                    Some(line) => self.doc_mut().buf.goto_line(line, col),
                    None => self.status = "Type a line number (and optionally :column)".into(),
                }
            }
            PromptKind::SaveAs => {
                if text.trim().is_empty() {
                    return;
                }
                let target = std::path::absolute(expand_home(&text)).unwrap_or_else(|_| expand_home(&text));
                // Writing over a file that changed since it was read here needs a yes.
                if self.doc().path.as_deref() == Some(target.as_path()) && self.doc().disk_differs() {
                    self.ask_overwrite(Some(target));
                } else {
                    self.write_current(Some(target));
                }
            }
            PromptKind::Open => {
                if !text.trim().is_empty() {
                    self.open(&text, None);
                }
            }
            PromptKind::Encoding => match encoding_name(&text) {
                Some(name) => {
                    self.doc_mut().set_encoding(name);
                    self.status = format!("Will save as {name} (Ctrl+S)");
                }
                None => self.status = format!("Unknown encoding: {text}"),
            },
            PromptKind::Reopen => match (encoding_name(&text), self.doc().path.clone()) {
                (Some(name), Some(path)) => self.open(&path.display().to_string(), Some(name)),
                (None, _) => self.status = format!("Unknown encoding: {text}"),
                _ => {}
            },
            PromptKind::Syntax => {
                let wanted = text.trim();
                let found = self.hl.names().into_iter().find(|n| n.eq_ignore_ascii_case(wanted));
                match found {
                    Some(name) => self.doc_mut().hl.set_syntax(&name),
                    None => self.status = format!("Unknown syntax: {wanted}"),
                }
            }
            PromptKind::ConfirmQuit
            | PromptKind::ConfirmClose
            | PromptKind::ConfirmReload(_)
            | PromptKind::ConfirmOverwrite(_) => {}
        }
    }

    // ----------------------------------------------------------------- mouse

    pub fn mouse(&mut self, ev: MouseEvent) {
        if self.show_help {
            return;
        }
        let m = self.metrics();
        let down = matches!(ev.kind, MouseEventKind::Down(MouseButton::Left));
        if down {
            self.status.clear(); // a click brings back the normal status line
        }

        // ---- an open menu takes every click: an entry runs, anywhere else closes it
        if let Some(state) = self.menu {
            if !down {
                return;
            }
            let (x0, _) = menu::title_span(state.menu);
            let width = menu::dropdown_width(state.menu);
            let entries = MENUS[state.menu].entries;
            let inside = ev.column >= x0
                && ev.column < x0 + width
                && ev.row >= 2
                && (ev.row as usize) < 2 + entries.len();
            if inside {
                let mut at = state;
                at.entry = ev.row as usize - 2;
                self.menu = None;
                if let Some(command) = at.command() {
                    self.run(command);
                }
            } else if ev.row == 0 {
                match menu::title_at(ev.column) {
                    Some(i) if i != state.menu => self.open_menu(i),
                    _ => self.menu = None,
                }
            } else {
                self.menu = None;
            }
            return;
        }
        if self.prompt.is_some() {
            return;
        }

        // ---- the Follow switch on the status line
        if down && ev.row + 1 == self.size.height {
            if let Some((from, to)) = self.follow_hit {
                if ev.column >= from && ev.column < to {
                    self.toggle_follow();
                }
            }
            return;
        }

        // ---- the menu bar and the file tabs
        if down && ev.row == 0 {
            if let Some(i) = menu::title_at(ev.column) {
                self.open_menu(i);
            }
            return;
        }
        if down && m.top == 2 && ev.row == 1 {
            let names: Vec<String> = self.docs.iter().map(ui::tab_label).collect();
            if let Some(i) = ui::tab_at(&names, ev.column) {
                self.current = i;
                self.settle();
            }
            return;
        }

        let doc = &mut self.docs[self.current];
        let place = |doc: &Doc, x: u16, y: u16| {
            let row = (doc.scroll_row + (y.saturating_sub(m.top)) as usize).min(doc.buf.line_count() - 1);
            let cell = doc.scroll_col + (x.saturating_sub(m.gutter)) as usize;
            (row, ui::col_at(&doc.buf.lines[row], cell))
        };
        let inside = |y: u16| y >= m.top && (y as usize) < m.top as usize + m.text_h;
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) if inside(ev.row) => {
                let pos = place(doc, ev.column, ev.row);
                if ev.modifiers.contains(KeyModifiers::ALT) {
                    doc.buf.add_cursor(pos);
                } else {
                    doc.buf.move_to(pos, false);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let y = ev.row.clamp(m.top, m.top + m.text_h as u16 - 1);
                let pos = place(doc, ev.column, y);
                doc.buf.move_to(pos, true);
            }
            MouseEventKind::ScrollUp => doc.scroll_row = doc.scroll_row.saturating_sub(3),
            MouseEventKind::ScrollDown => {
                doc.scroll_row = (doc.scroll_row + 3).min(doc.buf.line_count().saturating_sub(1));
            }
            _ => {}
        }
        // Scrolling by wheel must not be undone by "keep the cursor visible".
        if matches!(ev.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
            return;
        }
        self.settle();
    }
}

/// The canonical spelling of an encoding name typed by the user.
pub fn encoding_name(typed: &str) -> Option<&'static str> {
    let typed = typed.trim();
    ENCODINGS.iter().copied().find(|e| e.eq_ignore_ascii_case(typed)).or_else(|| {
        match typed.to_ascii_lowercase().as_str() {
            "utf8" => Some("UTF-8"),
            "utf16" | "utf-16" => Some("UTF-16LE"),
            "utf32" | "utf-32" => Some("UTF-32LE"),
            "euckr" | "cp949" => Some("EUC-KR"),
            "sjis" | "shiftjis" | "shift-jis" => Some("Shift_JIS"),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use ratatui::{
        Terminal,
        backend::TestBackend,
        crossterm::event::{KeyEventKind, KeyEventState},
        style::Color,
    };

    use super::*;
    use crate::menu::Command;

    fn app() -> App {
        App::new(Highlighter::new("base16-ocean.dark"), Size::new(60, 12))
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
    }

    fn key(app: &mut App, code: KeyCode) {
        press(app, code, KeyModifiers::NONE);
    }

    fn ctrl(app: &mut App, c: char) {
        press(app, KeyCode::Char(c), KeyModifiers::CONTROL);
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            if c == '\n' {
                key(app, KeyCode::Enter);
            } else {
                key(app, KeyCode::Char(c));
            }
        }
    }

    fn clear_prompt(app: &mut App) {
        let p = app.prompt.as_mut().unwrap();
        p.input.clear();
        p.cursor = 0;
    }

    fn tall_app() -> App {
        App::new(Highlighter::new("base16-ocean.dark"), Size::new(60, 24))
    }

    fn screen(app: &mut App) -> (Vec<String>, Terminal<TestBackend>) {
        screen_of(app, 12)
    }

    fn screen_of(app: &mut App, height: u16) -> (Vec<String>, Terminal<TestBackend>) {
        let mut terminal = Terminal::new(TestBackend::new(60, height)).unwrap();
        terminal.draw(|f| ui::draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rows = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect();
        (rows, terminal)
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nanote-tui-app-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn typing_shows_up_with_line_numbers_and_the_status_line() {
        let mut a = app();
        type_text(&mut a, "hello\nworld");
        let (rows, _) = screen(&mut a);
        assert!(rows[1].starts_with("  1 hello"), "{:?}", rows[1]);
        assert!(rows[2].starts_with("  2 world"), "{:?}", rows[2]);
        // (the path gives way when the line is narrow)
        assert!(rows[11].contains("[modified]"), "{:?}", rows[11]);
        assert!(rows[11].contains("Ln 2/2, Col 6"), "{:?}", rows[11]);
        assert!(rows[0].starts_with(" File  Edit  Search  Format  Help"), "{:?}", rows[0]);
    }

    #[test]
    fn save_as_writes_the_file_and_picks_the_language() {
        let path = temp("out.rs");
        let _ = std::fs::remove_file(&path);
        let mut a = app();
        type_text(&mut a, "fn main() {}");
        ctrl(&mut a, 's'); // no name yet: asks for one
        assert!(a.prompt.as_ref().is_some_and(|p| p.kind == PromptKind::SaveAs));
        type_text(&mut a, &path.display().to_string());
        key(&mut a, KeyCode::Enter);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn main() {}");
        assert_eq!(a.doc().syntax(), "Rust");
        assert!(!a.doc().is_dirty());
        // saving again goes straight to the file
        type_text(&mut a, "x");
        ctrl(&mut a, 's');
        assert!(a.prompt.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn main() {}x");
    }

    #[test]
    fn code_is_coloured_on_screen() {
        let path = temp("colour.rs");
        std::fs::write(&path, "fn main() {\n    let x = 1;\n}\n").unwrap();
        let mut a = app();
        a.open(&path.display().to_string(), None);
        let (_, terminal) = screen(&mut a);
        let buffer = terminal.backend().buffer();
        // `fn` (x = 5, 6) and `main` (x = 8..) differ in colour, and neither is the default
        let kw = buffer[(5, 1)].fg;
        let name = buffer[(8, 1)].fg;
        assert!(matches!(kw, Color::Rgb(..)));
        assert_ne!(kw, name);
    }

    #[test]
    fn find_selects_the_match_and_f3_moves_on() {
        let mut a = app();
        type_text(&mut a, "one two\nthree two");
        ctrl(&mut a, 'f');
        type_text(&mut a, "two");
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.doc().buf.selection(), Some(((0, 4), (0, 7))));
        key(&mut a, KeyCode::F(3));
        assert_eq!(a.doc().buf.selection(), Some(((1, 6), (1, 9))));
        key(&mut a, KeyCode::F(3));
        assert_eq!(a.doc().buf.selection(), Some(((0, 4), (0, 7)))); // wrapped
        press(&mut a, KeyCode::F(3), KeyModifiers::SHIFT);
        assert_eq!(a.doc().buf.selection(), Some(((1, 6), (1, 9))));
        // not found is reported
        ctrl(&mut a, 'f');
        clear_prompt(&mut a);
        type_text(&mut a, "zzz");
        key(&mut a, KeyCode::Enter);
        assert!(a.status.contains("Not found"));
    }

    #[test]
    fn replace_all_via_the_two_prompts() {
        let mut a = app();
        type_text(&mut a, "a b a");
        ctrl(&mut a, 'r');
        type_text(&mut a, "a");
        key(&mut a, KeyCode::Enter);
        assert!(matches!(a.prompt.as_ref().unwrap().kind, PromptKind::ReplaceWith(_)));
        type_text(&mut a, "xx");
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.doc().buf.text(), "xx b xx");
        assert!(a.status.contains("Replaced 2"));
        ctrl(&mut a, 'z');
        assert_eq!(a.doc().buf.text(), "a b a");
    }

    #[test]
    fn goto_and_scrolling_keep_the_cursor_in_view() {
        let mut a = app();
        let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        a.docs[0].buf = crate::buffer::Buffer::new(&text);
        ctrl(&mut a, 'g');
        type_text(&mut a, "50:3");
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.doc().buf.cursor, (49, 2));
        let (rows, _) = screen(&mut a);
        assert!(rows.iter().any(|r| r.contains(" 50 line 50")), "{rows:#?}");
        assert!(a.doc().scroll_row <= 49 && 49 < a.doc().scroll_row + a.metrics().text_h);
        ctrl(&mut a, 'g');
        type_text(&mut a, "x");
        key(&mut a, KeyCode::Enter);
        assert!(a.status.contains("line number"));
    }

    #[test]
    fn quitting_with_unsaved_changes_asks_first() {
        let path = temp("quit.txt");
        std::fs::write(&path, "abc").unwrap();
        let mut a = app();
        a.open(&path.display().to_string(), None);
        ctrl(&mut a, 'q');
        assert!(a.quit); // nothing modified: leaves at once

        let mut a = app();
        a.open(&path.display().to_string(), None);
        type_text(&mut a, "X");
        ctrl(&mut a, 'q');
        assert!(!a.quit);
        assert!(a.prompt.is_some());
        key(&mut a, KeyCode::Esc);
        assert!(!a.quit && a.prompt.is_none());
        ctrl(&mut a, 'q');
        key(&mut a, KeyCode::Char('n'));
        assert!(a.quit);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abc"); // discarded

        let mut a = app();
        a.open(&path.display().to_string(), None);
        type_text(&mut a, "Y");
        ctrl(&mut a, 'q');
        key(&mut a, KeyCode::Char('y'));
        assert!(a.quit);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Yabc"); // saved
    }

    #[test]
    fn several_files_can_be_opened_and_switched() {
        let (p1, p2) = (temp("one.txt"), temp("two.txt"));
        std::fs::write(&p1, "1").unwrap();
        std::fs::write(&p2, "2").unwrap();
        let mut a = app();
        a.open(&p1.display().to_string(), None);
        a.open(&p2.display().to_string(), None);
        assert_eq!(a.docs.len(), 2); // the empty starting buffer was replaced
        assert_eq!(a.doc().name(), "two.txt");
        press(&mut a, KeyCode::Left, KeyModifiers::ALT);
        assert_eq!(a.doc().name(), "one.txt");
        // opening a file that is already open switches to it
        a.open(&p2.display().to_string(), None);
        assert_eq!((a.docs.len(), a.doc().name()), (2, "two.txt".to_owned()));
        let (rows, _) = screen(&mut a);
        assert!(rows[1].contains("one.txt") && rows[1].contains("two.txt"), "{:?}", rows[1]);
        ctrl(&mut a, 'w');
        assert_eq!(a.docs.len(), 1);
        ctrl(&mut a, 'w');
        assert!(a.quit);
    }

    #[test]
    fn clipboard_copy_cut_paste_and_cut_line() {
        let mut a = app();
        type_text(&mut a, "abc def");
        ctrl(&mut a, 'a');
        ctrl(&mut a, 'c');
        assert_eq!(a.osc52.take().as_deref(), Some("abc def"));
        key(&mut a, KeyCode::End);
        type_text(&mut a, "\n");
        ctrl(&mut a, 'v');
        assert_eq!(a.doc().buf.text(), "abc def\nabc def");
        ctrl(&mut a, 'k');
        assert_eq!(a.doc().buf.text(), "abc def");
        ctrl(&mut a, 'v');
        assert_eq!(a.doc().buf.text(), "abc defabc def\n");
    }

    #[test]
    fn mouse_click_and_drag_place_the_cursor_and_select() {
        let mut a = app();
        type_text(&mut a, "hello world\nsecond line");
        let click = |a: &mut App, kind, x, y| {
            a.mouse(MouseEvent {
                kind,
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            })
        };
        let g = a.metrics().gutter;
        click(&mut a, MouseEventKind::Down(MouseButton::Left), g + 6, 1);
        assert_eq!(a.doc().buf.cursor, (0, 6));
        click(&mut a, MouseEventKind::Drag(MouseButton::Left), g + 3, 2);
        assert_eq!(a.doc().buf.selected_text().unwrap(), "world\nsec");
        click(&mut a, MouseEventKind::Down(MouseButton::Left), g + 40, 2);
        assert_eq!(a.doc().buf.cursor, (1, 11)); // past the end: clamped
    }

    #[test]
    fn encoding_prompt_and_toggles_change_how_the_file_is_saved() {
        let path = temp("enc.txt");
        std::fs::write(&path, "가").unwrap();
        let mut a = app();
        a.open(&path.display().to_string(), None);
        assert_eq!(a.doc().encoding, "UTF-8");
        ctrl(&mut a, 'e');
        clear_prompt(&mut a);
        type_text(&mut a, "euc-kr");
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.doc().encoding, "EUC-KR");
        ctrl(&mut a, 's');
        assert_eq!(std::fs::read(&path).unwrap(), b"\xB0\xA1");
        // reopen as EUC-KR shows the same text
        a.run(Command::Reopen);
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.doc().buf.text(), "가");
        // an unknown name is refused
        ctrl(&mut a, 'e');
        clear_prompt(&mut a);
        type_text(&mut a, "x");
        key(&mut a, KeyCode::Enter);
        assert!(a.status.contains("Unknown encoding"));
    }

    #[test]
    fn long_lines_scroll_horizontally_and_wide_characters_take_two_cells() {
        let mut a = app();
        type_text(&mut a, &"가".repeat(50));
        let (rows, _) = screen(&mut a);
        assert!(a.doc().scroll_col > 0);
        assert!(rows[1].contains('가'));
        key(&mut a, KeyCode::Home);
        key(&mut a, KeyCode::Home);
        assert_eq!(a.doc().scroll_col, 0);
        // the cursor after three wide characters is six cells in
        key(&mut a, KeyCode::Right);
        key(&mut a, KeyCode::Right);
        key(&mut a, KeyCode::Right);
        assert_eq!(ui::cell_of(&a.doc().buf.lines[0], a.doc().buf.cursor.1), 6);
    }

    #[test]
    fn help_opens_and_any_key_closes_it() {
        let mut a = app();
        key(&mut a, KeyCode::F(1));
        assert!(a.show_help);
        let (rows, _) = screen(&mut a);
        assert!(rows.iter().any(|r| r.contains("nanote-tui - keys")));
        key(&mut a, KeyCode::Char('x'));
        assert!(!a.show_help);
        assert_eq!(a.doc().buf.text(), ""); // the key only closed the help
    }

    #[test]
    fn paste_goes_into_the_text_or_the_open_prompt() {
        let mut a = app();
        a.paste("a\r\nb");
        assert_eq!(a.doc().buf.text(), "a\nb");
        ctrl(&mut a, 'f');
        a.paste("needle\n");
        assert_eq!(a.prompt.as_ref().unwrap().text(), "needle");
        assert_eq!(a.doc().buf.text(), "a\nb");
    }

    #[test]
    fn multi_cursor_keys_and_drawing() {
        let mut a = app();
        type_text(&mut a, "foo bar foo");
        key(&mut a, KeyCode::Home);
        key(&mut a, KeyCode::Home);
        ctrl(&mut a, 'd'); // selects the word
        ctrl(&mut a, 'd'); // adds the next one
        assert_eq!(a.doc().buf.cursor_count(), 2);
        let (rows, _) = screen(&mut a);
        assert!(rows[11].contains("2 cursors"), "{:?}", rows[11]);
        type_text(&mut a, "Z");
        assert_eq!(a.doc().buf.text(), "Z bar Z");
        ctrl(&mut a, 'z');
        assert_eq!(a.doc().buf.text(), "foo bar foo");
        key(&mut a, KeyCode::Esc);
        assert_eq!(a.doc().buf.cursor_count(), 1);
        // Esc once more drops the selection
        assert!(a.doc().buf.selection().is_some());
        key(&mut a, KeyCode::Esc);
        assert!(a.doc().buf.selection().is_none());
    }

    #[test]
    fn extra_cursors_are_drawn_as_highlighted_cells() {
        let mut a = app();
        type_text(&mut a, "ab\ncd");
        let g = a.metrics().gutter;
        a.doc_mut().buf.move_to((1, 2), false); // after the last character
        a.doc_mut().buf.add_cursor((0, 1)); // the newest cursor is the primary one
        a.settle();
        let (_, terminal) = screen(&mut a);
        let buffer = terminal.backend().buffer();
        let reversed = |x: u16, y: u16| {
            buffer[(x, y)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        };
        // the primary cursor is the terminal's, the other is a highlighted cell
        assert!(reversed(g + 2, 2), "end-of-line cursor");
        assert!(!reversed(g + 1, 1));
        assert!(!reversed(g, 1));
    }

    #[test]
    fn alt_click_adds_a_cursor_and_alt_arrows_add_lines() {
        let mut a = app();
        type_text(&mut a, "abc\nabc\nabc");
        let g = a.metrics().gutter;
        let click = |a: &mut App, x, y, modifiers| {
            a.mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: y,
                modifiers,
            })
        };
        click(&mut a, g + 1, 1, KeyModifiers::NONE);
        click(&mut a, g + 2, 2, KeyModifiers::ALT);
        assert_eq!(a.doc().buf.cursor_count(), 2);
        type_text(&mut a, "X");
        assert_eq!(a.doc().buf.text(), "aXbc\nabXc\nabc");
        click(&mut a, g, 3, KeyModifiers::NONE); // a plain click goes back to one cursor
        assert_eq!(a.doc().buf.cursor_count(), 1);
        press(&mut a, KeyCode::Up, KeyModifiers::ALT | KeyModifiers::CONTROL);
        press(&mut a, KeyCode::Up, KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert_eq!(a.doc().buf.cursor_count(), 3);
    }

    #[test]
    fn ctrl_l_and_alt_shift_i() {
        let mut a = app();
        type_text(&mut a, "x one\nx two\nx three");
        a.doc_mut().buf.move_to((0, 0), false);
        ctrl(&mut a, 'l');
        assert_eq!(a.doc().buf.cursor_count(), 3);
        assert!(a.status.contains("3 occurrences"));
        key(&mut a, KeyCode::Esc);
        key(&mut a, KeyCode::Esc);
        a.doc_mut().buf.select_range((0, 0), (2, 0));
        press(&mut a, KeyCode::Char('I'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert_eq!(a.doc().buf.cursor_count(), 3);
        type_text(&mut a, ";");
        assert_eq!(a.doc().buf.text(), "x one;\nx two;\nx three;");
        // with nothing to select the command says so
        let mut a = app();
        press(&mut a, KeyCode::Char('I'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert!(a.status.contains("Select several lines"));
    }

    #[test]
    fn f10_opens_the_menu_and_the_keys_choose_an_entry() {
        let mut a = tall_app();
        key(&mut a, KeyCode::F(10));
        assert_eq!(a.menu.map(|m| m.menu), Some(0));
        let (rows, _) = screen_of(&mut a, 24);
        let text = rows.join("\n");
        assert!(text.contains("Save As..."), "{text}");
        assert!(text.contains("Ctrl+S"), "{text}");
        assert!(text.contains("Exit"), "{text}");
        key(&mut a, KeyCode::Down); // Open...
        key(&mut a, KeyCode::Enter);
        assert!(a.menu.is_none());
        assert!(a.prompt.as_ref().is_some_and(|p| p.kind == PromptKind::Open));
        key(&mut a, KeyCode::Esc);
        // Esc closes a menu without running anything
        key(&mut a, KeyCode::F(10));
        key(&mut a, KeyCode::Esc);
        assert!(a.menu.is_none() && a.prompt.is_none());
        // typing does not reach the text while a menu is open
        key(&mut a, KeyCode::F(10));
        key(&mut a, KeyCode::Char('x'));
        key(&mut a, KeyCode::Esc);
        assert_eq!(a.doc().buf.text(), "");
    }

    #[test]
    fn alt_letters_open_menus_and_arrows_move_between_them() {
        let mut a = tall_app();
        press(&mut a, KeyCode::Char('e'), KeyModifiers::ALT);
        assert_eq!(a.menu.map(|m| m.menu), Some(1)); // Edit
        let (rows, _) = screen_of(&mut a, 24);
        assert!(rows.join("\n").contains("Select All Occurrences"));
        key(&mut a, KeyCode::Right);
        assert_eq!(a.menu.map(|m| m.menu), Some(2)); // Search
        key(&mut a, KeyCode::Left);
        key(&mut a, KeyCode::Left);
        assert_eq!(a.menu.map(|m| m.menu), Some(0));
        press(&mut a, KeyCode::Char('o'), KeyModifiers::ALT);
        assert_eq!(a.menu.map(|m| m.menu), Some(3)); // Format
        press(&mut a, KeyCode::Char('o'), KeyModifiers::ALT); // again closes it
        assert!(a.menu.is_none());
        press(&mut a, KeyCode::Char('h'), KeyModifiers::ALT);
        key(&mut a, KeyCode::Enter); // Help > Keys
        assert!(a.show_help);
    }

    #[test]
    fn menu_entries_run_the_same_commands_as_the_shortcuts() {
        let mut a = app();
        type_text(&mut a, "abc");
        // Edit > Select All
        press(&mut a, KeyCode::Char('e'), KeyModifiers::ALT);
        for _ in 0..6 {
            key(&mut a, KeyCode::Down); // Undo > Redo > Cut > Copy > Paste > Cut Line > Select All
        }
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.doc().buf.selected_text().as_deref(), Some("abc"));
        // Edit > Undo
        key(&mut a, KeyCode::Esc);
        press(&mut a, KeyCode::Char('e'), KeyModifiers::ALT);
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.doc().buf.text(), "");
        // Format > BOM and Line Ending show their state and toggle it
        press(&mut a, KeyCode::Char('o'), KeyModifiers::ALT);
        let (rows, _) = screen(&mut a);
        let text = rows.join("\n");
        assert!(text.contains("BOM (off)") && text.contains("Line Ending (LF)"), "{text}");
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Down);
        key(&mut a, KeyCode::Down); // Line Ending
        key(&mut a, KeyCode::Enter);
        assert!(a.doc().crlf);
        press(&mut a, KeyCode::Char('o'), KeyModifiers::ALT);
        let (rows, _) = screen(&mut a);
        assert!(rows.join("\n").contains("Line Ending (CRLF)"));
    }

    #[test]
    fn the_mouse_opens_menus_runs_entries_and_closes_them() {
        let mut a = app();
        type_text(&mut a, "hello");
        let click = |a: &mut App, x, y| {
            a.mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            })
        };
        let (edit_x, _) = menu::title_span(1);
        click(&mut a, edit_x + 1, 0);
        assert_eq!(a.menu.map(|m| m.menu), Some(1));
        // another title switches, the same one closes
        let (search_x, _) = menu::title_span(2);
        click(&mut a, search_x + 1, 0);
        assert_eq!(a.menu.map(|m| m.menu), Some(2));
        click(&mut a, search_x + 1, 0);
        assert!(a.menu.is_none());
        // Edit > Select All (the 7th entry: rows 2.. of the drop-down)
        click(&mut a, edit_x + 1, 0);
        click(&mut a, edit_x + 3, 2 + 7);
        assert!(a.menu.is_none());
        assert_eq!(a.doc().buf.selected_text().as_deref(), Some("hello"));
        // a click on a separator or outside only closes the menu
        click(&mut a, edit_x + 1, 0);
        click(&mut a, edit_x + 3, 2 + 2); // the separator after Redo
        assert!(a.menu.is_none() && a.doc().buf.selected_text().is_some());
        click(&mut a, edit_x + 1, 0);
        click(&mut a, 40, 6);
        assert!(a.menu.is_none());
        // the click that closed the menu did not move the cursor
        assert!(a.doc().buf.selected_text().is_some());
    }

    #[test]
    fn tabs_appear_with_several_files_and_can_be_clicked() {
        let (p1, p2) = (temp("tab1.txt"), temp("tab2.txt"));
        std::fs::write(&p1, "first").unwrap();
        std::fs::write(&p2, "second").unwrap();
        let mut a = app();
        a.open(&p1.display().to_string(), None);
        assert_eq!(a.metrics().top, 1); // one file: no tab row
        let (rows, _) = screen(&mut a);
        assert!(rows[1].contains("first"), "{:?}", rows[1]);
        a.open(&p2.display().to_string(), None);
        assert_eq!(a.metrics().top, 2);
        let (rows, _) = screen(&mut a);
        assert!(rows[2].contains("second") && rows[1].contains("tab1.txt"));
        a.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(a.doc().name(), "tab1.txt");
        // text clicks use the lower first row now
        let g = a.metrics().gutter;
        a.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: g + 2,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(a.doc().buf.cursor, (0, 2));
        ctrl(&mut a, 'w');
        assert_eq!(a.metrics().top, 1); // back to one file: the tab row goes away
    }

    fn check_now(app: &mut App) {
        app.last_check = Instant::now() - Duration::from_secs(5);
        app.tick();
    }

    fn append_file(path: &PathBuf, text: &str) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn an_unmodified_file_is_reloaded_when_it_changes_outside() {
        let path = temp("watch-reload.txt");
        std::fs::write(&path, "one\n").unwrap();
        let mut a = app();
        a.open(&path.display().to_string(), None);
        append_file(&path, "two\n");
        check_now(&mut a);
        assert_eq!(a.doc().buf.text(), "one\ntwo\n");
        assert!(a.status.contains("Reloaded"), "{}", a.status);
        assert!(a.prompt.is_none() && !a.doc().is_dirty());
        // the same file open in another tab is checked too
        let other = temp("watch-reload2.txt");
        std::fs::write(&other, "x").unwrap();
        a.open(&other.display().to_string(), None);
        append_file(&path, "three\n");
        check_now(&mut a);
        a.run(Command::PreviousFile);
        assert_eq!(a.doc().buf.text(), "one\ntwo\nthree\n");
    }

    #[test]
    fn a_file_with_edits_asks_before_taking_the_outside_change() {
        let path = temp("watch-conflict.txt");
        std::fs::write(&path, "base\n").unwrap();
        let mut a = app();
        a.open(&path.display().to_string(), None);
        type_text(&mut a, "mine ");
        append_file(&path, "theirs\n");
        check_now(&mut a);
        assert!(matches!(a.prompt.as_ref().map(|p| &p.kind), Some(PromptKind::ConfirmReload(0))));
        assert_eq!(a.doc().buf.text(), "mine base\n");
        // a question on screen is not repeated or replaced
        check_now(&mut a);
        // n: keep mine, and do not ask again for the same change
        key(&mut a, KeyCode::Char('n'));
        assert!(a.prompt.is_none());
        check_now(&mut a);
        assert!(a.prompt.is_none());
        assert_eq!(a.doc().buf.text(), "mine base\n");
        // saving over the outside change asks first; n cancels
        ctrl(&mut a, 's');
        assert!(matches!(a.prompt.as_ref().map(|p| &p.kind), Some(PromptKind::ConfirmOverwrite(None))));
        key(&mut a, KeyCode::Char('n'));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "base\ntheirs\n");
        assert!(a.doc().is_dirty());
        // y overwrites
        ctrl(&mut a, 's');
        key(&mut a, KeyCode::Char('y'));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine base\n");
        assert!(!a.doc().is_dirty());
        // and our own write is not a change
        check_now(&mut a);
        assert!(a.prompt.is_none());
    }

    #[test]
    fn answering_yes_reloads_and_escape_keeps_my_version() {
        let path = temp("watch-reload-yes.txt");
        std::fs::write(&path, "base").unwrap();
        let mut a = app();
        a.open(&path.display().to_string(), None);
        type_text(&mut a, "mine ");
        append_file(&path, "!");
        check_now(&mut a);
        key(&mut a, KeyCode::Char('y'));
        assert_eq!(a.doc().buf.text(), "base!");
        assert!(!a.doc().is_dirty());

        type_text(&mut a, "again ");
        append_file(&path, "?");
        check_now(&mut a);
        key(&mut a, KeyCode::Esc);
        assert!(a.doc().buf.text().contains("again "));
        check_now(&mut a);
        assert!(a.prompt.is_none()); // Esc counts as "keep mine"
    }

    #[test]
    fn quitting_does_not_overwrite_a_file_that_changed_outside() {
        let path = temp("watch-quit.txt");
        std::fs::write(&path, "base").unwrap();
        let mut a = app();
        a.open(&path.display().to_string(), None);
        type_text(&mut a, "mine ");
        append_file(&path, " theirs");
        ctrl(&mut a, 'q');
        key(&mut a, KeyCode::Char('y')); // save before quitting
        assert!(!a.quit);
        assert!(a.status.contains("changed on disk"), "{}", a.status);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "base theirs");
    }

    #[test]
    fn save_as_over_the_same_changed_file_asks() {
        let path = temp("watch-saveas.txt");
        std::fs::write(&path, "base").unwrap();
        let mut a = app();
        a.open(&path.display().to_string(), None);
        type_text(&mut a, "mine ");
        append_file(&path, "!");
        key(&mut a, KeyCode::F(2));
        key(&mut a, KeyCode::Enter); // the prompt starts with the file's own path
        assert!(matches!(
            a.prompt.as_ref().map(|p| &p.kind),
            Some(PromptKind::ConfirmOverwrite(Some(_)))
        ));
        key(&mut a, KeyCode::Char('y'));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine base");
    }

    #[test]
    fn follow_adds_appended_text_and_stays_at_the_end() {
        let path = temp("watch-follow.log");
        std::fs::write(&path, "start\n").unwrap();
        let mut a = app();
        a.open(&path.display().to_string(), None);
        ctrl(&mut a, 't');
        assert!(a.doc().follow);
        assert_eq!(a.doc().buf.cursor, (1, 0)); // at the end
        append_file(&path, "a\nb\n");
        check_now(&mut a);
        assert_eq!(a.doc().buf.text(), "start\na\nb\n");
        assert_eq!(a.doc().buf.cursor, (3, 0));
        assert!(!a.doc().is_dirty());
        // a long append scrolls so that the end is visible
        let many: String = (0..50).map(|i| format!("line {i}\n")).collect();
        append_file(&path, &many);
        check_now(&mut a);
        let (rows, _) = screen(&mut a);
        assert!(rows.iter().any(|r| r.contains("line 49")), "{rows:#?}");
        // Ctrl+T again turns it off
        ctrl(&mut a, 't');
        assert!(!a.doc().follow);
    }

    #[test]
    fn the_follow_switch_is_on_the_status_line_and_can_be_clicked() {
        let path = temp("watch-click.log");
        std::fs::write(&path, "x\n").unwrap();
        let mut a = app();
        a.open(&path.display().to_string(), None);
        let (rows, _) = screen(&mut a);
        assert!(rows[11].contains("[ ] Follow"), "{:?}", rows[11]);
        let (from, to) = a.follow_hit.expect("the switch has a place");
        assert_eq!(&rows[11].chars().skip(from as usize).take((to - from) as usize).collect::<String>(), "[ ] Follow");
        let click = |a: &mut App, x| {
            a.mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: 11,
                modifiers: KeyModifiers::NONE,
            })
        };
        click(&mut a, from + 2);
        assert!(a.doc().follow);
        let (rows, _) = screen(&mut a);
        assert!(rows[11].contains("[x] Follow"), "{:?}", rows[11]);
        click(&mut a, 0); // elsewhere on the line does nothing
        assert!(a.doc().follow);
        let (from, _) = a.follow_hit.unwrap();
        click(&mut a, from);
        assert!(!a.doc().follow);
    }

    #[test]
    fn follow_needs_a_saved_unmodified_file() {
        let mut a = app();
        ctrl(&mut a, 't');
        assert!(!a.doc().follow);
        assert!(a.status.contains("Save the file first"), "{}", a.status);

        let path = temp("watch-follow-dirty.log");
        std::fs::write(&path, "x").unwrap();
        a.open(&path.display().to_string(), None);
        type_text(&mut a, "edit");
        ctrl(&mut a, 't');
        assert!(!a.doc().follow);
        assert!(a.status.contains("Save or reload"), "{}", a.status);
    }

    #[test]
    fn follow_is_in_the_format_menu_with_its_state() {
        let path = temp("watch-menu.log");
        std::fs::write(&path, "x").unwrap();
        let mut a = tall_app();
        a.open(&path.display().to_string(), None);
        press(&mut a, KeyCode::Char('o'), KeyModifiers::ALT);
        let (rows, _) = screen_of(&mut a, 24);
        let text = rows.join("\n");
        assert!(text.contains("Follow File Changes (off)") && text.contains("Ctrl+T"), "{text}");
        for _ in 0..5 {
            key(&mut a, KeyCode::Down); // Encoding > Reopen > BOM > Line Ending > Syntax > Follow
        }
        key(&mut a, KeyCode::Enter);
        assert!(a.doc().follow);
    }
}
