//! Optional tree-sitter highlighting, provided by the `nanote_tree_sitter` plugin library
//! (`nanote_tree_sitter.dll` next to the executable). See `crates/nanote-tree-sitter`.
//!
//! The plugin reports which capture covers which byte range; colours come from the active
//! highlight theme, so both engines follow the same theme.

use std::{ops::Range, path::PathBuf, rc::Rc};

use gpui_kit::component::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
    InputHighlighterFactory, Rope,
};
use gpui_kit::*;

/// Must match `ABI_VERSION` in the plugin.
const ABI_VERSION: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct Span {
    start: u32,
    end: u32,
    capture: u32,
}

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type CountFn = unsafe extern "C" fn() -> usize;
type NameFn = unsafe extern "C" fn(usize, *mut usize) -> *const u8;
type HighlightFn =
    unsafe extern "C" fn(*const u8, usize, *const u8, usize, *mut *mut Span, *mut usize) -> i32;
type FreeFn = unsafe extern "C" fn(*mut Span, usize);

#[derive(Clone)]
pub enum PluginStatus {
    NotFound,
    Loaded { languages: usize },
    Failed(String),
}

pub struct TsPlugin {
    languages: Vec<String>,
    captures: Vec<String>,
    highlight: HighlightFn,
    free: FreeFn,
    /// Keeps the library mapped for as long as the function pointers above are used.
    _lib: libloading::Library,
}

/// Where the plugin is looked for: only next to the executable, never the working directory
/// (which would allow planting a DLL).
fn plugin_path() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    Some(dir.join(libloading::library_filename("nanote_tree_sitter")))
}

impl TsPlugin {
    pub fn load() -> Result<Self, PluginStatus> {
        let path = plugin_path().ok_or(PluginStatus::NotFound)?;
        if !path.is_file() {
            return Err(PluginStatus::NotFound);
        }
        Self::load_from(&path).map_err(PluginStatus::Failed)
    }

    fn load_from(path: &std::path::Path) -> Result<Self, String> {
        // SAFETY: loading a library runs its initialisers; it is our own plugin from the
        // application directory, and its exported symbols are checked against the ABI below.
        unsafe {
            let lib = libloading::Library::new(path).map_err(|e| e.to_string())?;
            let version: AbiVersionFn = *lib.get(b"nts_abi_version").map_err(|e| e.to_string())?;
            let v = version();
            if v != ABI_VERSION {
                return Err(format!("plugin ABI {v}, expected {ABI_VERSION}"));
            }
            let lang_count: CountFn = *lib.get(b"nts_language_count").map_err(|e| e.to_string())?;
            let lang_name: NameFn = *lib.get(b"nts_language_name").map_err(|e| e.to_string())?;
            let cap_count: CountFn = *lib.get(b"nts_capture_count").map_err(|e| e.to_string())?;
            let cap_name: NameFn = *lib.get(b"nts_capture_name").map_err(|e| e.to_string())?;
            let highlight: HighlightFn = *lib.get(b"nts_highlight").map_err(|e| e.to_string())?;
            let free: FreeFn = *lib.get(b"nts_free").map_err(|e| e.to_string())?;

            let names = |count: CountFn, name: NameFn| -> Vec<String> {
                (0..count())
                    .filter_map(|i| {
                        let mut len = 0usize;
                        let ptr = name(i, &mut len);
                        if ptr.is_null() {
                            return None;
                        }
                        let bytes = std::slice::from_raw_parts(ptr, len);
                        std::str::from_utf8(bytes).ok().map(str::to_owned)
                    })
                    .collect()
            };
            Ok(Self {
                languages: names(lang_count, lang_name),
                captures: names(cap_count, cap_name),
                highlight,
                free,
                _lib: lib,
            })
        }
    }

    /// Number of distinct grammars that can be selected for a file (internal ones that are only
    /// embedded in other languages, like markdown_inline, are not counted).
    pub fn language_count(&self) -> usize {
        let mut ids: Vec<&str> = PLUGIN_LANGUAGES
            .iter()
            .map(|(_, id)| *id)
            .filter(|id| self.languages.iter().any(|l| l == id))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    }

    /// Whether the plugin can highlight the language (a syntect language name, e.g. "Rust").
    pub fn supports(&self, lang: &str) -> bool {
        plugin_language_id(lang).is_some_and(|id| self.languages.iter().any(|l| l == id))
    }

    fn call(&self, id: &str, text: &str) -> Option<Vec<Span>> {
        if text.len() > u32::MAX as usize {
            return None;
        }
        let (mut spans, mut len) = (std::ptr::null_mut(), 0usize);
        // SAFETY: pointers/lengths come from live &str values; the plugin allocates the result
        // and we hand it back to `free` exactly once after copying.
        unsafe {
            let rc = (self.highlight)(
                id.as_ptr(),
                id.len(),
                text.as_ptr(),
                text.len(),
                &mut spans,
                &mut len,
            );
            if rc != 0 {
                return None;
            }
            let copy = if spans.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(spans, len).to_vec()
            };
            (self.free)(spans, len);
            Some(copy)
        }
    }

    /// Factory the editor uses to create highlighters for languages this plugin supports.
    pub fn factory(self: &Rc<Self>) -> InputHighlighterFactory {
        let plugin = self.clone();
        Rc::new(move |language: &str| {
            let id = plugin_language_id(language)?;
            plugin.supports(language).then(|| {
                Box::new(PluginHighlighter {
                    language: SharedString::from(language.to_owned()),
                    id,
                    plugin: plugin.clone(),
                    spans: Vec::new(),
                }) as Box<dyn InputHighlighter>
            })
        })
    }
}

