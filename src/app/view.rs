//! Menus, tab bar, editor area and status bar.

use gpui_kit::component::{
    ActiveTheme as _, GlobalState, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    input::{Copy, Cut, Editor, Paste, Redo, Replace, Search, SelectAll, Undo},
    menu::{DropdownMenu as _, PopupMenuItem},
    status_bar::StatusBar,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::*;

use super::*;
use crate::encoding::ENCODINGS;

fn item(name: &str, action: impl Action) -> MenuItem {
    MenuItem::action(name.to_owned(), action)
}

fn checked_item(name: &str, action: impl Action, checked: bool) -> MenuItem {
    MenuItem::Action {
        name: name.to_owned().into(),
        action: Box::new(action),
        os_action: None,
        checked,
        disabled: false,
    }
}

impl NanoteApp {
    /// (Re)build the in-window menu bar contents from the current settings and language.
    pub(super) fn set_menus(settings: &Settings, cx: &mut App) {
        let menus = vec![
            Menu::new(tr("File")).items([
                item(tr("New"), NewFile),
                item(tr("Open..."), OpenFile),
                item(tr("Save"), Save),
                item(tr("Save As..."), SaveAs),
                MenuItem::separator(),
                item(tr("Close Tab"), CloseTab),
                item(tr("Exit"), Quit),
            ]),
            Menu::new(tr("Edit")).items([
                item(tr("Undo"), Undo),
                item(tr("Redo"), Redo),
                MenuItem::separator(),
                item(tr("Cut"), Cut),
                item(tr("Copy"), Copy),
                item(tr("Paste"), Paste),
                item(tr("Select All"), SelectAll),
                MenuItem::separator(),
                item(tr("Find..."), Search),
                item(tr("Replace..."), Replace),
                MenuItem::separator(),
                item(tr("Add Next Occurrence"), SelectNextOccurrence),
                item(tr("Skip Occurrence"), SkipOccurrence),
                item(tr("Select All Occurrences"), SelectAllOccurrences),
                item(tr("Add Cursors to Line Ends"), AddCursorsToLineEnds),
            ]),
            Menu::new(tr("View")).items([
                checked_item(tr("Line Numbers"), ToggleLineNumbers, settings.line_numbers),
                checked_item(tr("Word Wrap"), ToggleWrap, settings.word_wrap),
                MenuItem::separator(),
                item(tr("Zoom In"), ZoomIn),
                item(tr("Zoom Out"), ZoomOut),
                item(tr("Reset Zoom"), ZoomReset),
            ]),
            Menu::new(tr("Settings")).items([item(tr("Settings..."), OpenSettings)]),
        ];
        GlobalState::global_mut(cx)
            .set_app_menus(menus.into_iter().map(Menu::owned).collect());
    }

    pub(super) fn reload_menus(&mut self, cx: &mut Context<Self>) {
        Self::set_menus(&self.settings, cx);
        self.menu_bar.update(cx, |bar, cx| bar.reload(cx));
    }

    // ---------------------------------------------------------------- action handlers

    fn on_zoom(&mut self, delta: f32, reset: bool, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| {
            s.font_size = if reset {
                Settings::default().font_size
            } else {
                (s.font_size + delta).clamp(6.0, 96.0)
            }
        });
    }

    fn toggle_wrap(&mut self, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.word_wrap = !s.word_wrap);
    }

    fn toggle_line_numbers(&mut self, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.line_numbers = !s.line_numbers);
    }

    fn step_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.docs.len() as isize;
        let next = (self.active as isize + delta).rem_euclid(n) as usize;
        self.select_tab(next, window, cx);
    }

    fn set_language(&mut self, lang: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let idx = self.active;
        self.docs[idx].language = lang;
        let name = self.language_of(&self.docs[idx], cx);
        if self.docs[idx].applied_language == name {
            cx.notify();
            return;
        }
        if self.wants_plugin(&name) != self.docs[idx].uses_plugin {
            // Switching between the plugin and the built-in highlighter needs a fresh editor.
            self.rebuild_editor(idx, window, cx);
        } else {
            let doc = &mut self.docs[idx];
            doc.applied_language = name.clone();
            doc.editor.update(cx, |st, cx| st.set_highlighter(name, cx));
        }
        cx.notify();
    }

    // ------------------------------------------------------------------- view parts

    fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.docs.iter().enumerate().map(|(i, d)| {
            let title = format!("{}{}", d.title(), if d.dirty { " ●" } else { "" });
            Tab::new().label(title).suffix(
                Button::new(("close-tab", i))
                    .ghost()
                    .xsmall()
                    .label("×")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.request_close(i, window, cx);
                    })),
            )
        });
        TabBar::new("tabs")
            .selected_index(self.active)
            .on_click(cx.listener(|this, ix: &usize, window, cx| {
                this.select_tab(*ix, window, cx);
            }))
            .children(tabs)
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let doc = self.active_doc();
        let (pos, chars) = {
            let st = doc.editor.read(cx);
            (st.cursor_position(), st.text().len_chars())
        };
        let app = cx.entity();
        let lang = self.language_of(doc, cx);
        let enc_label = if doc.bom {
            format!("{} BOM", doc.encoding)
        } else {
            doc.encoding.clone()
        };
        let eol_label = if doc.crlf { "CRLF" } else { "LF" };

        // Language picker (all languages, scrollable).
        let lang_button = {
            let app = app.clone();
            let names = self.lang_names.clone();
            let current = lang.clone();
            Button::new("status-lang")
                .ghost()
                .xsmall()
                .label(lang)
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu.scrollable(true).max_h(px(420.));
                    {
                        let app = app.clone();
                        menu = menu.item(PopupMenuItem::new(tr("Auto-detect")).on_click(
                            move |_, window, cx| app.update(cx, |a, cx| a.set_language(None, window, cx)),
                        ));
                    }
                    menu = menu.separator();
                    for name in names.iter().cloned() {
                        let app = app.clone();
                        let checked = name == current;
                        let n = name.clone();
                        menu = menu.item(PopupMenuItem::new(name).checked(checked).on_click(
                            move |_, window, cx| {
                                let n = n.clone();
                                app.update(cx, |a, cx| a.set_language(Some(n), window, cx))
                            },
                        ));
                    }
                    menu
                })
        };

        // Line ending picker.
        let eol_button = {
            let app = app.clone();
            let crlf = doc.crlf;
            Button::new("status-eol")
                .ghost()
                .xsmall()
                .label(eol_label)
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for (label, value) in [("LF (Unix)", false), ("CRLF (Windows)", true)] {
                        let app = app.clone();
                        menu = menu.item(PopupMenuItem::new(tr(label)).checked(crlf == value).on_click(
                            move |_, _, cx| {
                                app.update(cx, |a, cx| {
                                    let idx = a.active;
                                    a.docs[idx].crlf = value;
                                    a.docs[idx].dirty = true;
                                    cx.notify();
                                })
                            },
                        ));
                    }
                    menu
                })
        };

        // Encoding picker: reopen / save with an encoding, BOM handling.
        let enc_button = {
            let app = app.clone();
            let (current, bom) = (doc.encoding.clone(), doc.bom);
            let can_bom = crate::encoding::bom_bytes(&current).is_some();
            Button::new("status-enc")
                .ghost()
                .xsmall()
                .label(enc_label)
                .dropdown_menu(move |menu, window, cx| {
                    let (app_a, app_b, app_c, app_d) =
                        (app.clone(), app.clone(), app.clone(), app.clone());
                    let (cur_a, cur_b) = (current.clone(), current.clone());
                    menu.submenu(tr("Reopen with Encoding"), window, cx, move |m, _, _| {
                        let mut m = m.scrollable(true).max_h(px(420.));
                        for &name in ENCODINGS {
                            let app = app_a.clone();
                            m = m.item(PopupMenuItem::new(name).checked(cur_a == name).on_click(
                                move |_, window, cx| {
                                    app.update(cx, |a, cx| {
                                        a.request_reopen(name.to_owned(), window, cx)
                                    })
                                },
                            ));
                        }
                        m
                    })
                    .submenu(tr("Save with Encoding"), window, cx, move |m, _, _| {
                        let mut m = m.scrollable(true).max_h(px(420.));
                        for &name in ENCODINGS {
                            let app = app_b.clone();
                            m = m.item(PopupMenuItem::new(name).checked(cur_b == name).on_click(
                                move |_, window, cx| {
                                    app.update(cx, |a, cx| a.save_with_encoding(name, window, cx))
                                },
                            ));
                        }
                        m
                    })
                    .separator()
                    .item(
                        PopupMenuItem::new(tr("Write BOM on save"))
                            .checked(bom)
                            .disabled(!can_bom)
                            .on_click(move |_, _, cx| {
                                app_c.update(cx, |a, cx| {
                                    let idx = a.active;
                                    a.docs[idx].bom = !a.docs[idx].bom;
                                    a.docs[idx].dirty = true;
                                    cx.notify();
                                })
                            }),
                    )
                    .item(
                        PopupMenuItem::new(tr("Remove BOM and Save"))
                            .disabled(!(can_bom && bom))
                            .on_click(move |_, window, cx| {
                                app_d.update(cx, |a, cx| {
                                    let idx = a.active;
                                    a.docs[idx].bom = false;
                                    a.docs[idx].dirty = true;
                                    a.save(idx, false, window, cx);
                                })
                            }),
                    )
                })
        };

        let wrap = Checkbox::new("status-wrap")
            .label(tr("Wrap"))
            .checked(self.settings.word_wrap)
            .on_click(cx.listener(|this, _, _, cx| this.toggle_wrap(cx)));

        StatusBar::new()
            .left(format!(
                "{} {}, {} {}",
                tr("Ln"),
                pos.line + 1,
                tr("Col"),
                pos.character + 1
            ))
            .left(format!("{chars} {}", tr("chars")))
            .left(self.status.clone())
            .right(lang_button)
            .right(eol_button)
            .right(enc_button)
            .right(wrap)
    }

    /// Switch the document's encoding and save it right away; revert if saving fails.
    pub fn save_with_encoding(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let idx = self.active;
        let (old, old_bom) = {
            let d = &mut self.docs[idx];
            let old = std::mem::replace(&mut d.encoding, name.to_owned());
            let old_bom = d.bom;
            // Unicode encodings keep an existing BOM; legacy ones never have one.
            d.bom &= crate::encoding::bom_bytes(name).is_some();
            d.dirty = true;
            (old, old_bom)
        };
        match self.docs[idx].path.clone() {
            Some(path) => {
                if !self.write_doc(idx, path, cx) {
                    let d = &mut self.docs[idx];
                    d.encoding = old;
                    d.bom = old_bom;
                }
            }
            None => self.save(idx, false, window, cx),
        }
    }
}

