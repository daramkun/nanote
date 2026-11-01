//! Syntax highlighting with syntect: one style per character of each visible line.
//!
//! The parser state at the start of every line is kept, so an edit only re-highlights from its
//! line down, and only as far as the screen needs.

use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use syntect::{
    highlighting::{FontStyle, HighlightState, Highlighter as SynHighlighter, RangedHighlightIterator, Theme},
    parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet},
};
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

use crate::git::git_syntax_for;

pub const PLAIN_TEXT: &str = "Plain Text";
/// Longer lines are shown without colours (parsing them would stall the editor).
const MAX_HIGHLIGHTED_LINE: usize = 4000;

pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

impl Highlighter {
    pub fn new(theme_name: &str) -> Self {
        let set = two_face::theme::extra();
        let theme = EmbeddedLazyThemeSet::theme_names()
            .iter()
            .find(|t| t.as_name().eq_ignore_ascii_case(theme_name))
            .copied()
            .unwrap_or(EmbeddedThemeName::Base16OceanDark);
        Self {
            syntaxes: two_face::syntax::extra_newlines(),
            theme: set.get(theme).clone(),
        }
    }

    /// Names of the colour themes, for `--theme`.
    pub fn theme_names() -> Vec<&'static str> {
        EmbeddedLazyThemeSet::theme_names()
            .iter()
            .map(|t| t.as_name())
            .collect()
    }

    /// The language of a file: Git's own files, then extension and name, then the first line.
    pub fn detect(&self, path: Option<&Path>, first_line: &str) -> String {
        if let Some(p) = path {
            let name = p.file_name().and_then(|n| n.to_str());
            if let Some(git) = name.and_then(git_syntax_for) {
                if self.syntaxes.find_syntax_by_name(git).is_some() {
                    return git.into();
                }
            }
            let by_ext = p
                .extension()
                .and_then(|e| e.to_str())
                .and_then(|e| self.syntaxes.find_syntax_by_extension(e));
            let by_name = name.and_then(|n| self.syntaxes.find_syntax_by_extension(n));
            if let Some(s) = by_ext.or(by_name) {
                return s.name.clone();
            }
        }
        self.syntaxes
            .find_syntax_by_first_line(first_line)
            .map_or_else(|| PLAIN_TEXT.into(), |s| s.name.clone())
    }

    /// All language names, for the language prompt.
    pub fn names(&self) -> Vec<String> {
        self.syntaxes.syntaxes().iter().map(|s| s.name.clone()).collect()
    }

    fn syntax(&self, name: &str) -> Option<&SyntaxReference> {
        if name == PLAIN_TEXT {
            return None;
        }
        self.syntaxes.find_syntax_by_name(name)
    }
}

/// The highlighting of one document.
pub struct Highlights {
    syntax: String,
    /// State at the start of line `i` (`states[0]` is the start of the file).
    states: Vec<(ParseState, HighlightState)>,
    /// Styles per character of the lines already processed.
    styled: Vec<Vec<Style>>,
}

impl Highlights {
    pub fn new(syntax: &str) -> Self {
        Self {
            syntax: syntax.to_owned(),
            states: Vec::new(),
            styled: Vec::new(),
        }
    }

    pub fn syntax(&self) -> &str {
        &self.syntax
    }

    pub fn set_syntax(&mut self, syntax: &str) {
        self.syntax = syntax.to_owned();
        self.invalidate(0);
    }

    /// Lines from `row` down must be processed again.
    pub fn invalidate(&mut self, row: usize) {
        self.styled.truncate(row);
        self.states.truncate(row + 1);
    }

    /// One style per character of line `row`.
    pub fn line(&mut self, hl: &Highlighter, lines: &[String], row: usize) -> &[Style] {
        let Some(syntax) = hl.syntax(&self.syntax) else {
            // Plain text: nothing to compute.
            self.styled.clear();
            self.states.clear();
            return &[];
        };
        if self.states.is_empty() {
            let syn_hl = SynHighlighter::new(&hl.theme);
            self.states.push((
                ParseState::new(syntax),
                HighlightState::new(&syn_hl, ScopeStack::new()),
            ));
        }
        let syn_hl = SynHighlighter::new(&hl.theme);
        while self.styled.len() <= row.min(lines.len().saturating_sub(1)) {
            let i = self.styled.len();
            let (mut parse, mut high) = self.states[i].clone();
            let line = &lines[i];
            let chars = line.chars().count();
            let mut styles = Vec::with_capacity(chars);
            if chars <= MAX_HIGHLIGHTED_LINE {
                let text = format!("{line}\n");
                if let Ok(ops) = parse.parse_line(&text, &hl.syntaxes) {
                    for (style, piece, _) in RangedHighlightIterator::new(&mut high, &ops, &text, &syn_hl) {
                        let ours = convert(style);
                        styles.extend(piece.chars().filter(|c| *c != '\n').map(|_| ours));
                    }
                }
            }
            styles.resize(chars, Style::default());
            self.styled.push(styles);
            self.states.push((parse, high));
        }
        self.styled.get(row).map_or(&[], |v| v.as_slice())
    }
}

fn convert(style: syntect::highlighting::Style) -> Style {
    let c = style.foreground;
    let mut out = Style::default().fg(Color::Rgb(c.r, c.g, c.b));
    if style.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_owned).collect()
    }

    #[test]
    fn detects_by_extension_name_and_first_line() {
        let hl = Highlighter::new("base16-ocean.dark");
        assert_eq!(hl.detect(Some(Path::new("main.rs")), ""), "Rust");
        assert_eq!(hl.detect(Some(Path::new("COMMIT_EDITMSG")), ""), "Git Commit");
        assert_eq!(hl.detect(None, "#!/bin/bash"), "Bourne Again Shell (bash)");
        assert_eq!(hl.detect(Some(Path::new("notes.nothing")), "hello"), PLAIN_TEXT);
    }

    #[test]
    fn code_gets_colours_and_plain_text_does_not() {
        let hl = Highlighter::new("base16-ocean.dark");
        let src = lines("fn main() {\n    let x = 1; // hi\n}");
        let mut h = Highlights::new("Rust");
        let line0 = h.line(&hl, &src, 0).to_vec();
        assert_eq!(line0.len(), 11);
        assert!(line0.iter().any(|s| s.fg.is_some()));
        // the keyword and the identifier are coloured differently
        assert_ne!(line0[0].fg, line0[3].fg);
        let mut plain = Highlights::new(PLAIN_TEXT);
        assert!(plain.line(&hl, &src, 0).is_empty());
    }

    #[test]
    fn multi_line_state_carries_over_and_invalidation_recomputes() {
        let hl = Highlighter::new("base16-ocean.dark");
        let mut src = lines("/* a\nb */ let");
        let mut h = Highlights::new("Rust");
        let in_comment = h.line(&hl, &src, 1).to_vec();
        // `b */` is still a comment: same colour as its closing `*/`
        assert_eq!(in_comment[0].fg, in_comment[2].fg);
        src[0] = "// a".into();
        h.invalidate(0);
        let after = h.line(&hl, &src, 1).to_vec();
        // now line 1 is code: `b` is no longer a comment
        assert_ne!(after[0].fg, in_comment[0].fg);
    }

    #[test]
    fn styles_cover_every_character() {
        let hl = Highlighter::new("base16-ocean.dark");
        let src = lines("let s = \"안녕 😀\";");
        let mut h = Highlights::new("Rust");
        assert_eq!(h.line(&hl, &src, 0).len(), src[0].chars().count());
    }
}
