#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod encoding;
mod git;
mod i18n;
mod ipc;
mod lang;
mod quick_open;
mod settings;
mod shell_menu;
mod theme;
mod treesitter;
mod updater;

use std::net::TcpListener;

use gpui_kit::*;

use app::{NanoteApp, parse_cli};
use settings::Settings;

fn bind_instance_port(relaunched: bool) -> Option<TcpListener> {
    // After an update the previous instance is still shutting down: wait for it to let go of the
    // port, otherwise this one would start without owning the session and window state.
    let attempts = if relaunched { 100 } else { 1 };
    for attempt in 0..attempts {
        if let Ok(listener) = TcpListener::bind(ipc::ADDR) {
            return Some(listener);
        }
        if attempt + 1 < attempts {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    None
}

/// `--install-context-menu` / `--uninstall-context-menu`: change the Explorer entry and exit.
fn run_shell_menu_command(enable: bool, mut settings: Settings) -> ! {
    i18n::set_korean(settings.korean());
    let result = match std::env::current_exe() {
        Ok(exe) if enable => shell_menu::register(i18n::tr("Open with nanote"), &exe),
        Ok(_) => shell_menu::unregister(),
        Err(e) => Err(e),
    };
    if result.is_ok() {
        // Keep the switch in the settings window truthful.
        settings.explorer_menu = enable;
        settings.save();
    }
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}

fn main() {
    // Leftovers of an earlier update are removed on every start.
    updater::cleanup_leftovers();
    let settings = Settings::load();
    let cli = parse_cli();
    if let Some(enable) = cli.shell_menu {
        run_shell_menu_command(enable, settings);
    }
    let new_window = cli.new_window.unwrap_or(settings.new_window);
    if !cli.paths.is_empty() && !new_window && ipc::forward(&cli.paths, cli.wait) {
        return;
    }
    // Only a regular instance accepts files from later invocations; a `--wait` one is
    // short-lived and must exit when its file is closed.
    let listener = if cli.wait {
        None
    } else {
        bind_instance_port(cli.relaunch)
    };
    // The port is taken but nobody answered the files above: another instance is still starting.
    // Explorer starts one process per selected file, all at once, so this really happens. Hand the
    // files over as soon as that instance listens instead of opening a second window.
    if listener.is_none() && !cli.wait && !cli.relaunch && !cli.paths.is_empty() && !new_window {
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if ipc::forward(&cli.paths, false) {
                return;
            }
        }
    }

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
