#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod encoding;
mod git;
mod i18n;
mod ipc;
mod lang;
mod settings;
mod theme;
mod treesitter;

use std::net::TcpListener;

use gpui_kit::*;

use app::{NanoteApp, parse_cli};
use settings::Settings;

fn main() {
    let settings = Settings::load();
    let cli = parse_cli();
    let new_window = cli.new_window.unwrap_or(settings.new_window);
    if !cli.paths.is_empty() && !new_window && ipc::forward(&cli.paths, cli.wait) {
        return;
    }
    // Only a regular instance accepts files from later invocations; a `--wait` one is
    // short-lived and must exit when its file is closed.
    let listener = if cli.wait {
        None
    } else {
        TcpListener::bind(ipc::ADDR).ok()
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            app::init(cx);
            let options = WindowOptions {
                // Where the window was last time; on a first start, a normal-sized window in the
                // middle of the screen (the platform default is nearly the whole screen).
                window_bounds: Some(app::saved_window_bounds(cx).unwrap_or_else(|| {
                    WindowBounds::Windowed(Bounds::centered(None, size(px(1100.), px(760.)), cx))
                })),
                titlebar: Some(TitlebarOptions {
                    title: Some("nanote".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| NanoteApp::new(window, cx, settings, cli, listener))
            })
            .expect("failed to open window");
            cx.activate(true);
        });
}
