//! Optional tree-sitter highlighting plugin for nanote.
//!
//! Built as a `cdylib` with a small C ABI so that `nanote.exe` can load it at runtime
//! (see `src/treesitter.rs` in the host). All data crossing the boundary is plain
//! `#[repr(C)]` structs and UTF-8 pointer/length pairs; memory allocated here is freed here
//! (`nts_free`).
//!
//! ABI (version 1):
//! * `nts_abi_version() -> u32`
//! * `nts_language_count() -> usize`, `nts_language_name(i, *len) -> *const u8`
//! * `nts_capture_count() -> usize`,  `nts_capture_name(i, *len) -> *const u8`
//! * `nts_highlight(lang, lang_len, text, text_len, *out_spans, *out_len) -> i32`
//! * `nts_free(spans, len)`

use std::{cell::RefCell, collections::HashMap};

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};
use tree_sitter_language::LanguageFn;

pub const ABI_VERSION: u32 = 1;

/// A highlighted byte range. `capture` indexes `nts_capture_name`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub capture: u32,
}

/// Capture names the host knows how to colour. Queries' captures are mapped to the closest
/// of these (`function.builtin` falls back to `function`, ...).
const CAPTURES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "comment.doc",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "emphasis",
    "emphasis.strong",
    "enum",
    "escape",
    "function",
    "function.builtin",
    "function.macro",
    "keyword",
    "label",
    "link_text",
    "link_uri",
    "module",
    "number",
    "operator",
    "preproc",
    "property",
    "property.builtin",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.list_marker",
    "punctuation.special",
    "string",
    "string.escape",
    "string.regex",
    "string.special",
    "string.special.symbol",
    "tag",
    "text.emphasis",
    "text.literal",
    "text.reference",
    "text.strong",
    "text.title",
    "text.uri",
    "title",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "variant",
];

struct LangDef {
    id: &'static str,
    language: LanguageFn,
    /// Query sources, concatenated in order (some grammars' queries build on another's).
    highlights: &'static [&'static str],
    injections: &'static [&'static str],
    locals: &'static [&'static str],
    /// Languages this one embeds (their configs must exist while highlighting).
    deps: &'static [&'static str],
}

const fn def(
    id: &'static str,
    language: LanguageFn,
    highlights: &'static [&'static str],
) -> LangDef {
    LangDef {
        id,
        language,
        highlights,
        injections: &[],
        locals: &[],
        deps: &[],
    }
}

