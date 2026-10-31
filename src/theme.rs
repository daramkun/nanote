//! Syntax themes: the 32 syntect themes shipped by `two-face`, converted to GPUI Kit
//! highlight themes (which name colours by tree-sitter-style capture names).

use std::sync::Arc;

use gpui_kit::component::highlighter::HighlightTheme;
use serde_json::{Map, Value, json};
use syntect::{
    highlighting::{Color, FontStyle, Highlighter as ThemeLookup, Theme},
    parsing::Scope,
};
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

pub struct Themes {
    set: EmbeddedLazyThemeSet,
    pub dark: Vec<&'static str>,
    pub light: Vec<&'static str>,
}

fn hex(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, 0xff)
}

fn luminance(c: Color) -> f32 {
    (0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32) / 255.0
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let m = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    Color {
        r: m(a.r, b.r),
        g: m(a.g, b.g),
        b: m(a.b, b.b),
        a: 0xff,
    }
}

/// TextMate scope that best corresponds to each highlight name of the GPUI theme.
const SCOPES: &[(&str, &str)] = &[
    ("attribute", "entity.other.attribute-name"),
    ("boolean", "constant.language"),
    ("comment", "comment"),
    ("comment.doc", "comment.block.documentation"),
    ("constant", "constant"),
    ("constructor", "entity.name.type"),
    ("embedded", "meta.embedded"),
    ("emphasis", "markup.italic"),
    ("emphasis.strong", "markup.bold"),
    ("enum", "entity.name.type.enum"),
    ("function", "entity.name.function"),
    ("keyword", "keyword"),
    ("label", "entity.name.label"),
    ("link_text", "markup.underline.link"),
    ("link_uri", "markup.underline.link"),
    ("number", "constant.numeric"),
    ("operator", "keyword.operator"),
    ("preproc", "meta.preprocessor"),
    ("property", "variable.other.member"),
    ("punctuation", "punctuation"),
    ("punctuation.bracket", "punctuation"),
    ("punctuation.delimiter", "punctuation"),
    ("punctuation.list_marker", "punctuation"),
    ("punctuation.special", "punctuation"),
    ("string", "string"),
    ("string.escape", "constant.character.escape"),
    ("string.regex", "string.regexp"),
    ("string.special", "string"),
    ("string.special.symbol", "constant.other.symbol"),
    ("tag", "entity.name.tag"),
    ("tag.doctype", "meta.tag"),
    ("text.code.span", "markup.raw"),
    ("text.literal", "markup.raw"),
    ("title", "markup.heading"),
    ("type", "entity.name.type"),
    ("variable", "variable"),
    ("variable.special", "variable.language"),
    ("variant", "entity.name.type.enum"),
];

/// Many themes have no rules for Markdown-style markup. When a capture's own scope gives the
/// default look, try these scopes in order, then force a font style so the element still
/// stands out: (highlight name, fallback scopes, forced style).
const FALLBACKS: &[(&str, &[&str], Force)] = &[
    ("title", &["entity.name.section", "entity.name.function", "keyword"], Force::Bold),
    ("emphasis", &["string"], Force::Italic),
    ("emphasis.strong", &["constant.numeric", "constant"], Force::Bold),
    // GPUI cannot draw underlines from a theme, so the colour has to carry the distinction.
    (
        "link_uri",
        &["string.other.link", "constant.other", "constant", "constant.numeric", "string"],
        Force::Underline,
    ),
    ("link_text", &["entity.name.tag", "constant"], Force::None),
    ("punctuation.special", &["keyword.operator", "keyword"], Force::None),
];

#[derive(Clone, Copy, PartialEq)]
enum Force {
    None,
    Bold,
    Italic,
    Underline,
}

impl Themes {
    pub fn new() -> Self {
        let set = two_face::theme::extra();
        let (mut dark, mut light) = (Vec::new(), Vec::new());
        for t in EmbeddedLazyThemeSet::theme_names() {
            let bg = set.get(*t).settings.background.unwrap_or(Color::BLACK);
            if luminance(bg) < 0.5 {
                dark.push(t.as_name());
            } else {
                light.push(t.as_name());
            }
        }
        Self { set, dark, light }
    }

    fn syntect_theme(&self, name: &str) -> &Theme {
        let t = EmbeddedLazyThemeSet::theme_names()
            .iter()
            .find(|t| t.as_name() == name)
            .copied()
            .unwrap_or(EmbeddedThemeName::Base16OceanDark);
        self.set.get(t)
    }

