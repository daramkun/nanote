//! An open file: its text plus the properties that decide how it is saved.

use std::{
    io,
    path::{Path, PathBuf},
};

use crate::{
    buffer::Buffer,
    encoding::{self, Loaded},
    filewatch::{self, Change, Stamp, Tail},
    highlight::{Highlighter, Highlights},
};

pub struct Doc {
    pub buf: Buffer,
    pub path: Option<PathBuf>,
    pub encoding: String,
    pub bom: bool,
    pub crlf: bool,
    /// Encoding, BOM or line ending changed since the last save.
    meta_dirty: bool,
    pub hl: Highlights,
    pub scroll_row: usize,
    /// Horizontal scroll in screen cells.
    pub scroll_col: usize,
    /// What the file looked like when it was read or written here.
    stamp: Option<Stamp>,
    /// How far the file was read (for following a growing file).
    tail: Tail,
    /// Keep up with text appended to the file, and stay at its end.
    pub follow: bool,
    /// The change of the file that the user was already asked about.
    pub asked: Option<Stamp>,
}

/// What a look at the file on disk found.
#[derive(Debug, PartialEq, Eq)]
pub enum Disk {
    /// Nothing new (or the file is gone, or the user already decided about this change).
    Quiet,
    /// The file changed while the document has unsaved edits: the user has to decide.
    Conflict,
    /// Unmodified document: the new content was read in.
    Reloaded,
    /// Unmodified document that follows the file: what was appended was added at the end.
    Appended,
    /// Reading the file failed.
    Failed(String),
}

impl Doc {
    pub fn untitled(hl: &Highlighter) -> Self {
        Self::from_loaded(Loaded::empty(), None, hl)
    }

    fn from_loaded(loaded: Loaded, path: Option<PathBuf>, hl: &Highlighter) -> Self {
        let syntax = hl.detect(path.as_deref(), loaded.text.lines().next().unwrap_or_default());
        Self {
            buf: Buffer::new(&loaded.text),
            path,
            encoding: loaded.encoding,
            bom: loaded.bom,
            crlf: loaded.crlf,
            meta_dirty: false,
            hl: Highlights::new(&syntax),
            scroll_row: 0,
            scroll_col: 0,
            stamp: loaded.stamp,
            tail: loaded.tail,
            follow: false,
            asked: None,
        }
    }

    /// Open a file. A path that does not exist yet gives an empty document that will create it.
    pub fn open(path: &Path, forced: Option<&str>, hl: &Highlighter) -> io::Result<Self> {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        match encoding::read_file(&path, forced) {
            Ok(l) => Ok(Self::from_loaded(l, Some(path), hl)),
            Err(e) if e.kind() == io::ErrorKind::NotFound && forced.is_none() => {
                Ok(Self::from_loaded(Loaded::empty(), Some(path), hl))
            }
            Err(e) => Err(e),
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.buf.is_dirty() || self.meta_dirty
    }

    pub fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(|| "Untitled".to_owned(), |n| n.to_string_lossy().into_owned())
    }

    pub fn set_encoding(&mut self, encoding: &str) {
        self.encoding = encoding.to_owned();
        self.meta_dirty = true;
    }

    pub fn toggle_bom(&mut self) {
        self.bom = !self.bom;
        self.meta_dirty = true;
    }

    pub fn toggle_crlf(&mut self) {
        self.crlf = !self.crlf;
        self.meta_dirty = true;
    }

