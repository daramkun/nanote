//! Automatic updates: periodic background checks, a manual check from the settings window, and
//! restarting into the new version. The file work lives in `crate::updater`.

use std::time::Duration;

use gpui_kit::*;

use super::*;
use crate::updater::Outcome;

/// Give start-up (session restore, window layout) a moment before touching the network.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(8);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

impl NanoteApp {
    /// What the settings window shows before any check has run.
    pub(super) fn initial_update_status() -> String {
        if updater::supported() {
            String::new()
        } else {
            tr("Updates are disabled in this build").into()
        }
    }

    pub fn set_update_status(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text = text.into();
        self.update_status.update(cx, |status, cx| {
            *status = text;
            cx.notify();
        });
    }

    /// Check shortly after start and then every few hours, while the setting is on. Only the
    /// primary instance does this, so that several windows do not update at the same time.
    pub(super) fn start_update_checks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !updater::supported() || self.incoming.is_none() {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(FIRST_CHECK_DELAY).await;
            loop {
                let alive = this
                    .update_in(cx, |this, window, cx| {
                        if this.settings.auto_update {
                            this.check_for_updates(false, window, cx);
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
                cx.background_executor().timer(CHECK_INTERVAL).await;
            }
        })
        .detach();
    }

    /// Look for a newer release and install it in the background. `manual` is a click on the
    /// button: the outcome is then always reported, not only in the settings status line.
    pub fn check_for_updates(&mut self, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = updater::update_repo().filter(|_| updater::supported()) else {
            self.set_update_status(tr("Updates are disabled in this build"), cx);
            return;
        };
        if self.update_running {
            return;
        }
        self.update_running = true;
        self.set_update_status(tr("Checking for updates..."), cx);
        let current = Version::current();
        let newer_than = self.update_installed.map_or(current, |v| v.max(current));

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { updater::check_and_install(repo, newer_than) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.on_update_result(result, manual, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn on_update_result(
        &mut self,
        result: Result<Outcome, String>,
        manual: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_running = false;
        match result {
            Ok(Outcome::UpToDate) => {
                let version = self.update_installed.unwrap_or_else(Version::current);
                self.set_update_status(format!("{} ({version})", tr("You are up to date")), cx);
            }
            Ok(Outcome::Installed(version)) => {
                self.update_installed = Some(version);
                let text = format!("{} {version}", tr("Update installed, restart to apply:"));
                self.set_update_status(text.clone(), cx);
                self.status = text.into();

                let rx = window.prompt(
                    PromptLevel::Info,
                    &format!("nanote {version}"),
                    Some(tr("The update was installed. Restart now to use it?")),
                    &[tr("Restart now"), tr("Later")],
                    cx,
                );
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(0) = rx.await {
                        this.update_in(cx, |this, window, cx| this.restart_to_apply(window, cx))
                            .ok();
                    }
                })
                .detach();
            }
            Err(error) => {
                let text = format!("{}: {error}", tr("Update check failed"));
                self.set_update_status(text.clone(), cx);
                if manual {
                    self.status = text.into();
                }
            }
        }
        cx.notify();
    }

    /// Quit and start the installed version. Unsaved changes are asked about as on any close;
    /// cancelling that prompt cancels the restart too.
    pub fn restart_to_apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.relaunch_on_quit = true;
        if self.should_close_window(window, cx) {
            cx.quit();
        }
    }
}
