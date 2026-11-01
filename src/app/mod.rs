//! The application: state and document handling. Rendering lives in `view.rs`.

mod multicursor;
mod open_path;
mod settings_ui;
mod shell_integration;
mod updates;
mod view;

pub use multicursor::*;
pub use open_path::QuickOpen;

use std::{
    io::Write,
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, mpsc::Receiver},
    time::Duration,
};

use gpui_kit::component::{
    ActiveTheme as _, Theme, ThemeMode as GpuiThemeMode,
    input::{EditorState, InputEvent},
    menu::AppMenuBar,
};
use gpui_kit::*;

use crate::{
    encoding::{Loaded, encode_text, read_file},
    i18n::{self, tr},
    ipc,
    lang::Langs,
    settings::{Session, Settings, ThemeMode, UiLang, WindowMode, WindowState},
    theme::Themes,
    treesitter::{PluginStatus, TsPlugin},
    updater::{self, Version},
};

/// The live settings. The settings window edits this global; the app observes it and applies
/// whatever changed (theme, font, language, editor options) and saves it.
pub struct SettingsGlobal(pub Settings);
impl Global for SettingsGlobal {}

/// The theme monospace family before we touch it, restored when "default" is chosen.
struct DefaultMono(SharedString);
impl Global for DefaultMono {}

actions!(
    nanote,
    [
        NewFile,
        OpenFile,
        Save,
        SaveAs,
        CloseTab,
        Quit,
        NextTab,
        PrevTab,
        OpenSettings,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        ToggleWrap,
        ToggleLineNumbers,
    ]
);