const LANGS: &[LangDef] = &[
    def("rust", tree_sitter_rust::LANGUAGE, &[tree_sitter_rust::HIGHLIGHTS_QUERY]),
    def("python", tree_sitter_python::LANGUAGE, &[tree_sitter_python::HIGHLIGHTS_QUERY]),
    def("c", tree_sitter_c::LANGUAGE, &[tree_sitter_c::HIGHLIGHT_QUERY]),
    // The C++ query only adds to C's, so C's must come first.
    def(
        "cpp",
        tree_sitter_cpp::LANGUAGE,
        &[tree_sitter_c::HIGHLIGHT_QUERY, tree_sitter_cpp::HIGHLIGHT_QUERY],
    ),
    def("java", tree_sitter_java::LANGUAGE, &[tree_sitter_java::HIGHLIGHTS_QUERY]),
    def("csharp", tree_sitter_c_sharp::LANGUAGE, &[tree_sitter_c_sharp::HIGHLIGHTS_QUERY]),
    LangDef {
        injections: &[tree_sitter_javascript::INJECTIONS_QUERY],
        locals: &[tree_sitter_javascript::LOCALS_QUERY],
        ..def(
            "javascript",
            tree_sitter_javascript::LANGUAGE,
            &[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            ],
        )
    },
    def("sql", tree_sitter_sequel::LANGUAGE, &[tree_sitter_sequel::HIGHLIGHTS_QUERY]),
    def("go", tree_sitter_go::LANGUAGE, &[tree_sitter_go::HIGHLIGHTS_QUERY]),
    LangDef {
        injections: &[tree_sitter_html::INJECTIONS_QUERY],
        deps: &["javascript", "css"],
        ..def("html", tree_sitter_html::LANGUAGE, &[tree_sitter_html::HIGHLIGHTS_QUERY])
    },
    def("css", tree_sitter_css::LANGUAGE, &[tree_sitter_css::HIGHLIGHTS_QUERY]),
    def("json", tree_sitter_json::LANGUAGE, &[tree_sitter_json::HIGHLIGHTS_QUERY]),
    def("yaml", tree_sitter_yaml::LANGUAGE, &[tree_sitter_yaml::HIGHLIGHTS_QUERY]),
    def("toml", tree_sitter_toml_ng::LANGUAGE, &[tree_sitter_toml_ng::HIGHLIGHTS_QUERY]),
    LangDef {
        injections: &[tree_sitter_md::INJECTION_QUERY_BLOCK],
        deps: &["markdown_inline"],
        ..def(
            "markdown",
            tree_sitter_md::LANGUAGE,
            &[tree_sitter_md::HIGHLIGHT_QUERY_BLOCK],
        )
    },
    LangDef {
        injections: &[tree_sitter_md::INJECTION_QUERY_INLINE],
        ..def(
            "markdown_inline",
            tree_sitter_md::INLINE_LANGUAGE,
            &[tree_sitter_md::HIGHLIGHT_QUERY_INLINE],
        )
    },
    def("bash", tree_sitter_bash::LANGUAGE, &[tree_sitter_bash::HIGHLIGHT_QUERY]),
    LangDef {
        injections: &[tree_sitter_cmake::INJECTIONS_QUERY],
        ..def("cmake", tree_sitter_cmake::LANGUAGE, &[tree_sitter_cmake::HIGHLIGHTS_QUERY])
    },
    def("diff", tree_sitter_diff::LANGUAGE, &[tree_sitter_diff::HIGHLIGHTS_QUERY]),
    LangDef {
        injections: &[tree_sitter_elixir::INJECTIONS_QUERY],
        ..def("elixir", tree_sitter_elixir::LANGUAGE, &[tree_sitter_elixir::HIGHLIGHTS_QUERY])
    },
    LangDef {
        injections: &[tree_sitter_embedded_template::INJECTIONS_EJS_QUERY],
        deps: &["javascript", "html"],
        ..def(
            "ejs",
            tree_sitter_embedded_template::LANGUAGE,
            &[tree_sitter_embedded_template::HIGHLIGHTS_QUERY],
        )
    },
    LangDef {
        injections: &[tree_sitter_embedded_template::INJECTIONS_ERB_QUERY],
        deps: &["ruby", "html"],
        ..def(
            "erb",
            tree_sitter_embedded_template::LANGUAGE,
            &[tree_sitter_embedded_template::HIGHLIGHTS_QUERY],
        )
    },
    def("kotlin", tree_sitter_kotlin_sg::LANGUAGE, &[tree_sitter_kotlin_sg::HIGHLIGHTS_QUERY]),
    LangDef {
        injections: &[tree_sitter_lua::INJECTIONS_QUERY],
        locals: &[tree_sitter_lua::LOCALS_QUERY],
        ..def("lua", tree_sitter_lua::LANGUAGE, &[tree_sitter_lua::HIGHLIGHTS_QUERY])
    },
    def("make", tree_sitter_make::LANGUAGE, &[tree_sitter_make::HIGHLIGHTS_QUERY]),
    LangDef {
        injections: &[tree_sitter_php::INJECTIONS_QUERY],
        deps: &["html", "javascript", "css"],
        ..def("php", tree_sitter_php::LANGUAGE_PHP, &[tree_sitter_php::HIGHLIGHTS_QUERY])
    },
    LangDef {
        injections: &[tree_sitter_php::INJECTIONS_QUERY],
        ..def(
            "php_only",
            tree_sitter_php::LANGUAGE_PHP_ONLY,
            &[tree_sitter_php::HIGHLIGHTS_QUERY],
        )
    },
    LangDef {
        locals: &[tree_sitter_ruby::LOCALS_QUERY],
        ..def("ruby", tree_sitter_ruby::LANGUAGE, &[tree_sitter_ruby::HIGHLIGHTS_QUERY])
    },
    LangDef {
        locals: &[tree_sitter_scala::LOCALS_QUERY],
        ..def("scala", tree_sitter_scala::LANGUAGE, &[tree_sitter_scala::HIGHLIGHTS_QUERY])
    },
    LangDef {
        injections: &[tree_sitter_svelte_next::INJECTIONS_QUERY],
        locals: &[tree_sitter_svelte_next::LOCALS_QUERY],
        deps: &["javascript", "typescript", "css"],
        ..def(
            "svelte",
            tree_sitter_svelte_next::LANGUAGE,
            &[tree_sitter_svelte_next::HIGHLIGHTS_QUERY],
        )
    },
    LangDef {
        injections: &[tree_sitter_astro_next::INJECTIONS_QUERY],
        deps: &["javascript", "typescript", "css"],
        ..def(
            "astro",
            tree_sitter_astro_next::LANGUAGE,
            &[tree_sitter_astro_next::HIGHLIGHTS_QUERY],
        )
    },
    LangDef {
        injections: &[tree_sitter_swift::INJECTIONS_QUERY],
        locals: &[tree_sitter_swift::LOCALS_QUERY],
        ..def("swift", tree_sitter_swift::LANGUAGE, &[tree_sitter_swift::HIGHLIGHTS_QUERY])
    },
    // The TypeScript queries build on JavaScript's.
    LangDef {
        locals: &[tree_sitter_typescript::LOCALS_QUERY],
        ..def(
            "typescript",
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            &[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            ],
        )
    },
    LangDef {
        locals: &[tree_sitter_typescript::LOCALS_QUERY],
        ..def(
            "tsx",
            tree_sitter_typescript::LANGUAGE_TSX,
            &[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            ],
        )
    },
    LangDef {
        injections: &[tree_sitter_zig::INJECTIONS_QUERY],
        ..def("zig", tree_sitter_zig::LANGUAGE, &[tree_sitter_zig::HIGHLIGHTS_QUERY])
    },
    def("jsdoc", tree_sitter_jsdoc::LANGUAGE, &[tree_sitter_jsdoc::HIGHLIGHTS_QUERY]),
];

