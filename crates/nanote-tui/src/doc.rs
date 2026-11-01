//! An open file: its text plus the properties that decide how it is saved.

use std::{
    io,
    path::{Path, PathBuf},
};

use crate::{
    buffer::Buffer,
    encoding,
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
}

impl Doc {
    pub fn untitled(hl: &Highlighter) -> Self {
        Self::from_text(String::new(), None, "UTF-8".into(), false, false, hl)
    }

    fn from_text(
        text: String,
        path: Option<PathBuf>,
        encoding: String,
        bom: bool,
        crlf: bool,
        hl: &Highlighter,
    ) -> Self {
        let syntax = hl.detect(path.as_deref(), text.lines().next().unwrap_or_default());
        Self {
            buf: Buffer::new(&text),
            path,
            encoding,
            bom,
            crlf,
            meta_dirty: false,
            hl: Highlights::new(&syntax),
            scroll_row: 0,
            scroll_col: 0,
        }
    }

    /// Open a file. A path that does not exist yet gives an empty document that will create it.
    pub fn open(path: &Path, forced: Option<&str>, hl: &Highlighter) -> io::Result<Self> {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        match encoding::read_file(&path, forced) {
            Ok(l) => Ok(Self::from_text(l.text, Some(path), l.encoding, l.bom, l.crlf, hl)),
            Err(e) if e.kind() == io::ErrorKind::NotFound && forced.is_none() => Ok(Self::from_text(
                String::new(),
                Some(path),
                "UTF-8".into(),
                false,
                false,
                hl,
            )),
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

    /// Write to `path` (or the document's own) in the document's encoding.
    pub fn save(&mut self, path: Option<&Path>) -> Result<(), String> {
        let target = match path {
            Some(p) => p.to_owned(),
            None => self.path.clone().ok_or("no file name")?,
        };
        let bytes = encoding::encode_text(&self.buf.text(), &self.encoding, self.bom, self.crlf)?;
        std::fs::write(&target, bytes).map_err(|e| format!("{}: {e}", target.display()))?;
        self.path = Some(target);
        self.buf.mark_saved();
        self.meta_dirty = false;
        Ok(())
    }

    /// Display name of the language.
    pub fn syntax(&self) -> &str {
        self.hl.syntax()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nanote-tui-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn round_trips_encoding_bom_and_line_endings() {
        let hl = Highlighter::new("base16-ocean.dark");
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
        let hl = Highlighter::new("base16-ocean.dark");
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
        let hl = Highlighter::new("base16-ocean.dark");
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
        let hl = Highlighter::new("base16-ocean.dark");
        let path = temp("unmappable.txt");
        std::fs::write(&path, "abc").unwrap();
        let mut doc = Doc::open(&path, None, &hl).unwrap();
        doc.set_encoding("windows-1252");
        doc.buf.insert_str("가");
        assert!(doc.save(None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"abc");
        assert!(doc.is_dirty());
    }
}
