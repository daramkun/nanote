//! Keeps the Explorer context-menu entry in line with its setting.

use gpui_kit::*;

use super::*;
use crate::{file_assoc, shell_menu};

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

    /// "Set as default app": register nanote for its file types, then show Windows' Default
    /// apps page, where choosing nanote for them is up to the user.
    pub(super) fn register_file_types(&mut self, group: file_assoc::Group, cx: &mut Context<Self>) {
        let result = std::env::current_exe()
            .and_then(|exe| file_assoc::register(&exe, tr("Open with nanote"), group))
            .and_then(|_| file_assoc::open_default_apps());
        if let Err(error) = result {
            self.status = format!("{}: {error}", tr("Could not register the file types")).into();
            cx.notify();
        }
    }

    /// Remove the file type registration again.
    pub(super) fn unregister_file_types(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = file_assoc::unregister() {
            self.status = format!("{}: {error}", tr("Could not register the file types")).into();
            cx.notify();
        }
    }

    /// At start, while the entry is switched on, correct it if it points elsewhere (the program
    /// was moved) or carries an old label. Only the primary instance does this.
    pub(super) fn repair_explorer_menu(&mut self) {
        if self.incoming.is_none() {
            return;
        }
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        if self.settings.explorer_menu {
            let _ = shell_menu::ensure(tr("Open with nanote"), &exe);
        }
        // Registered file types follow the program when it is moved.
        // (The group is not kept: re-adding the paths of the old registration is enough.)
        if file_assoc::registered_command().is_some_and(|c| c != file_assoc::command_for(&exe)) {
            let _ = file_assoc::repoint(&exe);
        }
    }
}