fn find_def(id: &str) -> Option<&'static LangDef> {
    LANGS.iter().find(|d| d.id == id)
}

/// Names that embedded-code queries use for a language, mapped to our ids.
fn canonical(name: &str) -> &str {
    match name {
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "ts" => "typescript",
        "py" => "python",
        "rs" => "rust",
        "sh" | "shell" | "zsh" => "bash",
        "yml" => "yaml",
        "md" => "markdown",
        "c++" | "cc" | "cxx" | "hpp" => "cpp",
        "c#" | "cs" => "csharp",
        "rb" => "ruby",
        "kt" => "kotlin",
        "golang" => "go",
        other => other,
    }
}

thread_local! {
    static HIGHLIGHTER: RefCell<Highlighter> = RefCell::new(Highlighter::new());
    /// Highlight configurations, built lazily per language.
    static CONFIGS: RefCell<HashMap<&'static str, HighlightConfiguration>> =
        RefCell::new(HashMap::new());
}

fn build_config(def: &LangDef) -> Result<HighlightConfiguration, String> {
    let mut injections = def.injections.join("\n");
    if def.id == "markdown" {
        // The block grammar's `inline` node has one child per line, and tree-sitter-highlight
        // leaves children out of an injected range unless told otherwise: without this the
        // inline grammar would be handed nothing to parse (no emphasis, code spans or links).
        injections = injections.replace(
            "(#set! injection.language \"markdown_inline\")",
            "(#set! injection.language \"markdown_inline\")\n  (#set! injection.include-children)",
        );
    }
    let mut config = HighlightConfiguration::new(
        def.language.into(),
        def.id,
        &def.highlights.join("\n"),
        &injections,
        &def.locals.join("\n"),
    )
    .map_err(|e| format!("{}: {e}", def.id))?;
    config.configure(CAPTURES);
    Ok(config)
}

