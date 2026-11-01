//! The settings window (a GPUI Kit `Settings` view inside a dialog).

use gpui_kit::component::{
    Disableable as _, Sizable as _, WindowExt as _,
    button::Button,
    h_flex,
    setting::{
        NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage,
        Settings as SettingsView,
    },
    v_flex,
};
use gpui_kit::*;

use super::*;
use crate::{
    file_assoc::Group,
    git::{git_set_editor, git_show_editor, git_unset_editor},
    settings::DEFAULT_FONT,
};

fn update(cx: &mut App, f: impl FnOnce(&mut Settings)) {
    let mut next = cx.global::<SettingsGlobal>().0.clone();
    f(&mut next);
    cx.set_global(SettingsGlobal(next));
}

fn read<T>(cx: &App, f: impl FnOnce(&Settings) -> T) -> T {
    f(&cx.global::<SettingsGlobal>().0)
}

fn options(items: &[(&str, &str)]) -> Vec<(SharedString, SharedString)> {
    items
        .iter()
        .map(|(v, l)| {
            (
                SharedString::from(v.to_string()),
                SharedString::from(l.to_string()),
            )
        })
        .collect()
}

impl NanoteApp {
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dark: Vec<&'static str> = self.themes.dark.clone();
        let light: Vec<&'static str> = self.themes.light.clone();
        let mut fonts: Vec<String> = cx
            .text_system()
            .all_font_names()
            .into_iter()
            .filter(|n| !n.starts_with('.'))
            .collect();
        fonts.sort_by_key(|s| s.to_lowercase());
        fonts.dedup();
        let git_status: Entity<SharedString> = cx.new(|_| SharedString::default());
        let app = cx.entity();
        let update_status = self.update_status.clone();
        let plugin_loaded = matches!(self.plugin_status, PluginStatus::Loaded { .. });
        let plugin_text: SharedString = match &self.plugin_status {
            PluginStatus::Loaded { languages } => format!(
                "{} ({} {})",
                tr("tree-sitter plugin loaded"),
                languages,
                tr("languages")
            ),
            PluginStatus::NotFound => format!(
                "{} {}",
                tr("tree-sitter plugin not found"),
                tr("Place nanote_tree_sitter.dll next to nanote.exe.")
            ),
            PluginStatus::Failed(e) => {
                format!("{}: {e}", tr("tree-sitter plugin failed to load"))
            }
        }
        .into();

        window.open_dialog(cx, move |dialog, _window, _cx| {
            dialog
                .title(tr("Settings"))
                .w(px(860.))
                .child(div().h(px(560.)).w_full().child(
                    SettingsView::new("nanote-settings").pages(settings_pages(
                        dark.clone(),
                        light.clone(),
                        fonts.clone(),
                        git_status.clone(),
                        plugin_loaded,
                        plugin_text.clone(),
                        app.clone(),
                        update_status.clone(),
                    )),
                ))
        });
    }
}