/// (syntect language name, plugin language id) for every language the plugin can highlight.
pub const PLUGIN_LANGUAGES: &[(&str, &str)] = &[
    ("Rust", "rust"),
    ("Python", "python"),
    ("C", "c"),
    ("C++", "cpp"),
    ("Java", "java"),
    ("C#", "csharp"),
    ("JavaScript", "javascript"),
    ("JavaScript (Rails)", "javascript"),
    ("SQL", "sql"),
    ("Go", "go"),
    ("HTML", "html"),
    ("HTML (Rails)", "erb"),
    ("CSS", "css"),
    ("JSON", "json"),
    ("YAML", "yaml"),
    ("TOML", "toml"),
    ("Markdown", "markdown"),
    ("Bourne Again Shell (bash)", "bash"),
    ("Shell-Unix-Generic", "bash"),
    ("CMake", "cmake"),
    ("Diff", "diff"),
    ("Elixir", "elixir"),
    ("Kotlin", "kotlin"),
    ("Lua", "lua"),
    ("Makefile", "make"),
    ("PHP", "php"),
    ("PHP Source", "php_only"),
    ("Ruby", "ruby"),
    ("Ruby on Rails", "ruby"),
    ("Scala", "scala"),
    ("Svelte", "svelte"),
    ("Swift", "swift"),
    ("TypeScript", "typescript"),
    ("TypeScriptReact", "tsx"),
    ("Zig", "zig"),
];

/// syntect language name -> plugin language id.
pub fn plugin_language_id(lang: &str) -> Option<&'static str> {
    PLUGIN_LANGUAGES
        .iter()
        .find(|(name, _)| *name == lang)
        .map(|(_, id)| *id)
}

/// The plugin's capture names use tree-sitter conventions; the GPUI theme knows fewer names.
fn theme_name(capture: &str) -> &str {
    match capture {
        "function.builtin" | "function.macro" => "function",
        "constant.builtin" => "constant",
        "type.builtin" => "type",
        "variable.builtin" => "variable.special",
        "variable.parameter" => "variable",
        "property.builtin" => "property",
        "escape" | "string.escape" => "string.escape",
        "module" => "type",
        "text.title" => "title",
        "text.emphasis" => "emphasis",
        "text.strong" => "emphasis.strong",
        "text.uri" => "link_uri",
        "text.reference" => "link_text",
        other => other,
    }
}

struct PluginHighlighter {
    language: SharedString,
    id: &'static str,
    plugin: Rc<TsPlugin>,
    /// Ordered, non-overlapping byte ranges with their capture index.
    spans: Vec<(Range<usize>, usize)>,
}

impl PluginHighlighter {
    fn reparse(&mut self, text: &Rope) {
        let text = text.to_string();
        let Some(spans) = self.plugin.call(self.id, &text) else {
            self.spans.clear();
            return;
        };
        self.spans = spans
            .into_iter()
            .filter(|s| s.end > s.start && (s.end as usize) <= text.len())
            .map(|s| (s.start as usize..s.end as usize, s.capture as usize))
            .collect();
    }
}

impl InputHighlighter for PluginHighlighter {
    fn language(&self) -> SharedString {
        self.language.clone()
    }

    fn update(
        &mut self,
        _edit: Option<InputEdit>,
        text: &Rope,
        _folding: bool,
        _window: &mut Window,
        _cx: &mut Context<EditorState>,
    ) {
        // Full reparse; the plugin is fast enough for notepad-sized files.
        self.reparse(text);
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let mut out = Vec::new();
        let mut pos = range.start;
        // Spans are sorted: skip to the first one that can intersect `range`.
        let first = self.spans.partition_point(|(r, _)| r.end <= range.start);
        for (r, capture) in &self.spans[first..] {
            if r.start >= range.end {
                break;
            }
            let (start, end) = (r.start.max(range.start), r.end.min(range.end));
            if start > pos {
                out.push((pos..start, HighlightStyle::default()));
            }
            let style = self
                .plugin
                .captures
                .get(*capture)
                .and_then(|name| resolver.style(theme_name(name)))
                .unwrap_or_default();
            out.push((start..end, style));
            pos = end;
        }
        if pos < range.end {
            out.push((pos..range.end, HighlightStyle::default()));
        }
        out
    }

    fn fold_ranges(&self, _text: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would pull in gpui's `test` macro and shadow `#[test]`.
    use super::PLUGIN_LANGUAGES;

    #[test]
    fn plugin_language_names_exist_in_syntect() {
        let names = crate::lang::Langs::new().names();
        for (name, id) in PLUGIN_LANGUAGES {
            assert!(names.iter().any(|n| n == name), "syntect has no `{name}` ({id})");
        }
    }
}