/// Make sure the config for `id` (and, recursively, what it embeds) exists.
fn ensure(
    configs: &mut HashMap<&'static str, HighlightConfiguration>,
    id: &str,
) -> Result<(), String> {
    let Some(def) = find_def(canonical(id)) else {
        return Ok(()); // unknown embedded language: simply not highlighted
    };
    if configs.contains_key(def.id) {
        return Ok(());
    }
    let config = build_config(def)?;
    configs.insert(def.id, config);
    for dep in def.deps {
        // An embedded language that fails to build only loses its own highlighting.
        let _ = ensure(configs, dep);
    }
    Ok(())
}

/// Info strings of fenced code blocks (```rust), so their languages are ready before parsing.
fn fence_languages(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let rest = l.strip_prefix("```").or_else(|| l.strip_prefix("~~~"))?;
            rest.split(|c: char| c.is_whitespace() || c == '{').next()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Highlight `text` as `lang`. Returns non-overlapping spans in order; the innermost capture
/// wins where captures nest.
pub fn highlight(lang: &str, text: &str) -> Result<Vec<Span>, String> {
    let def = find_def(lang).ok_or_else(|| format!("unknown language: {lang}"))?;

    CONFIGS.with(|configs| {
        {
            let mut configs = configs.borrow_mut();
            ensure(&mut configs, def.id)?;
            if def.id == "markdown" {
                for fence in fence_languages(text) {
                    let _ = ensure(&mut configs, fence);
                }
            }
        }
        let configs = configs.borrow();
        let config = &configs[def.id];

        HIGHLIGHTER.with(|hl| {
            let mut hl = hl.borrow_mut();
            let events = hl
                .highlight(config, text.as_bytes(), None, None, |name| {
                    configs.get(canonical(name))
                })
                .map_err(|e| e.to_string())?;
            let mut stack: Vec<usize> = Vec::new();
            let mut spans: Vec<Span> = Vec::new();
            for event in events {
                match event.map_err(|e| e.to_string())? {
                    HighlightEvent::HighlightStart(h) => stack.push(h.0),
                    HighlightEvent::HighlightEnd => {
                        stack.pop();
                    }
                    HighlightEvent::Source { start, end } => {
                        let Some(&capture) = stack.last() else {
                            continue;
                        };
                        if start >= end {
                            continue;
                        }
                        let (start, end, capture) = (start as u32, end as u32, capture as u32);
                        match spans.last_mut() {
                            Some(last) if last.capture == capture && last.end == start => {
                                last.end = end
                            }
                            _ => spans.push(Span {
                                start,
                                end,
                                capture,
                            }),
                        }
                    }
                }
            }
            Ok(spans)
        })
    })
}

// ------------------------------------------------------------------ C ABI

#[unsafe(no_mangle)]
pub extern "C" fn nts_abi_version() -> u32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn nts_language_count() -> usize {
    LANGS.len()
}

/// # Safety
/// `len` must be a valid pointer. The returned bytes are static, UTF-8, not NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nts_language_name(i: usize, len: *mut usize) -> *const u8 {
    match LANGS.get(i) {
        Some(d) => {
            unsafe { *len = d.id.len() };
            d.id.as_ptr()
        }
        None => {
            unsafe { *len = 0 };
            std::ptr::null()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nts_capture_count() -> usize {
    CAPTURES.len()
}

/// # Safety
/// `len` must be a valid pointer. The returned bytes are static, UTF-8, not NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nts_capture_name(i: usize, len: *mut usize) -> *const u8 {
    match CAPTURES.get(i) {
        Some(n) => {
            unsafe { *len = n.len() };
            n.as_ptr()
        }
        None => {
            unsafe { *len = 0 };
            std::ptr::null()
        }
    }
}

/// Highlight `text` as language `lang`. On success (`0`) `*out_spans`/`*out_len` describe an
/// array that must be released with [`nts_free`].
/// Errors: `1` unknown language, `2` invalid UTF-8, `3` highlighting failed.
///
/// # Safety
/// All pointers must be valid for the given lengths; `out_*` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nts_highlight(
    lang: *const u8,
    lang_len: usize,
    text: *const u8,
    text_len: usize,
    out_spans: *mut *mut Span,
    out_len: *mut usize,
) -> i32 {
    unsafe {
        *out_spans = std::ptr::null_mut();
        *out_len = 0;
    }
    let lang = unsafe { std::slice::from_raw_parts(lang, lang_len) };
    let text = unsafe { std::slice::from_raw_parts(text, text_len) };
    let (Ok(lang), Ok(text)) = (std::str::from_utf8(lang), std::str::from_utf8(text)) else {
        return 2;
    };
    if find_def(lang).is_none() {
        return 1;
    }
    match highlight(lang, text) {
        Ok(spans) => {
            let boxed = spans.into_boxed_slice();
            unsafe {
                *out_len = boxed.len();
                *out_spans = Box::into_raw(boxed) as *mut Span;
            }
            0
        }
        Err(_) => 3,
    }
}

/// Release an array returned by [`nts_highlight`].
///
/// # Safety
/// `spans`/`len` must be exactly what `nts_highlight` returned, and freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nts_free(spans: *mut Span, len: usize) {
    if !spans.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(spans, len)) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small but non-trivial sample per language. Every language must produce spans.
    const SAMPLES: &[(&str, &str)] = &[
        ("rust", "fn main() { let s = \"hi\"; // c\n}\n"),
        ("python", "def f(x):\n    return \"a\" + x  # c\n"),
        ("c", "int main(void) { return 0; /* c */ }\n"),
        ("cpp", "#include <vector>\nclass A { public: int x; };\n"),
        ("java", "public class A { String s = \"x\"; }\n"),
        ("csharp", "class A { static void Main() { var s = \"x\"; } }\n"),
        ("javascript", "const f = (x) => `a${x}`; // c\n"),
        ("sql", "SELECT id, name FROM users WHERE id = 1;\n"),
        ("go", "package main\nfunc main() { x := \"a\" }\n"),
        ("html", "<!DOCTYPE html>\n<div class=\"a\">hi</div>\n"),
        ("css", "a { color: red; }\n"),
        ("json", "{\"a\": [1, true, null], \"b\": \"c\"}\n"),
        ("yaml", "a: 1\nb:\n  - x\n  - \"y\"\n"),
        ("toml", "[a]\nb = 1\nc = \"d\"\n"),
        ("markdown", "# Title\n\nSome *emph* and `code`.\n\n```rust\nfn main() {}\n```\n"),
        ("markdown_inline", "some *emph* and `code`"),
        ("bash", "if [ -f a ]; then echo \"hi\"; fi\n"),
        ("cmake", "project(a)\nadd_executable(a main.c)\n"),
        ("diff", "--- a\n+++ b\n@@ -1 +1 @@\n-a\n+b\n"),
        ("elixir", "defmodule A do\n  def f(x), do: x + 1\nend\n"),
        ("ejs", "<ul><% for (let i of a) { %><li><%= i %></li><% } %></ul>\n"),
        ("erb", "<ul><% a.each do |i| %><li><%= i %></li><% end %></ul>\n"),
        ("kotlin", "fun main() { val s = \"a\" }\n"),
        ("lua", "local function f(x) return x + 1 end -- c\n"),
        ("make", "all: a.o\n\tcc -o a a.o\n"),
        ("php", "<?php function f($x) { return $x . \"a\"; } ?>\n<p>hi</p>\n"),
        ("php_only", "<?php function f($x) { return $x . \"a\"; }\n"),
        ("ruby", "def f(x)\n  puts \"a#{x}\" # c\nend\n"),
        ("scala", "object A { def f(x: Int) = x + 1 }\n"),
        ("svelte", "<script>let a = 1;</script>\n<p>{a}</p>\n"),
        ("astro", "---\nconst a = 1;\n---\n<p>{a}</p>\n"),
        ("swift", "func f(_ x: Int) -> Int { return x + 1 }\n"),
        ("typescript", "const f = (x: number): string => `a${x}`;\n"),
        ("tsx", "const A = () => <div className=\"a\">hi</div>;\n"),
        ("zig", "pub fn main() void { const a = 1; _ = a; }\n"),
        ("jsdoc", "/** @param {string} a the thing */\n"),
    ];

    #[test]
    fn every_language_has_a_sample_and_highlights() {
        assert_eq!(SAMPLES.len(), LANGS.len(), "add a sample for every language");
        let mut failures = Vec::new();
        for def in LANGS {
            let src = SAMPLES
                .iter()
                .find(|(id, _)| *id == def.id)
                .unwrap_or_else(|| panic!("no sample for {}", def.id))
                .1;
            let spans = match highlight(def.id, src) {
                Ok(s) => s,
                Err(e) => {
                    failures.push(format!("{}: {e}", def.id));
                    continue;
                }
            };
            if spans.is_empty() {
                failures.push(format!("{}: no spans", def.id));
                continue;
            }
            let mut prev_end = 0;
            for s in &spans {
                let id = def.id;
                let ok = s.start >= prev_end
                    && s.end > s.start
                    && (s.end as usize) <= src.len()
                    && (s.capture as usize) < CAPTURES.len()
                    && src.is_char_boundary(s.start as usize)
                    && src.is_char_boundary(s.end as usize);
                if !ok {
                    failures.push(format!("{id}: invalid span {s:?}"));
                    break;
                }
                prev_end = s.end;
            }
        }
        assert!(failures.is_empty(), "failing languages:\n{}", failures.join("\n"));
    }

    #[test]
    fn markdown_fence_is_highlighted_by_its_language() {
        let src = "```rust\nfn main() {}\n```\n";
        let spans = highlight("markdown", src).unwrap();
        // `fn` sits inside the fence: some span must start within the code line.
        assert!(spans
            .iter()
            .any(|s| (s.start as usize) >= 8 && (s.end as usize) <= 21));
    }

    #[test]
    fn c_abi_round_trip() {
        let (lang, src) = ("rust", "fn main() {}\n");
        let (mut spans, mut len) = (std::ptr::null_mut(), 0usize);
        let rc = unsafe {
            nts_highlight(lang.as_ptr(), lang.len(), src.as_ptr(), src.len(), &mut spans, &mut len)
        };
        assert_eq!(rc, 0);
        assert!(len > 0 && !spans.is_null());
        unsafe { nts_free(spans, len) };
        let rc = unsafe {
            nts_highlight(b"nope".as_ptr(), 4, src.as_ptr(), src.len(), &mut spans, &mut len)
        };
        assert_eq!(rc, 1);
    }

    #[test]
    fn non_ascii_text_has_valid_boundaries() {
        let src = "fn main() { let s = \"안녕하세요\"; }\n";
        for s in highlight("rust", src).unwrap() {
            assert!(src.is_char_boundary(s.start as usize));
            assert!(src.is_char_boundary(s.end as usize));
        }
    }

    #[test]
    fn markdown_inline_spans_are_highlighted() {
        // `*emph*` is bytes 5..11, `code` is 20..26: both come from the inline grammar.
        let src = "Some *emph* and `code` here\n";
        let spans = highlight("markdown", src).unwrap();
        let names: Vec<&str> = spans.iter().map(|s| CAPTURES[s.capture as usize]).collect();
        let covers = |lo: u32, hi: u32| spans.iter().any(|s| s.start < hi && s.end > lo);
        assert!(covers(6, 10), "emphasis not highlighted: {spans:?} {names:?}");
        assert!(covers(21, 25), "inline code not highlighted: {spans:?} {names:?}");
        assert!(names.contains(&"text.emphasis"), "{names:?}");
    }
}