fn settings_pages(
    dark: Vec<&'static str>,
    light: Vec<&'static str>,
    fonts: Vec<String>,
    git_status: Entity<SharedString>,
    plugin_loaded: bool,
    plugin_text: SharedString,
    app: Entity<NanoteApp>,
    update_status: Entity<SharedString>,
) -> Vec<SettingPage> {
    let theme_options = |names: &[&'static str]| -> Vec<(SharedString, SharedString)> {
        names
            .iter()
            .map(|n| {
                (
                    SharedString::from(n.to_string()),
                    SharedString::from(n.to_string()),
                )
            })
            .collect()
    };
    let mut font_options = vec![(
        SharedString::from(DEFAULT_FONT),
        SharedString::from(DEFAULT_FONT),
    )];
    font_options.extend(
        fonts
            .into_iter()
            .map(|n| (SharedString::from(n.clone()), SharedString::from(n))),
    );

    let windows_app = app.clone();
    let can_update = updater::supported();
    let updates = SettingGroup::new()
        .title(tr("Updates"))
        .item(
            SettingItem::new(
                tr("Check for updates automatically"),
                SettingField::switch(
                    |cx| read(cx, |s| s.auto_update),
                    |v, cx| update(cx, |s| s.auto_update = v),
                ),
            )
            .disabled(!can_update),
        )
        .item(SettingItem::render(move |_, _, cx| {
            let app = app.clone();
            v_flex()
                .gap_2()
                .child(div().child(format!("nanote {}", Version::current())))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("check-updates")
                                .label(tr("Check for updates"))
                                .small()
                                .disabled(!can_update)
                                .on_click(move |_, window, cx| {
                                    app.update(cx, |a, cx| a.check_for_updates(true, window, cx))
                                }),
                        )
                        .child(div().child(update_status.read(cx).clone())),
                )
        }));

    let windows = SettingGroup::new().title(tr("Windows")).item(
        SettingItem::new(
            tr("Add \"Open with nanote\" to the Explorer context menu"),
            SettingField::switch(
                |cx| read(cx, |s| s.explorer_menu),
                |v, cx| update(cx, |s| s.explorer_menu = v),
            ),
        )
        .description(tr("On Windows 11 it is under \"Show more options\"."))
        .disabled(!cfg!(windows)),
    )
    .item(SettingItem::render(move |_, _, _| {
        let button = |id: &'static str, label: &'static str, group: Option<Group>| {
            let app = windows_app.clone();
            Button::new(id)
                .label(tr(label))
                .small()
                .disabled(!cfg!(windows))
                .on_click(move |_, _, cx| {
                    app.update(cx, |a, cx| match group {
                        Some(group) => a.register_file_types(group, cx),
                        None => a.unregister_file_types(cx),
                    })
                })
        };
        v_flex()
            .w_full()
            .gap_2()
            .child(div().child(tr("File associations")))
            .child(
                div()
                    .text_xs()
                    .opacity(0.7)
                    .child(tr("Lists nanote under Open with and Default apps; Windows leaves the choice of default to you.")),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(button("assoc-text", "Text (.txt .md)", Some(Group::Text)))
                    .child(button("assoc-data", "+ .json .xml .yml", Some(Group::Data)))
                    .child(button("assoc-all", "All supported types", Some(Group::All)))
                    .child(button("assoc-remove", "Remove", None)),
            )
    }));

    let general = SettingPage::new(tr("General"))
        .group(
            SettingGroup::new()
                .item(SettingItem::new(
                    tr("Interface language"),
                    SettingField::dropdown(
                        options(&[
                            ("system", tr("System")),
                            ("en", "English"),
                            ("ko", "한국어"),
                        ]),
                        |cx| {
                            match read(cx, |s| s.ui_language) {
                                UiLang::System => "system",
                                UiLang::English => "en",
                                UiLang::Korean => "ko",
                            }
                            .into()
                        },
                        |v, cx| {
                            update(cx, |s| {
                                s.ui_language = match v.as_ref() {
                                    "en" => UiLang::English,
                                    "ko" => UiLang::Korean,
                                    _ => UiLang::System,
                                }
                            })
                        },
                    ),
                ))
                .item(SettingItem::new(
                    tr("Restore previously opened files on startup"),
                    SettingField::switch(
                        |cx| read(cx, |s| s.restore_session),
                        |v, cx| update(cx, |s| s.restore_session = v),
                    ),
                ))
                .item(SettingItem::new(
                    tr("Open command-line files in a new window"),
                    SettingField::switch(
                        |cx| read(cx, |s| s.new_window),
                        |v, cx| update(cx, |s| s.new_window = v),
                    ),
                )),
        )
        .group(windows)
        .group(updates);

    let appearance = SettingPage::new(tr("Appearance"))
        .group(
            SettingGroup::new()
                .title(tr("Theme"))
                .item(SettingItem::new(
                    tr("Theme"),
                    SettingField::dropdown(
                        options(&[
                            ("system", tr("System")),
                            ("light", tr("Light")),
                            ("dark", tr("Dark")),
                        ]),
                        |cx| {
                            match read(cx, |s| s.theme_mode) {
                                ThemeMode::System => "system",
                                ThemeMode::Light => "light",
                                ThemeMode::Dark => "dark",
                            }
                            .into()
                        },
                        |v, cx| {
                            update(cx, |s| {
                                s.theme_mode = match v.as_ref() {
                                    "light" => ThemeMode::Light,
                                    "dark" => ThemeMode::Dark,
                                    _ => ThemeMode::System,
                                }
                            })
                        },
                    ),
                ))
                .item(SettingItem::new(
                    tr("Syntax theme (dark mode)"),
                    SettingField::scrollable_dropdown(
                        theme_options(&dark),
                        |cx| read(cx, |s| s.syntax_dark.clone()).into(),
                        |v, cx| update(cx, |s| s.syntax_dark = v.to_string()),
                    ),
                ))
                .item(SettingItem::new(
                    tr("Syntax theme (light mode)"),
                    SettingField::scrollable_dropdown(
                        theme_options(&light),
                        |cx| read(cx, |s| s.syntax_light.clone()).into(),
                        |v, cx| update(cx, |s| s.syntax_light = v.to_string()),
                    ),
                )),
        )
        .group(
            SettingGroup::new()
                .title(tr("Highlighting engine"))
                .item(
                    SettingItem::new(
                        tr("Use tree-sitter highlighting when available"),
                        SettingField::switch(
                            |cx| read(cx, |s| s.use_tree_sitter),
                            |v, cx| update(cx, |s| s.use_tree_sitter = v),
                        ),
                    )
                    .disabled(!plugin_loaded),
                )
                .item(SettingItem::render(move |_, _, _| {
                    div().child(plugin_text.clone())
                })),
        )
        .group(
            SettingGroup::new()
                .title(tr("Font"))
                .item(SettingItem::new(
                    tr("Size"),
                    SettingField::number_input(
                        NumberFieldOptions {
                            min: 6.0,
                            max: 96.0,
                            step: 1.0,
                        },
                        |cx| read(cx, |s| s.font_size as f64),
                        |v, cx| update(cx, |s| s.font_size = v as f32),
                    ),
                ))
                .item(SettingItem::new(
                    tr("Font"),
                    SettingField::scrollable_dropdown(
                        font_options,
                        |cx| read(cx, |s| s.font_family.clone()).into(),
                        |v, cx| update(cx, |s| s.font_family = v.to_string()),
                    ),
                )),
        );

    let git = SettingPage::new("Git").group(
        SettingGroup::new()
            .item(SettingItem::render(move |_, _, _| {
                div().child(tr(
                    "Use nanote as the editor for git commit messages, rebase todo lists, etc.",
                ))
            }))
            .item(SettingItem::render({
                let status = git_status.clone();
                move |_, _, _| {
                    let (a, b, c) = (status.clone(), status.clone(), status.clone());
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("git-set")
                                .label(tr("Set as Git editor"))
                                .small()
                                .on_click(move |_, _, cx| {
                                    a.update(cx, |s, cx| {
                                        *s = git_set_editor().into();
                                        cx.notify();
                                    })
                                }),
                        )
                        .child(
                            Button::new("git-unset")
                                .label(tr("Unset Git editor"))
                                .small()
                                .on_click(move |_, _, cx| {
                                    b.update(cx, |s, cx| {
                                        *s = git_unset_editor().into();
                                        cx.notify();
                                    })
                                }),
                        )
                        .child(
                            Button::new("git-show")
                                .label(tr("Show current Git editor"))
                                .small()
                                .on_click(move |_, _, cx| {
                                    c.update(cx, |s, cx| {
                                        *s = git_show_editor().into();
                                        cx.notify();
                                    })
                                }),
                        )
                }
            }))
            .item(SettingItem::render({
                let status = git_status;
                move |_, _, cx| div().child(status.read(cx).clone())
            })),
    );

    vec![general, appearance, git]
}