pub fn init(cx: &mut App) {
    multicursor::register_keys(cx);
    cx.bind_keys([
        KeyBinding::new("ctrl-n", NewFile, None),
        KeyBinding::new("ctrl-o", OpenFile, None),
        KeyBinding::new("ctrl-p", QuickOpen, None),
        KeyBinding::new("ctrl-s", Save, None),
        KeyBinding::new("ctrl-shift-s", SaveAs, None),
        KeyBinding::new("ctrl-w", CloseTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
        KeyBinding::new("ctrl-,", OpenSettings, None),
        KeyBinding::new("ctrl-=", ZoomIn, None),
        KeyBinding::new("ctrl-+", ZoomIn, None),
        KeyBinding::new("ctrl--", ZoomOut, None),
        KeyBinding::new("ctrl-0", ZoomReset, None),
    ]);
}

/// Command line: `nanote [--wait] [--new-window|--reuse-window] [--] [files...]`
#[derive(Default)]
pub struct Cli {
    pub paths: Vec<PathBuf>,
    pub wait: bool,
    pub new_window: Option<bool>,
    /// Started by the updater: wait for the previous instance to release the instance port.
    pub relaunch: bool,
    /// `--install-context-menu` / `--uninstall-context-menu`: do it and exit.
    pub shell_menu: Option<bool>,
    /// `--install-file-types[=group]`, `--uninstall-file-types`, `--open-default-apps`.
    pub file_types: FileTypesCommand,
}

/// File association work requested on the command line (done in this order, then exit).
#[derive(Default)]
pub struct FileTypesCommand {
    /// `Some(Some(group))`: register that group; `Some(None)`: remove the registration.
    pub register: Option<Option<crate::file_assoc::Group>>,
    pub open_default_apps: bool,
    /// An unknown group name was given.
    pub invalid: bool,
}

impl FileTypesCommand {
    pub fn requested(&self) -> bool {
        self.register.is_some() || self.open_default_apps || self.invalid
    }
}

pub fn parse_cli() -> Cli {
    let mut cli = Cli::default();
    let mut rest_are_paths = false;
    for arg in std::env::args_os().skip(1) {
        let text = arg.to_string_lossy();
        if !rest_are_paths && text.starts_with('-') && text.len() > 1 {
            match &*text {
                "--" => rest_are_paths = true,
                "-w" | "--wait" => cli.wait = true,
                "--relaunch" => cli.relaunch = true,
                "--install-context-menu" => cli.shell_menu = Some(true),
                "--uninstall-context-menu" => cli.shell_menu = Some(false),
                "--uninstall-file-types" => cli.file_types.register = Some(None),
                "--open-default-apps" => cli.file_types.open_default_apps = true,
                "--install-file-types" => {
                    cli.file_types.register = Some(Some(crate::file_assoc::Group::All))
                }
                _ if text.starts_with("--install-file-types=") => {
                    match crate::file_assoc::Group::from_name(&text["--install-file-types=".len()..])
                    {
                        Some(group) => cli.file_types.register = Some(Some(group)),
                        None => cli.file_types.invalid = true,
                    }
                }
                "-n" | "--new-window" => cli.new_window = Some(true),
                "-r" | "--reuse-window" => cli.new_window = Some(false),
                _ => {}
            }
        } else {
            let p = PathBuf::from(&arg);
            cli.paths.push(std::path::absolute(&p).unwrap_or(p));
        }
    }
    cli
}

/// One open tab.
pub struct Doc {
    pub id: u64,
    pub path: Option<PathBuf>,
    pub editor: Entity<EditorState>,
    pub dirty: bool,
    pub crlf: bool,
    /// Encoding name used when saving (see `encoding::ENCODINGS`).
    pub encoding: String,
    /// Whether a byte order mark is written on save (UTF-8/16/32 only).
    pub bom: bool,
    /// Manually chosen language; `None` = detect from the file.
    pub language: Option<String>,
    /// Language currently applied to the editor.
    pub applied_language: String,
    /// Whether the editor uses the tree-sitter plugin (otherwise the built-in highlighter).
    pub uses_plugin: bool,
    /// Selections created by `Ctrl+D` & co, in order, while they are still the live ones.
    pub occurrences: Vec<std::ops::Range<usize>>,
    /// Whether `Ctrl+D` matches whole words only (true when it started from a bare cursor).
    pub occurrence_whole_word: bool,
    /// Keyboard column selection in progress.
    pub column: Option<ColumnState>,
    /// The next `Change` event comes from us (loading), not from the user.
    ignore_change: bool,
    _subscription: Subscription,
}

impl Doc {
    pub fn title(&self) -> String {
        match &self.path {
            Some(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            None => tr("Untitled").into(),
        }
    }
}

/// Someone waits for a document to be closed (`--wait`); `None` stream = this process itself.
struct Waiter {
    doc_id: u64,
    stream: Option<TcpStream>,
}

pub struct NanoteApp {
    pub settings: Settings,
    pub docs: Vec<Doc>,
    pub active: usize,
    next_id: u64,
    pub langs: Langs,
    pub lang_names: Vec<String>,
    pub themes: Themes,
    /// The optional tree-sitter plugin, if present next to the executable.
    pub plugin: Option<Rc<TsPlugin>>,
    pub plugin_status: PluginStatus,
    menu_bar: Entity<AppMenuBar>,
    pub status: SharedString,
    incoming: Option<Receiver<ipc::Incoming>>,
    waiters: Vec<Waiter>,
    quit_when_clean: bool,
    force_quit: bool,
    last_session: (Vec<String>, usize),
    /// Bumped on every move/resize so only the last one of a burst is written to disk.
    bounds_generation: u64,
    /// The title currently shown in the OS title bar.
    window_title: String,
    /// Text of the updates line in the settings window.
    pub update_status: Entity<SharedString>,
    /// A newer version that is already installed (it runs after a restart).
    update_installed: Option<Version>,
    update_running: bool,
    /// Start the installed version once the app has quit.
    relaunch_on_quit: bool,
}

impl NanoteApp {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        settings: Settings,
        cli: Cli,
        listener: Option<TcpListener>,
    ) -> Self {
        i18n::set_korean(settings.korean());
        Self::set_menus(&settings, cx);
        cx.set_global(SettingsGlobal(settings.clone()));
        let default_mono = cx.theme().mono_font_family.clone();
        cx.set_global(DefaultMono(default_mono));

        let incoming = listener.map(|l| {
            let (tx, rx) = std::sync::mpsc::channel();
            ipc::serve(l, tx, Arc::new(|| {}));
            rx
        });
        let (plugin, plugin_status) = match TsPlugin::load() {
            Ok(p) => {
                let n = p.language_count();
                (Some(Rc::new(p)), PluginStatus::Loaded { languages: n })
            }
            Err(status) => (None, status),
        };
        let langs = Langs::new();
        let lang_names = langs.names();
        let menu_bar = AppMenuBar::new(cx);
        let update_status = cx.new(|_| SharedString::from(Self::initial_update_status()));
        let mut app = Self {
            settings,
            docs: Vec::new(),
            active: 0,
            next_id: 1,
            langs,
            lang_names,
            themes: Themes::new(),
            plugin,
            plugin_status,
            menu_bar,
            status: SharedString::default(),
            incoming,
            waiters: Vec::new(),
            quit_when_clean: false,
            force_quit: false,
            last_session: (Vec::new(), 0),
            bounds_generation: 0,
            window_title: String::new(),
            update_status,
            update_installed: None,
            update_running: false,
            relaunch_on_quit: false,
        };

        app.apply_theme(window, cx);
        Self::apply_font(&app.settings, cx);
        cx.observe_global_in::<SettingsGlobal>(window, |this, window, cx| {
            this.on_settings_changed(window, cx)
        })
        .detach();

        // Only the primary instance owns the saved session.
        if app.incoming.is_some() && app.settings.restore_session {
            let session = Session::load();
            for f in &session.files {
                let path = PathBuf::from(f);
                if path.is_file() {
                    app.open_path(path, window, cx);
                }
            }
            if let Some(i) = session.files.get(session.active).and_then(|f| {
                app.docs
                    .iter()
                    .position(|d| d.path.as_deref() == Some(Path::new(f)))
            }) {
                app.active = i;
            }
        }
        for f in cli.paths {
            if let (Some(id), true) = (app.open_path(f, window, cx), cli.wait) {
                app.waiters.push(Waiter {
                    doc_id: id,
                    stream: None,
                });
            }
        }
        if app.docs.is_empty() {
            app.new_doc(window, cx);
        }

        // Remember where the window is, so the next start puts it back.
        cx.observe_window_bounds(window, |this, window, cx| {
            this.schedule_window_state_save(window, cx)
        })
        .detach();

        // Start the installed version after quitting, when an update asked for a restart.
        cx.on_app_quit(|this, _| {
            if this.relaunch_on_quit {
                updater::relaunch();
            }
            async {}
        })
        .detach();
        app.start_update_checks(window, cx);
        app.repair_explorer_menu();

        // Closing the window with unsaved changes asks first.
        let this = cx.entity();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |app, cx| app.should_close_window(window, cx))
        });

        // Poll for files sent by later `nanote file...` launches.
        if app.incoming.is_some() {
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(150))
                        .await;
                    let alive = this
                        .update_in(cx, |this, window, cx| this.poll_incoming(window, cx))
                        .is_ok();
                    if !alive {
                        break;
                    }
                }
            })
            .detach();
        }
        app.focus_editor(window, cx);
        app.save_session();
        app
    }

    // ----------------------------------------------------------------- setup helpers

    /// Apply the theme mode (light/dark/system) and the syntax colour theme for that mode.
    fn apply_theme(&self, window: &mut Window, cx: &mut App) {
        match self.settings.theme_mode {
            ThemeMode::System => Theme::sync_system_appearance(Some(window), cx),
            ThemeMode::Light => Theme::change(GpuiThemeMode::Light, Some(window), cx),
            ThemeMode::Dark => Theme::change(GpuiThemeMode::Dark, Some(window), cx),
        }
        let dark = cx.theme().mode.is_dark();
        if let Some(theme) = self.themes.highlight_theme(self.settings.syntax_theme(dark)) {
            Theme::global_mut(cx).highlight_theme = theme;
        }
        cx.refresh_windows();
    }

    /// Change settings through the global so every part of the UI sees one source of truth.
    pub fn update_settings(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Settings)) {
        let mut next = cx.global::<SettingsGlobal>().0.clone();
        f(&mut next);
        cx.set_global(SettingsGlobal(next));
    }

    /// The settings global changed: apply the differences and persist.
    fn on_settings_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let new = cx.global::<SettingsGlobal>().0.clone();
        if new == self.settings {
            return;
        }
        let old = std::mem::replace(&mut self.settings, new);
        let s = &self.settings;
        if s.ui_language != old.ui_language {
            i18n::set_korean(s.korean());
        }
        if s.theme_mode != old.theme_mode
            || s.syntax_dark != old.syntax_dark
            || s.syntax_light != old.syntax_light
        {
            self.apply_theme(window, cx);
        }
        if s.font_family != old.font_family || s.font_size != old.font_size {
            Self::apply_font(&self.settings, cx);
        }
        if s.word_wrap != old.word_wrap {
            let wrap = s.word_wrap;
            for d in &self.docs {
                d.editor
                    .update(cx, |st, cx| st.set_soft_wrap(wrap, window, cx));
            }
        }
        if s.line_numbers != old.line_numbers {
            let on = s.line_numbers;
            for d in &self.docs {
                d.editor
                    .update(cx, |st, cx| st.set_line_number(on, window, cx));
            }
        }
        // The menu entry follows its switch, and its label follows the interface language.
        if self.settings.explorer_menu != old.explorer_menu
            || (self.settings.explorer_menu && self.settings.korean() != old.korean())
        {
            self.apply_explorer_menu(cx);
        }
        if self.settings.use_tree_sitter != old.use_tree_sitter {
            for idx in 0..self.docs.len() {
                let lang = self.language_of(&self.docs[idx], cx);
                if self.wants_plugin(&lang) != self.docs[idx].uses_plugin {
                    self.rebuild_editor(idx, window, cx);
                }
            }
        }
        if self.settings.ui_language != old.ui_language
            || self.settings.line_numbers != old.line_numbers
            || self.settings.word_wrap != old.word_wrap
        {
            self.reload_menus(cx);
        }
        self.settings.save();
        cx.notify();
    }

    fn apply_font(settings: &Settings, cx: &mut App) {
        let default_mono = cx.global::<DefaultMono>().0.clone();
        let theme = Theme::global_mut(cx);
        theme.mono_font_size = px(settings.font_size);
        if settings.font_family != crate::settings::DEFAULT_FONT {
            theme.mono_font_family = settings.font_family.clone().into();
        } else {
            theme.mono_font_family = default_mono;
        }
        cx.refresh_windows();
    }

    // ------------------------------------------------------------------- documents

    pub fn active_doc(&self) -> &Doc {
        &self.docs[self.active]
    }

    pub fn focus_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(doc) = self.docs.get(self.active) {
            let handle = doc.editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    /// Language shown/applied for a document.
    pub fn language_of(&self, doc: &Doc, cx: &App) -> String {
        doc.language.clone().unwrap_or_else(|| {
            let first = doc.editor.read(cx).text().to_string();
            let first_line = first.lines().next().unwrap_or("");
            self.langs.detect(doc.path.as_deref(), first_line)
        })
    }

    fn make_doc(
        &mut self,
        path: Option<PathBuf>,
        loaded: Loaded,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Doc {
        let id = self.next_id;
        self.next_id += 1;
        let first_line = loaded.text.lines().next().unwrap_or("").to_owned();
        let lang = self.langs.detect(path.as_deref(), &first_line);
        let (line_numbers, wrap) = (self.settings.line_numbers, self.settings.word_wrap);
        let text = loaded.text.clone();
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(lang.clone())
                .line_number(line_numbers)
                .soft_wrap(wrap)
                .default_value(text)
        });
        let uses_plugin = self.wants_plugin(&lang);
        if uses_plugin {
            self.install_plugin(&editor, cx);
        }
        let subscription = cx.subscribe(&editor, move |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.on_change(id, cx);
            }
        });
        Doc {
            id,
            path,
            editor,
            dirty: false,
            crlf: loaded.crlf,
            encoding: loaded.encoding,
            bom: loaded.bom,
            language: None,
            applied_language: lang,
            uses_plugin,
            occurrences: Vec::new(),
            occurrence_whole_word: false,
            column: None,
            ignore_change: false,
            _subscription: subscription,
        }
    }

    /// Whether the plugin should highlight `lang` (installed, enabled, language supported).
    pub fn wants_plugin(&self, lang: &str) -> bool {
        self.settings.use_tree_sitter && self.plugin.as_ref().is_some_and(|p| p.supports(lang))
    }

    fn install_plugin(&self, editor: &Entity<EditorState>, cx: &mut Context<Self>) {
        if let Some(plugin) = &self.plugin {
            let factory = plugin.factory();
            editor.update(cx, |st, cx| st.set_highlighter_factory(factory, cx));
        }
    }

    /// Re-create the editor of document `idx` with the right highlighter, keeping text and cursor.
    /// (An editor that got the plugin factory cannot go back to the built-in highlighter.)
    pub fn rebuild_editor(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (text, cursor, lang) = {
            let doc = &self.docs[idx];
            let st = doc.editor.read(cx);
            (st.value().to_string(), st.cursor_position(), self.language_of(doc, cx))
        };
        let (line_numbers, wrap) = (self.settings.line_numbers, self.settings.word_wrap);
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(lang.clone())
                .line_number(line_numbers)
                .soft_wrap(wrap)
                .default_value(text)
        });
        let uses_plugin = self.wants_plugin(&lang);
        if uses_plugin {
            self.install_plugin(&editor, cx);
        }
        editor.update(cx, |st, cx| st.set_cursor_position(cursor, window, cx));
        let id = self.docs[idx].id;
        let subscription = cx.subscribe(&editor, move |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.on_change(id, cx);
            }
        });
        let doc = &mut self.docs[idx];
        doc.editor = editor;
        doc.occurrences.clear();
        doc.column = None;
        doc.applied_language = lang;
        doc.uses_plugin = uses_plugin;
        doc._subscription = subscription;
        if idx == self.active {
            self.focus_editor(window, cx);
        }
        cx.notify();
    }

    fn on_change(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) {
            // Offsets remembered by the multi-cursor commands are stale after an edit.
            doc.occurrences.clear();
            doc.column = None;
            if doc.ignore_change {
                doc.ignore_change = false;
            } else {
                doc.dirty = true;
            }
        }
        cx.notify();
    }

    pub fn new_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let loaded = Loaded {
            text: String::new(),
            encoding: "UTF-8".into(),
            bom: false,
            crlf: false,
        };
        let doc = self.make_doc(None, loaded, window, cx);
        self.docs.push(doc);
        self.active = self.docs.len() - 1;
        self.focus_editor(window, cx);
        self.save_session();
        cx.notify();
    }

    /// Open `path` in a tab (or switch to it). Returns the document id.
    pub fn open_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        if let Some(i) = self
            .docs
            .iter()
            .position(|d| d.path.as_deref() == Some(path.as_path()))
        {
            self.active = i;
            // Without this the focus stays in the editor of the tab that was just left (or gets
            // lost when a dialog closes), and keyboard shortcuts stop reaching the app.
            self.focus_editor(window, cx);
            self.save_session();
            cx.notify();
            return Some(self.docs[i].id);
        }
        // A path that doesn't exist yet opens as an empty document that is created on save.
        let read = match read_file(&path, None) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Loaded {
                text: String::new(),
                encoding: "UTF-8".into(),
                bom: false,
                crlf: false,
            }),
            other => other,
        };
        match read {
            Ok(loaded) => {
                // Reuse a pristine empty "Untitled" tab instead of piling up tabs.
                let reuse = self.docs.len() == 1
                    && self.docs[0].path.is_none()
                    && !self.docs[0].dirty
                    && self.docs[0].editor.read(cx).text().len() == 0;
                let doc = self.make_doc(Some(path), loaded, window, cx);
                let id = doc.id;
                if reuse {
                    self.docs[0] = doc;
                    self.active = 0;
                } else {
                    self.docs.push(doc);
                    self.active = self.docs.len() - 1;
                }
                self.focus_editor(window, cx);
                self.save_session();
                cx.notify();
                Some(id)
            }
            Err(e) => {
                self.status = format!("{} {}: {e}", tr("Failed to open"), path.display()).into();
                cx.notify();
                None
            }
        }
    }

    pub fn open_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update_in(cx, |this, window, cx| {
                    for p in paths {
                        this.open_path(p, window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// Save document `idx`; asks for a path when it has none (or `as_new`).
    pub fn save(&mut self, idx: usize, as_new: bool, window: &mut Window, cx: &mut Context<Self>) {
        match (&self.docs[idx].path, as_new) {
            (Some(p), false) => {
                let p = p.clone();
                self.write_doc(idx, p, cx);
            }
            _ => {
                let dir = self.docs[idx]
                    .path
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                let name = self.docs[idx].title();
                let id = self.docs[idx].id;
                let rx = cx.prompt_for_new_path(&dir, Some(&name));
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(Ok(Some(path))) = rx.await {
                        this.update_in(cx, |this, _, cx| {
                            if let Some(idx) = this.docs.iter().position(|d| d.id == id) {
                                this.write_doc(idx, path, cx);
                            }
                        })
                        .ok();
                    }
                })
                .detach();
            }
        }
    }

    /// Encode and write; returns whether it succeeded.
    fn write_doc(&mut self, idx: usize, path: PathBuf, cx: &mut Context<Self>) -> bool {
        let text = self.docs[idx].editor.read(cx).value().to_string();
        let doc = &self.docs[idx];
        let data = match encode_text(&text, &doc.encoding, doc.bom, doc.crlf) {
            Ok(data) => data,
            Err(e) => {
                self.status = e.into();
                cx.notify();
                return false;
            }
        };
        match std::fs::write(&path, data) {
            Ok(()) => {
                let doc = &mut self.docs[idx];
                doc.path = Some(path);
                doc.dirty = false;
                self.status = tr("Saved").into();
                self.save_session();
                cx.notify();
                true
            }
            Err(e) => {
                self.status = format!("{}: {e}", tr("Failed to save")).into();
                cx.notify();
                false
            }
        }
    }

    /// Re-read the document's file from disk using the given encoding.
    pub fn reopen_with(
        &mut self,
        idx: usize,
        encoding: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.docs[idx].path.clone() else {
            self.status = tr("Save the file first to reopen it with another encoding").into();
            cx.notify();
            return;
        };
        match read_file(&path, Some(encoding)) {
            Ok(loaded) => {
                let doc = &mut self.docs[idx];
                doc.ignore_change = true;
                doc.crlf = loaded.crlf;
                doc.encoding = loaded.encoding;
                doc.bom = loaded.bom;
                doc.dirty = false;
                let text = loaded.text;
                doc.editor
                    .update(cx, |st, cx| st.set_value(text, window, cx));
                self.status = format!("{} {encoding}", tr("Reopened as")).into();
            }
            Err(e) => self.status = format!("{}: {e}", tr("Failed to reopen")).into(),
        }
        cx.notify();
    }

    pub fn request_reopen(&mut self, encoding: String, window: &mut Window, cx: &mut Context<Self>) {
        let idx = self.active;
        if self.docs[idx].path.is_none() {
            self.status = tr("Save the file first to reopen it with another encoding").into();
            cx.notify();
        } else if self.docs[idx].dirty {
            let name = self.docs[idx].title();
            let rx = window.prompt(
                PromptLevel::Warning,
                tr("Unsaved changes"),
                Some(&format!("{} {name}", tr("Discard unsaved changes to:"))),
                &[tr("Discard"), tr("Cancel")],
                cx,
            );
            let id = self.docs[idx].id;
            cx.spawn_in(window, async move |this, cx| {
                if let Ok(0) = rx.await {
                    this.update_in(cx, |this, window, cx| {
                        if let Some(idx) = this.docs.iter().position(|d| d.id == id) {
                            this.reopen_with(idx, &encoding, window, cx);
                        }
                    })
                    .ok();
                }
            })
            .detach();
        } else {
            self.reopen_with(idx, &encoding, window, cx);
        }
    }

    pub fn select_tab(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        if idx < self.docs.len() {
            self.active = idx;
            self.focus_editor(window, cx);
            self.save_session();
            cx.notify();
        }
    }

    /// Close a tab; asks first when it has unsaved changes.
    pub fn request_close(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.docs[idx].dirty {
            self.close_tab(idx, window, cx);
            return;
        }
        self.active = idx;
        let name = self.docs[idx].title();
        let id = self.docs[idx].id;
        let rx = window.prompt(
            PromptLevel::Warning,
            tr("Unsaved changes"),
            Some(&format!("{} {name}", tr("Discard unsaved changes to:"))),
            &[tr("Save"), tr("Discard"), tr("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = rx.await else { return };
            this.update_in(cx, |this, window, cx| {
                let Some(idx) = this.docs.iter().position(|d| d.id == id) else {
                    return;
                };
                match choice {
                    0 => {
                        // Save, then close once the write succeeded.
                        if this.docs[idx].path.is_some() {
                            let p = this.docs[idx].path.clone().unwrap();
                            if this.write_doc(idx, p, cx) {
                                this.close_tab(idx, window, cx);
                            }
                        } else {
                            this.save(idx, false, window, cx);
                        }
                    }
                    1 => this.close_tab(idx, window, cx),
                    _ => {}
                }
            })
            .ok();
        })
        .detach();
    }

    fn close_tab(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let closed = self.docs.remove(idx);
        self.release_waiters(closed.id);
        if self.docs.is_empty() {
            self.new_doc(window, cx);
        }
        if self.active >= self.docs.len() || idx < self.active {
            self.active = self.active.saturating_sub(1).min(self.docs.len() - 1);
        }
        self.focus_editor(window, cx);
        self.save_session();
        // A `--wait` window that was opened just for an editing request ends with it.
        if self.quit_when_clean {
            let pristine = self
                .docs
                .iter()
                .all(|d| d.path.is_none() && !d.dirty && d.editor.read(cx).text().len() == 0);
            if pristine {
                self.force_quit = true;
                cx.quit();
            }
        }
        cx.notify();
    }

    /// Called when the OS window is closed. Returns whether it may close.
    fn should_close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.save_window_state(window);
        if self.force_quit || !self.docs.iter().any(|d| d.dirty) {
            return true;
        }
        let names: Vec<String> = self
            .docs
            .iter()
            .filter(|d| d.dirty)
            .map(Doc::title)
            .collect();
        let rx = window.prompt(
            PromptLevel::Warning,
            tr("Unsaved changes"),
            Some(&format!(
                "{} {}",
                tr("Discard unsaved changes to:"),
                names.join(", ")
            )),
            &[tr("Discard"), tr("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                this.update(cx, |this, cx| {
                    this.force_quit = true;
                    cx.quit();
                })
                .ok();
            } else {
                // Cancelled: a pending "restart to update" is cancelled with it.
                this.update(cx, |this, _| this.relaunch_on_quit = false).ok();
            }
        })
        .detach();
        false
    }

    // ------------------------------------------------------------------ window state

    /// Write the window state (primary instance only, like the session).
    fn save_window_state(&self, window: &Window) {
        if self.incoming.is_none() {
            return;
        }
        let state = window_state_of(window);
        if state.is_sane() {
            state.save();
        }
    }

    /// Save shortly after the last move/resize instead of on every event of a drag.
    fn schedule_window_state_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.incoming.is_none() {
            return;
        }
        self.bounds_generation += 1;
        let generation = self.bounds_generation;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            this.update_in(cx, |this, window, _| {
                if this.bounds_generation == generation {
                    this.save_window_state(window);
                }
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------- single instance / session

    fn poll_incoming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rx) = &self.incoming else { return };
        let requests: Vec<ipc::Incoming> = rx.try_iter().collect();
        for req in requests {
            let mut stream = Some(req.stream);
            for p in req.paths {
                if let Some(id) = self.open_path(p, window, cx) {
                    // Only the first file of a `--wait` request gates the caller.
                    if let (true, Some(s)) = (req.wait, stream.take()) {
                        self.waiters.push(Waiter {
                            doc_id: id,
                            stream: Some(s),
                        });
                    }
                }
            }
            window.activate_window();
        }
    }

    /// Tell anyone waiting on this document that it was closed.
    fn release_waiters(&mut self, doc_id: u64) {
        let mut own = false;
        self.waiters.retain_mut(|w| {
            if w.doc_id != doc_id {
                return true;
            }
            match &mut w.stream {
                Some(s) => {
                    let _ = s.write_all(b"DONE\n");
                }
                None => own = true,
            }
            false
        });
        if own && !self.waiters.iter().any(|w| w.stream.is_none()) {
            self.quit_when_clean = true;
        }
    }

    /// Persist the list of open files whenever it changes (primary instance only).
    pub fn save_session(&mut self) {
        if self.incoming.is_none() || self.docs.is_empty() {
            return;
        }
        let files: Vec<String> = self
            .docs
            .iter()
            .filter_map(|d| d.path.as_ref().map(|p| p.to_string_lossy().into_owned()))
            .collect();
        let active = self.docs[self.active.min(self.docs.len() - 1)]
            .path
            .as_ref()
            .and_then(|p| files.iter().position(|f| Path::new(f) == p.as_path()))
            .unwrap_or(0);
        if self.last_session != (files.clone(), active) {
            Session {
                files: files.clone(),
                active,
            }
            .save();
            self.last_session = (files, active);
        }
    }
}

