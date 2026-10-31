//! Keeps the Explorer context-menu entry in line with its setting.

use gpui_kit::*;

use super::*;
use crate::shell_menu;

impl NanoteApp {
    /// The setting was switched (or the language changed while it is on): write or remove the
    /// registry entry.
    pub(super) fn apply_explorer_menu(&mut self, cx: &mut Context<Self>) {
        let result = if self.settings.explorer_menu {
            std::env::current_exe()
                .and_then(|exe| shell_menu::ensure(tr("Open with nanote"), &exe).map(|_| ()))
        } else {
            shell_menu::unregister()
        };
        if let Err(error) = result {
            self.status = format!("{}: {error}", tr("Could not update the Explorer menu")).into();
            cx.notify();
        }
    }

    /// At start, while the entry is switched on, correct it if it points elsewhere (the program
    /// was moved) or carries an old label. Only the primary instance does this.
    pub(super) fn repair_explorer_menu(&mut self) {
        if !self.settings.explorer_menu || self.incoming.is_none() {
            return;
        }
        if let Ok(exe) = std::env::current_exe() {
            let _ = shell_menu::ensure(tr("Open with nanote"), &exe);
        }
    }
}