impl Render for NanoteApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Every change of the active tab (switching, opening, closing, saving under a new name)
        // re-renders, so this is the one place that keeps the title bar in sync.
        let title = window_title_for(&self.active_doc().title());
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }
        let editor = self.active_doc().editor.clone();
        let border = cx.theme().border;
        v_flex()
            .size_full()
            .key_context("Nanote")
            .on_action(cx.listener(|this, _: &NewFile, window, cx| this.new_doc(window, cx)))
            .on_action(cx.listener(|this, _: &OpenFile, window, cx| this.open_dialog(window, cx)))
            .on_action(cx.listener(|this, _: &Save, window, cx| {
                let idx = this.active;
                this.save(idx, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SaveAs, window, cx| {
                let idx = this.active;
                this.save(idx, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                let idx = this.active;
                this.request_close(idx, window, cx)
            }))
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                if this.should_close_window(window, cx) {
                    cx.quit();
                }
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.step_tab(1, window, cx)))
            .on_action(cx.listener(|this, _: &PrevTab, window, cx| this.step_tab(-1, window, cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.on_zoom(1.0, false, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.on_zoom(-1.0, false, cx)))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| this.on_zoom(0.0, true, cx)))
            .on_action(cx.listener(|this, _: &ToggleWrap, _, cx| this.toggle_wrap(cx)))
            .on_action(cx.listener(|this, _: &ToggleLineNumbers, _, cx| this.toggle_line_numbers(cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)))
            .on_action(cx.listener(|this, _: &SelectNextOccurrence, _, cx| {
                this.select_next_occurrence(cx)
            }))
            .on_action(cx.listener(|this, _: &SkipOccurrence, _, cx| this.skip_occurrence(cx)))
            .on_action(cx.listener(|this, _: &SelectAllOccurrences, _, cx| {
                this.select_all_occurrences(cx)
            }))
            .on_action(cx.listener(|this, _: &AddCursorsToLineEnds, _, cx| {
                this.add_cursors_to_line_ends(cx)
            }))
            .on_action(cx.listener(|this, _: &ColumnSelectUp, _, cx| this.column_select(-1, 0, cx)))
            .on_action(cx.listener(|this, _: &ColumnSelectDown, _, cx| this.column_select(1, 0, cx)))
            .on_action(cx.listener(|this, _: &ColumnSelectLeft, _, cx| this.column_select(0, -1, cx)))
            .on_action(cx.listener(|this, _: &ColumnSelectRight, _, cx| this.column_select(0, 1, cx)))
            .on_action(cx.listener(|this, _: &ColumnSelectPageUp, _, cx| {
                this.column_select(-PAGE_LINES, 0, cx)
            }))
            .on_action(cx.listener(|this, _: &ColumnSelectPageDown, _, cx| {
                this.column_select(PAGE_LINES, 0, cx)
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                for p in paths.paths() {
                    this.open_path(p.clone(), window, cx);
                }
            }))
            .child(div().w_full().child(self.menu_bar.clone()))
            .child(h_flex().w_full().border_b_1().border_color(border).child(self.tab_bar(cx)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    
                    .child(Editor::new(&editor).h_full().bordered(false)),
            )
            .child(self.status_bar(cx))
    }
}
