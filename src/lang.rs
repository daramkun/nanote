//! Language detection for files (extension, well-known names, first line) using the same
//! syntect/two-face syntax set as the highlighter.

use std::path::Path;

use syntect::parsing::SyntaxSet;

use crate::git::git_syntax_for;

pub const PLAIN_TEXT: &str = "Plain Text";

pub struct Langs {
    syntaxes: SyntaxSet,
}

impl Langs {
    pub fn new() -> Self {
        Self {
            syntaxes: two_face::syntax::extra_newlines(),
        }
    }

    /// All language names, sorted, for the language picker.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .syntaxes
            .syntaxes()
            .iter()
            .map(|s| s.name.clone())
            .collect();
        v.sort_by_key(|s| s.to_lowercase());
        v.dedup();
        v
    }

    /// Detect the language of a document from its path and first line.
    pub fn detect(&self, path: Option<&Path>, first_line: &str) -> String {
        if let Some(p) = path {
            if let Some(name) = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(git_syntax_for)
            {
                if self.syntaxes.find_syntax_by_name(name).is_some() {
                    return name.into();
                }
            }
            let by_ext = p
                .extension()
                .and_then(|e| e.to_str())
                .and_then(|e| self.syntaxes.find_syntax_by_extension(e));
            let by_name = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| self.syntaxes.find_syntax_by_extension(n));
            if let Some(s) = by_ext.or(by_name) {
                return s.name.clone();
            }
        }
        if let Some(s) = self.syntaxes.find_syntax_by_first_line(first_line) {
            return s.name.clone();
        }
        PLAIN_TEXT.into()
    }
}
