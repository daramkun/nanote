//! `Ctrl+P`: open a file by typing its absolute or relative path, with completion.

use std::path::{MAIN_SEPARATOR, PathBuf};

use gpui_kit::component::{
    ActiveTheme as _, IconName, WindowExt as _,
    command::{Command, CommandItem, CommandState},
};
use gpui_kit::*;

use super::*;
use crate::quick_open::{self, Kind};

actions!(nanote, [QuickOpen]);

/// The content of the dialog. It is a view of its own so that every change of the query
/// recomputes the candidates: the dialog's builder only runs when the dialog layer re-renders.
struct QuickOpenView {
    app: Entity<NanoteApp>,
    state: Entity<CommandState>,
    /// Folders relative paths are resolved against, in order.
    bases: Vec<PathBuf>,
}

impl Render for QuickOpenView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.state.read(cx).query(cx).to_string();
        let completion = quick_open::complete(&query, &self.bases, &|name| std::env::var(name).ok());

        let items: Vec<CommandItem> = completion
            .candidates
            .iter()
            .map(|c| match c.kind {
                Kind::Dir => CommandItem::new()
                    .label(format!("{}{MAIN_SEPARATOR}", c.name))
                    .icon(IconName::Folder),
                Kind::File => CommandItem::new().label(c.name.clone()).icon(IconName::File),
                Kind::NewFile => CommandItem::new()
                    .label(format!("{} {}", tr("New file:"), c.name))
                    .icon(IconName::Plus),
            })
            .collect();

        let candidates = completion.candidates;
        let typed_dir = completion.typed_dir;
        let (app, state, this) = (self.app.clone(), self.state.clone(), cx.entity());
        // The folder being listed shows how a relative path was resolved.
        let listed = match completion.dir {
            Some(dir) => format!("{} {}", tr("Folder:"), dir.display()),
            None => tr("No such folder").to_owned(),
        };

        Command::new(&self.state)
            .items(items)
            // The candidates are already narrowed down by the typed text.
            .filterable(false)
            .bordered(false)
            .placeholder(tr("Type a path (absolute or relative)..."))
            .header(move |_, _, cx| {
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(listed.clone())
            })
            .empty(|_, _, cx| {
                div()
                    .p_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr("No matching path"))
            })
            .on_query(move |_, _, cx| this.update(cx, |_, cx| cx.notify()))
            .on_confirm(move |index, window, cx| {
                let Some(choice) = candidates.get(index.row).cloned() else {
                    return;
                };
                match choice.kind {
                    // Descend into the folder and keep the dialog open.
                    Kind::Dir => {
                        let next = format!("{typed_dir}{}{MAIN_SEPARATOR}", choice.name);
                        state.update(cx, |state, cx| state.set_query(next, window, cx));
                    }
                    Kind::File | Kind::NewFile => {
                        window.close_dialog(cx);
                        app.update(cx, |app, cx| {
                            app.open_path(choice.path.clone(), window, cx);
                        });
                    }
                }
            })
    }
}

impl NanoteApp {
    pub fn open_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Relative paths start at the folder of the file in the active tab.
        let bases = quick_open::bases_for(
            self.active_doc().path.as_deref(),
            std::env::current_dir().ok(),
        );

        let app = cx.entity();
        let state = cx.new(|cx| CommandState::new(window, cx));
        let view = cx.new(|_| QuickOpenView {
            app,
            state: state.clone(),
            bases,
        });
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.w(px(640.)).child(view.clone())
        });
        state.update(cx, |state, cx| state.focus(window, cx));
    }
}