    /// Build the GPUI highlight theme for the named syntect theme.
    pub fn highlight_theme(&self, name: &str) -> Option<Arc<HighlightTheme>> {
        let theme = self.syntect_theme(name);
        let bg = theme.settings.background.unwrap_or(Color::BLACK);
        let fg = theme.settings.foreground.unwrap_or(Color::WHITE);
        let dark = luminance(bg) < 0.5;
        let lookup = ThemeLookup::new(theme);

        let style_of = |scope: &str| {
            Scope::new(scope)
                .ok()
                .map(|s| lookup.style_for_stack(&[s]))
        };
        // Same look as plain text: no colour of its own and no font style.
        let is_plain =
            |st: &syntect::highlighting::Style| st.foreground == fg && st.font_style.is_empty();

        let mut syntax = Map::new();
        for (capture, scope) in SCOPES {
            let Some(mut style) = style_of(scope) else {
                continue;
            };
            let mut force = Force::None;
            if let Some((_, fallbacks, forced)) = FALLBACKS.iter().find(|(c, _, _)| c == capture) {
                if is_plain(&style) {
                    for fb in *fallbacks {
                        if let Some(st) = style_of(fb).filter(|st| !is_plain(st)) {
                            style = st;
                            break;
                        }
                    }
                    force = *forced;
                }
            }
            let mut entry = Map::new();
            entry.insert("color".into(), json!(hex(style.foreground)));
            let italic = style.font_style.contains(FontStyle::ITALIC) || force == Force::Italic;
            let underline =
                style.font_style.contains(FontStyle::UNDERLINE) || force == Force::Underline;
            if italic {
                entry.insert("font_style".into(), json!("italic"));
            } else if underline {
                entry.insert("font_style".into(), json!("underline"));
            }
            if style.font_style.contains(FontStyle::BOLD) || force == Force::Bold {
                entry.insert("font_weight".into(), json!(700));
            }
            syntax.insert((*capture).to_owned(), Value::Object(entry));
        }
        syntax.insert("primary".into(), json!({ "color": hex(fg) }));

        let value: Value = json!({
            "name": name,
            "appearance": if dark { "dark" } else { "light" },
            "style": {
                "editor.background": hex(bg),
                "editor.foreground": hex(fg),
                "editor.active_line.background": hex(mix(bg, fg, 0.06)),
                "editor.line_number": hex(mix(bg, fg, 0.45)),
                "editor.active_line_number": hex(fg),
                "editor.invisible": hex(mix(bg, fg, 0.3)),
                "syntax": Value::Object(syntax),
            }
        });
        match serde_json::from_value::<HighlightTheme>(value) {
            Ok(t) => Some(Arc::new(t)),
            Err(e) => {
                eprintln!("theme {name}: {e}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_syntect_theme_converts() {
        let themes = Themes::new();
        assert!(!themes.dark.is_empty() && !themes.light.is_empty());
        for name in themes.dark.iter().chain(&themes.light) {
            assert!(themes.highlight_theme(name).is_some(), "{name} failed");
        }
    }

    #[test]
    fn font_styles_are_carried_over() {
        // Most themes italicise comments or embolden headings; at least one must show up.
        let themes = Themes::new();
        let any_styled = themes
            .dark
            .iter()
            .chain(&themes.light)
            .filter_map(|n| themes.highlight_theme(n))
            .any(|t| {
                let json = serde_json::to_string(&*t).unwrap();
                json.contains("italic") || json.contains("\"font_weight\":700")
            });
        assert!(any_styled, "no theme kept a bold/italic style");
    }

    #[test]
    fn markup_stands_out_even_in_themes_without_markup_rules() {
        // The GitHub theme has no rules for headings / emphasis / links.
        let themes = Themes::new();
        let t = themes.highlight_theme("GitHub").unwrap();
        let v = serde_json::to_value(&*t).unwrap();
        let syn = &v["style"]["syntax"];
        assert_eq!(syn["title"]["font_weight"], 700, "{}", syn["title"]);
        assert_eq!(syn["emphasis"]["font_style"], "italic", "{}", syn["emphasis"]);
        assert_eq!(syn["emphasis.strong"]["font_weight"], 700);
        assert_eq!(syn["link_uri"]["font_style"], "underline");
    }
}
