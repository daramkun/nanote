//! Everything the editor can do as a named command, the menu bar that lists them, and the key
//! shortcuts that run them.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    New,
    Open,
    Save,
    SaveAs,
    Close,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    CutLine,
    SelectAll,
    NextOccurrence,
    AllOccurrences,
    LineEnds,
    CursorAbove,
    CursorBelow,
    Find,
    FindNext,
    FindPrevious,
    Replace,
    Goto,
    Encoding,
    Reopen,
    ToggleBom,
    ToggleLineEnding,
    Syntax,
    Follow,
    Keys,
    PreviousFile,
    NextFile,
}

pub enum Entry {
    Item {
        label: &'static str,
        keys: &'static str,
        command: Command,
    },
    Separator,
}

const fn item(label: &'static str, keys: &'static str, command: Command) -> Entry {
    Entry::Item {
        label,
        keys,
        command,
    }
}

pub struct Menu {
    pub title: &'static str,
    /// Alt + this letter opens the menu; it is underlined in the bar.
    pub mnemonic: char,
    pub entries: &'static [Entry],
}

pub const MENUS: &[Menu] = &[
    Menu {
        title: "File",
        mnemonic: 'f',
        entries: &[
            item("New", "Ctrl+N", Command::New),
            item("Open...", "Ctrl+P", Command::Open),
            item("Save", "Ctrl+S", Command::Save),
            item("Save As...", "F2", Command::SaveAs),
            item("Close", "Ctrl+W", Command::Close),
            Entry::Separator,
            item("Previous File", "Alt+Left", Command::PreviousFile),
            item("Next File", "Alt+Right", Command::NextFile),
            Entry::Separator,
            item("Exit", "Ctrl+Q", Command::Quit),
        ],
    },
    Menu {
        title: "Edit",
        mnemonic: 'e',
        entries: &[
            item("Undo", "Ctrl+Z", Command::Undo),
            item("Redo", "Ctrl+Y", Command::Redo),
            Entry::Separator,
            item("Cut", "Ctrl+X", Command::Cut),
            item("Copy", "Ctrl+C", Command::Copy),
            item("Paste", "Ctrl+V", Command::Paste),
            item("Cut Line", "Ctrl+K", Command::CutLine),
            item("Select All", "Ctrl+A", Command::SelectAll),
            Entry::Separator,
            item("Add Next Occurrence", "Ctrl+D", Command::NextOccurrence),
            item("Select All Occurrences", "Ctrl+L", Command::AllOccurrences),
            item("Cursors at Line Ends", "Alt+Shift+I", Command::LineEnds),
            item("Add Cursor Above", "Ctrl+Alt+Up", Command::CursorAbove),
            item("Add Cursor Below", "Ctrl+Alt+Down", Command::CursorBelow),
        ],
    },
    Menu {
        title: "Search",
        mnemonic: 's',
        entries: &[
            item("Find...", "Ctrl+F", Command::Find),
            item("Find Next", "F3", Command::FindNext),
            item("Find Previous", "Shift+F3", Command::FindPrevious),
            item("Replace...", "Ctrl+R", Command::Replace),
            Entry::Separator,
            item("Go to Line...", "Ctrl+G", Command::Goto),
        ],
    },
    Menu {
        title: "Format",
        mnemonic: 'o',
        entries: &[
            item("Encoding...", "Ctrl+E", Command::Encoding),
            item("Reopen with Encoding...", "", Command::Reopen),
            item("BOM", "", Command::ToggleBom),
            item("Line Ending", "", Command::ToggleLineEnding),
            item("Syntax...", "", Command::Syntax),
            Entry::Separator,
            item("Follow File Changes", "Ctrl+T", Command::Follow),
        ],
    },
    Menu {
        title: "Help",
        mnemonic: 'h',
        entries: &[item("Keys", "F1", Command::Keys)],
    },
];

/// The open menu and the highlighted entry in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuState {
    pub menu: usize,
    pub entry: usize,
}

impl MenuState {
    pub fn open(menu: usize) -> Self {
        let entry = MENUS[menu]
            .entries
            .iter()
            .position(|e| matches!(e, Entry::Item { .. }))
            .unwrap_or(0);
        Self { menu, entry }
    }

    fn entries(&self) -> &'static [Entry] {
        MENUS[self.menu].entries
    }

    /// Move the highlight one entry up (`-1`) or down (`1`), skipping separators and wrapping.
    pub fn step(&mut self, delta: isize) {
        let n = self.entries().len() as isize;
        let mut i = self.entry as isize;
        loop {
            i = (i + delta.signum()).rem_euclid(n);
            if matches!(self.entries()[i as usize], Entry::Item { .. }) {
                break;
            }
        }
        self.entry = i as usize;
    }

    pub fn command(&self) -> Option<Command> {
        match self.entries().get(self.entry)? {
            Entry::Item { command, .. } => Some(*command),
            Entry::Separator => None,
        }
    }

    pub fn switch(&mut self, delta: isize) {
        let n = MENUS.len() as isize;
        *self = Self::open((self.menu as isize + delta).rem_euclid(n) as usize);
    }
}

/// Where the title of menu `i` is in the bar: `(first column, width)`, spaces around it included.
pub fn title_span(i: usize) -> (u16, u16) {
    let mut x = 0u16;
    for (j, m) in MENUS.iter().enumerate() {
        let w = m.title.chars().count() as u16 + 2;
        if j == i {
            return (x, w);
        }
        x += w;
    }
    (x, 0)
}

/// The menu whose title is at column `x` of the bar.
pub fn title_at(x: u16) -> Option<usize> {
    (0..MENUS.len()).find(|&i| {
        let (start, w) = title_span(i);
        x >= start && x < start + w
    })
}