    /// Has the file been changed (or created) outside the editor since it was read or written?
    pub fn disk_differs(&self) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        let now = filewatch::stamp(path);
        match &self.stamp {
            Some(seen) => !matches!(
                filewatch::classify(seen, now.as_ref()),
                Change::Same | Change::Gone
            ),
            None => now.is_some(),
        }
    }

    /// Write to `path` (or the document's own) in the document's encoding.
    pub fn save(&mut self, path: Option<&Path>) -> Result<(), String> {
        let target = match path {
            Some(p) => p.to_owned(),
            None => self.path.clone().ok_or("no file name")?,
        };
        let bytes = encoding::encode_text(&self.buf.text(), &self.encoding, self.bom, self.crlf)?;
        std::fs::write(&target, &bytes).map_err(|e| format!("{}: {e}", target.display()))?;
        self.stamp = filewatch::stamp(&target);
        self.tail = Tail::of(&bytes);
        self.asked = None;
        self.path = Some(target);
        self.buf.mark_saved();
        self.meta_dirty = false;
        Ok(())
    }

    /// Replace the text by what is on disk now.
    pub fn reload(&mut self) -> Result<(), String> {
        let path = self.path.clone().ok_or("no file name")?;
        let loaded = encoding::reread_file(&path, &self.encoding).map_err(|e| e.to_string())?;
        self.encoding = loaded.encoding;
        self.bom = loaded.bom;
        self.crlf = loaded.crlf;
        self.stamp = loaded.stamp;
        self.tail = loaded.tail;
        self.asked = None;
        self.meta_dirty = false;
        if self.buf.text() == loaded.text {
            self.buf.mark_saved();
        } else {
            self.buf.reload(&loaded.text);
        }
        Ok(())
    }

    /// Add what was appended to the file to the end of the text. False when the file is not the
    /// old one with more at the end: it has to be read again.
    fn append(&mut self) -> bool {
        let Some(path) = self.path.clone() else {
            return false;
        };
        let Ok(Some((text, tail))) = filewatch::read_appended(&path, &self.tail) else {
            return false;
        };
        self.stamp = filewatch::stamp(&path);
        self.tail = tail;
        self.asked = None;
        if !text.is_empty() {
            self.buf.append_saved(&text);
        }
        true
    }

    /// Look at the file on disk and take over what changed, as far as that is safe.
    pub fn check_disk(&mut self) -> Disk {
        let Some(path) = &self.path else {
            return Disk::Quiet;
        };
        let now = filewatch::stamp(path);
        let change = match &self.stamp {
            Some(seen) => filewatch::classify(seen, now.as_ref()),
            None if now.is_some() => Change::Changed,
            None => Change::Same,
        };
        if matches!(change, Change::Same | Change::Gone) || self.asked == now {
            return Disk::Quiet;
        }
        if self.is_dirty() {
            return Disk::Conflict;
        }
        if change == Change::Grew && self.follow && self.encoding == "UTF-8" && self.append() {
            return Disk::Appended;
        }
        match self.reload() {
            Ok(()) => Disk::Reloaded,
            Err(e) => Disk::Failed(e),
        }
    }

    /// The user was asked about the file's present state and chose to keep their version.
    pub fn keep_mine(&mut self) {
        if let Some(path) = &self.path {
            self.asked = filewatch::stamp(path);
        }
    }

    /// Display name of the language.
    pub fn syntax(&self) -> &str {
        self.hl.syntax()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nanote-tui-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn hl() -> Highlighter {
        Highlighter::new("base16-ocean.dark")
    }

    /// A write is only noticed when the stamp differs: make sure it does.
    fn append_to(path: &Path, text: &str) {
        let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn round_trips_encoding_bom_and_line_endings() {
        let hl = hl();
        let path = temp("rt.txt");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend("가\r\nb".encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&path, &bytes).unwrap();

        let mut doc = Doc::open(&path, None, &hl).unwrap();
        assert_eq!(doc.encoding, "UTF-16LE");
        assert!(doc.bom && doc.crlf);
        assert_eq!(doc.buf.text(), "가\nb");
        doc.buf.insert_char('x');
        assert!(doc.is_dirty());
        doc.save(None).unwrap();
        assert!(!doc.is_dirty());
        let again = std::fs::read(&path).unwrap();
        assert!(again.starts_with(&[0xFF, 0xFE]));
        let units: Vec<u16> = again[2..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(String::from_utf16(&units).unwrap(), "x가\r\nb");
    }

    #[test]
    fn a_missing_file_opens_empty_and_is_created_on_save() {
        let hl = hl();
        let path = temp("new-file.rs");
        let _ = std::fs::remove_file(&path);
        let mut doc = Doc::open(&path, None, &hl).unwrap();
        assert_eq!(doc.buf.text(), "");
        assert_eq!(doc.syntax(), "Rust");
        doc.buf.insert_str("fn main() {}");
        doc.save(None).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn main() {}");
    }

    #[test]
    fn changing_the_encoding_or_line_ending_makes_the_document_dirty() {
        let hl = hl();
        let path = temp("meta.txt");
        std::fs::write(&path, "abc\n").unwrap();
        let mut doc = Doc::open(&path, None, &hl).unwrap();
        assert!(!doc.is_dirty());
        doc.toggle_crlf();
        assert!(doc.is_dirty());
        doc.save(None).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"abc\r\n");
        doc.set_encoding("EUC-KR");
        doc.buf.insert_str("가");
        doc.save(None).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"\xB0\xA1abc\r\n");
    }

    #[test]
    fn unmappable_characters_do_not_overwrite_the_file() {
        let hl = hl();
        let path = temp("unmappable.txt");
        std::fs::write(&path, "abc").unwrap();
        let mut doc = Doc::open(&path, None, &hl).unwrap();
        doc.set_encoding("windows-1252");
        doc.buf.insert_str("가");
        assert!(doc.save(None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"abc");
        assert!(doc.is_dirty());
    }

    #[test]
    fn an_unmodified_document_reads_outside_changes_by_itself() {
        let hl = hl();
        let path = temp("outside.txt");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let mut doc = Doc::open(&path, None, &hl).unwrap();
        assert_eq!(doc.check_disk(), Disk::Quiet);
        doc.buf.move_to((1, 2), false);
        append_to(&path, "three\n");
        assert_eq!(doc.check_disk(), Disk::Reloaded);
        assert_eq!(doc.buf.text(), "one\ntwo\nthree\n");
        assert_eq!(doc.buf.cursor, (1, 2)); // the cursor stayed
        assert!(!doc.is_dirty());
        assert_eq!(doc.check_disk(), Disk::Quiet);
        // a rewrite that makes the file shorter is read too
        std::fs::write(&path, "x").unwrap();
        assert_eq!(doc.check_disk(), Disk::Reloaded);
        assert_eq!(doc.buf.text(), "x");
        assert_eq!(doc.buf.cursor, (0, 1)); // clamped
        // a file that was deleted is left alone
        std::fs::remove_file(&path).unwrap();
        assert_eq!(doc.check_disk(), Disk::Quiet);
        assert_eq!(doc.buf.text(), "x");
    }

    #[test]
    fn a_document_with_edits_is_not_overwritten_and_asks_once_per_change() {
        let hl = hl();
        let path = temp("conflict.txt");
        std::fs::write(&path, "base\n").unwrap();
        let mut doc = Doc::open(&path, None, &hl).unwrap();
        doc.buf.insert_str("mine ");
        append_to(&path, "theirs\n");
        assert_eq!(doc.check_disk(), Disk::Conflict);
        assert_eq!(doc.buf.text(), "mine base\n"); // untouched
        doc.keep_mine();
        assert_eq!(doc.check_disk(), Disk::Quiet); // not asked again for the same change
        assert!(doc.disk_differs()); // but saving still has to ask
        append_to(&path, "more\n");
        assert_eq!(doc.check_disk(), Disk::Conflict); // a newer change asks again
        // the user chooses to reload
        doc.reload().unwrap();
        assert_eq!(doc.buf.text(), "base\ntheirs\nmore\n");
        assert!(!doc.is_dirty() && !doc.disk_differs());
        // saving records the new state, so our own write is not "a change"
        doc.buf.insert_str("x");
        doc.save(None).unwrap();
        assert_eq!(doc.check_disk(), Disk::Quiet);
        assert!(!doc.disk_differs());
    }

    #[test]
    fn a_file_created_outside_is_a_conflict_for_a_new_document() {
        let hl = hl();
        let path = temp("created-meanwhile.txt");
        let _ = std::fs::remove_file(&path);
        let mut doc = Doc::open(&path, None, &hl).unwrap();
        assert!(!doc.disk_differs());
        std::fs::write(&path, "someone else").unwrap();
        assert!(doc.disk_differs());
        doc.save(None).unwrap(); // overwriting is the caller's decision
        assert!(!doc.disk_differs());
    }

    #[test]
    fn following_adds_the_appended_text_and_keeps_the_cursor_at_the_end() {
        let hl = hl();
        let path = temp("follow.log");
        std::fs::write(&path, "start\n").unwrap();
        let mut doc = Doc::open(&path, None, &hl).unwrap();
        doc.follow = true;
        doc.buf.doc_end(false);
        append_to(&path, "a\r\nb\n");
        assert_eq!(doc.check_disk(), Disk::Appended);
        assert_eq!(doc.buf.text(), "start\na\nb\n");
        assert_eq!(doc.buf.cursor, (3, 0)); // moved with the end
        assert!(!doc.is_dirty());
        // a cursor that is somewhere else stays there
        doc.buf.move_to((0, 2), false);
        append_to(&path, "c");
        assert_eq!(doc.check_disk(), Disk::Appended);
        assert_eq!(doc.buf.text(), "start\na\nb\nc");
        assert_eq!(doc.buf.cursor, (0, 2));
        // a file that was replaced is read again from the start, not appended to
        std::fs::write(&path, "a completely different and longer content than before\n").unwrap();
        assert_eq!(doc.check_disk(), Disk::Reloaded);
        assert_eq!(doc.buf.text(), "a completely different and longer content than before\n");
    }

    #[test]
    fn without_follow_appended_text_is_a_plain_reload_and_undo_history_is_dropped() {
        let hl = hl();
        let path = temp("nofollow.log");
        std::fs::write(&path, "x\n").unwrap();
        let mut doc = Doc::open(&path, None, &hl).unwrap();
        doc.buf.insert_char('y');
        doc.save(None).unwrap();
        append_to(&path, "z\n");
        assert_eq!(doc.check_disk(), Disk::Reloaded);
        assert!(!doc.buf.undo()); // the baseline is the file on disk now
        assert_eq!(doc.buf.text(), "yx\nz\n");
    }

    #[test]
    fn a_following_document_with_edits_does_not_append() {
        let hl = hl();
        let path = temp("follow-dirty.log");
        std::fs::write(&path, "a\n").unwrap();
        let mut doc = Doc::open(&path, None, &hl).unwrap();
        doc.follow = true;
        doc.buf.insert_char('!');
        append_to(&path, "b\n");
        assert_eq!(doc.check_disk(), Disk::Conflict);
        assert_eq!(doc.buf.text(), "!a\n");
    }
}