/// The current window state, as stored in `window.json`.
fn window_state_of(window: &Window) -> WindowState {
    let bounds = window.window_bounds();
    let b = bounds.get_bounds();
    WindowState {
        x: f32::from(b.origin.x),
        y: f32::from(b.origin.y),
        width: f32::from(b.size.width),
        height: f32::from(b.size.height),
        mode: match bounds {
            WindowBounds::Windowed(_) => WindowMode::Normal,
            WindowBounds::Maximized(_) => WindowMode::Maximized,
            WindowBounds::Fullscreen(_) => WindowMode::Fullscreen,
        },
    }
}

/// The window bounds saved last time, if they are still usable on the current displays.
pub fn saved_window_bounds(cx: &App) -> Option<WindowBounds> {
    let state = WindowState::load().filter(WindowState::is_sane)?;
    let displays: Vec<(f32, f32, f32, f32)> = cx
        .displays()
        .iter()
        .map(|d| {
            let b = d.bounds();
            (
                f32::from(b.origin.x),
                f32::from(b.origin.y),
                f32::from(b.size.width),
                f32::from(b.size.height),
            )
        })
        .collect();
    if !state.is_on_screen(&displays) {
        return None;
    }
    let bounds = Bounds::new(
        point(px(state.x), px(state.y)),
        size(px(state.width), px(state.height)),
    );
    Some(match state.mode {
        WindowMode::Normal => WindowBounds::Windowed(bounds),
        WindowMode::Maximized => WindowBounds::Maximized(bounds),
        WindowMode::Fullscreen => WindowBounds::Fullscreen(bounds),
    })
}

/// Title bar text for the window: the active file, then the app name (`file_a.yml - nanote`).
pub fn window_title_for(file_name: &str) -> String {
    format!("{file_name} - nanote")
}

#[cfg(test)]
mod title_tests {
    use super::window_title_for;

    #[test]
    fn names_the_file_then_the_app() {
        assert_eq!(window_title_for("file_a.yml"), "file_a.yml - nanote");
        assert_eq!(window_title_for("한글 파일.txt"), "한글 파일.txt - nanote");
    }
}