/// Width of the drop-down of menu `i`, borders included.
pub fn dropdown_width(i: usize) -> u16 {
    let inner = MENUS[i]
        .entries
        .iter()
        .map(|e| match e {
            // label (some show their state, " (CRLF)" at most), a gap, the shortcut, and a
            // space on each side
            Entry::Item {
                label,
                keys,
                command,
            } => {
                let state = match command {
                    Command::ToggleBom | Command::ToggleLineEnding | Command::Follow => 7,
                    _ => 0,
                };
                label.chars().count() + state + 3 + keys.chars().count() + 2
            }
            Entry::Separator => 0,
        })
        .max()
        .unwrap_or(10);
    inner as u16 + 2
}

/// The command a key combination runs directly.
pub fn shortcut(key: KeyEvent) -> Option<Command> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    Some(match key.code {
        KeyCode::Char(c) if ctrl && !alt => match c.to_ascii_lowercase() {
            's' => Command::Save,
            'q' => Command::Quit,
            'f' => Command::Find,
            'r' => Command::Replace,
            'g' => Command::Goto,
            'p' => Command::Open,
            'n' => Command::New,
            'w' => Command::Close,
            'z' => Command::Undo,
            'y' => Command::Redo,
            'a' => Command::SelectAll,
            'c' => Command::Copy,
            'x' => Command::Cut,
            'k' => Command::CutLine,
            'v' => Command::Paste,
            'd' => Command::NextOccurrence,
            'l' => Command::AllOccurrences,
            'e' => Command::Encoding,
            't' => Command::Follow,
            _ => return None,
        },
        KeyCode::Char('i' | 'I') if alt && shift && !ctrl => Command::LineEnds,
        KeyCode::F(1) => Command::Keys,
        KeyCode::F(2) => Command::SaveAs,
        KeyCode::F(3) if shift => Command::FindPrevious,
        KeyCode::F(3) => Command::FindNext,
        KeyCode::Up if alt && (ctrl || shift) => Command::CursorAbove,
        KeyCode::Down if alt && (ctrl || shift) => Command::CursorBelow,
        KeyCode::Left if alt => Command::PreviousFile,
        KeyCode::Right if alt => Command::NextFile,
        KeyCode::PageUp if ctrl => Command::PreviousFile,
        KeyCode::PageDown if ctrl => Command::NextFile,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_laid_out_without_gaps_and_found_by_column() {
        assert_eq!(title_span(0), (0, 6)); // " File "
        assert_eq!(title_span(1), (6, 6)); // " Edit "
        assert_eq!(title_at(0), Some(0));
        assert_eq!(title_at(5), Some(0));
        assert_eq!(title_at(6), Some(1));
        let (last, w) = title_span(MENUS.len() - 1);
        assert_eq!(title_at(last + w), None);
    }

    #[test]
    fn stepping_skips_separators_and_wraps() {
        let mut s = MenuState::open(0);
        assert_eq!(s.command(), Some(Command::New));
        s.step(-1); // wraps to the last entry
        assert_eq!(s.command(), Some(Command::Quit));
        s.step(1);
        assert_eq!(s.command(), Some(Command::New));
        // Close is followed by a separator: the next step lands on Previous File
        for _ in 0..4 {
            s.step(1);
        }
        assert_eq!(s.command(), Some(Command::Close));
        s.step(1);
        assert_eq!(s.command(), Some(Command::PreviousFile));
        s.step(-1);
        assert_eq!(s.command(), Some(Command::Close));
    }

    #[test]
    fn every_menu_opens_on_an_item_and_switching_wraps() {
        for i in 0..MENUS.len() {
            assert!(MenuState::open(i).command().is_some(), "{}", MENUS[i].title);
        }
        let mut s = MenuState::open(MENUS.len() - 1);
        s.switch(1);
        assert_eq!(s.menu, 0);
        s.switch(-1);
        assert_eq!(s.menu, MENUS.len() - 1);
    }

    #[test]
    fn every_command_with_a_menu_shortcut_has_that_shortcut() {
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(shortcut(key(KeyCode::Char('s'), ctrl)), Some(Command::Save));
        assert_eq!(shortcut(key(KeyCode::Char('S'), ctrl)), Some(Command::Save));
        assert_eq!(shortcut(key(KeyCode::F(2), KeyModifiers::NONE)), Some(Command::SaveAs));
        assert_eq!(
            shortcut(key(KeyCode::F(3), KeyModifiers::SHIFT)),
            Some(Command::FindPrevious)
        );
        assert_eq!(
            shortcut(key(KeyCode::Up, KeyModifiers::ALT | KeyModifiers::CONTROL)),
            Some(Command::CursorAbove)
        );
        assert_eq!(
            shortcut(key(KeyCode::Char('I'), KeyModifiers::ALT | KeyModifiers::SHIFT)),
            Some(Command::LineEnds)
        );
        assert_eq!(shortcut(key(KeyCode::Char('x'), KeyModifiers::NONE)), None);
        // Alt+letter opens menus, it is not a command
        assert_eq!(shortcut(key(KeyCode::Char('f'), KeyModifiers::ALT)), None);
    }

    #[test]
    fn dropdowns_are_wide_enough_for_label_and_shortcut() {
        for (i, m) in MENUS.iter().enumerate() {
            for e in m.entries {
                if let Entry::Item { label, keys, .. } = e {
                    assert!(
                        dropdown_width(i) as usize >= label.len() + keys.len() + 5,
                        "{label}"
                    );
                }
            }
        }
    }
}
