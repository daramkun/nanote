//! Persistent user settings and the saved session (list of open files).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const DEFAULT_FONT: &str = "(Default monospace)";
pub const DEFAULT_DARK_SYNTAX: &str = "base16-ocean.dark";
pub const DEFAULT_LIGHT_SYNTAX: &str = "GitHub";

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum UiLang {
    System,
    English,
    Korean,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub ui_language: UiLang,
    /// Reopen the files that were open when the app was last closed.
    pub restore_session: bool,
    /// Look for new releases in the background and install them.
    pub auto_update: bool,
    /// The Explorer context menu entry "Open with nanote" is installed.
    pub explorer_menu: bool,
    /// Prefer tree-sitter highlighting (plugin) over syntect for languages it supports.
    pub use_tree_sitter: bool,
    /// Open files given on the command line in a new window instead of a tab of a running one.
    pub new_window: bool,
    pub font_family: String,
    pub font_size: f32,
    pub theme_mode: ThemeMode,
    pub syntax_dark: String,
    pub syntax_light: String,
    pub word_wrap: bool,
    pub line_numbers: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ui_language: UiLang::System,
            restore_session: true,
            auto_update: true,
            explorer_menu: false,
            use_tree_sitter: true,
            new_window: false,
            font_family: DEFAULT_FONT.into(),
            font_size: 15.0,
            theme_mode: ThemeMode::System,
            syntax_dark: DEFAULT_DARK_SYNTAX.into(),
            syntax_light: DEFAULT_LIGHT_SYNTAX.into(),
            word_wrap: false,
            line_numbers: true,
        }
    }
}

impl Settings {
    /// Whether the interface is shown in Korean (the choice, or the system language).
    pub fn korean(&self) -> bool {
        match self.ui_language {
            UiLang::System => crate::i18n::system_is_korean(),
            UiLang::English => false,
            UiLang::Korean => true,
        }
    }

    pub fn syntax_theme(&self, dark: bool) -> &str {
        if dark {
            &self.syntax_dark
        } else {
            &self.syntax_light
        }
    }

    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("APPDATA")
            .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("nanote").join("settings.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, s);
            }
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Session {
    pub files: Vec<String>,
    pub active: usize,
}

impl Session {
    pub fn path() -> Option<PathBuf> {
        Settings::path().map(|p| p.with_file_name("session.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string(self) {
                let _ = std::fs::write(p, s);
            }
        }
    }
}

/// How the window was left when the app was last closed.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowMode {
    Normal,
    Maximized,
    Fullscreen,
}

/// Window position, size (of the restored, non-maximized window) and state, in logical pixels.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub mode: WindowMode,
}

impl WindowState {
    pub fn path() -> Option<PathBuf> {
        Settings::path().map(|p| p.with_file_name("window.json"))
    }

    pub fn load() -> Option<Self> {
        let text = std::fs::read_to_string(Self::path()?).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string(self) {
                let _ = std::fs::write(p, s);
            }
        }
    }

    /// A state worth saving and restoring: finite, not tiny, not a minimized-window placeholder.
    pub fn is_sane(&self) -> bool {
        let finite = [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite());
        finite
            && self.width >= 200.0
            && self.height >= 150.0
            && self.x > -10_000.0
            && self.y > -10_000.0
    }

    /// Whether enough of the window lies on one of the `displays` (x, y, width, height) for the
    /// user to reach it. Guards against restoring onto a monitor that is no longer there.
    pub fn is_on_screen(&self, displays: &[(f32, f32, f32, f32)]) -> bool {
        const MIN_VISIBLE_W: f32 = 120.0;
        const MIN_VISIBLE_H: f32 = 60.0;
        displays.iter().any(|&(dx, dy, dw, dh)| {
            let w = (self.x + self.width).min(dx + dw) - self.x.max(dx);
            let h = (self.y + self.height).min(dy + dh) - self.y.max(dy);
            w >= MIN_VISIBLE_W && h >= MIN_VISIBLE_H
        })
    }
}

#[cfg(test)]
mod window_state_tests {
    use super::*;

    fn state(x: f32, y: f32, w: f32, h: f32) -> WindowState {
        WindowState {
            x,
            y,
            width: w,
            height: h,
            mode: WindowMode::Normal,
        }
    }

    #[test]
    fn round_trips_through_json() {
        let s = WindowState {
            mode: WindowMode::Maximized,
            ..state(10.0, 20.0, 800.0, 600.0)
        };
        let back: WindowState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn rejects_degenerate_states() {
        assert!(state(0.0, 0.0, 800.0, 600.0).is_sane());
        assert!(!state(0.0, 0.0, 50.0, 600.0).is_sane());
        assert!(!state(-32000.0, -32000.0, 800.0, 600.0).is_sane()); // minimized placeholder
        assert!(!state(f32::NAN, 0.0, 800.0, 600.0).is_sane());
    }

    #[test]
    fn detects_windows_that_are_off_every_display() {
        let displays = [(0.0, 0.0, 1920.0, 1080.0), (1920.0, 0.0, 1280.0, 1024.0)];
        assert!(state(100.0, 100.0, 800.0, 600.0).is_on_screen(&displays));
        // Straddling two monitors is fine.
        assert!(state(1500.0, 100.0, 800.0, 600.0).is_on_screen(&displays));
        // A monitor that was unplugged: the window sits at x = 3500.
        assert!(!state(3500.0, 100.0, 800.0, 600.0).is_on_screen(&displays));
        // Only a sliver at the edge is visible: not reachable enough.
        assert!(!state(1900.0, 100.0, 800.0, 600.0).is_on_screen(&displays[..1]));
        // Nothing to compare against (no displays reported): do not trust the saved place.
        assert!(!state(0.0, 0.0, 800.0, 600.0).is_on_screen(&[]));
    }
}
