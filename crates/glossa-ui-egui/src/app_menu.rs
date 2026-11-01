//! macOS: what the menu bar's items do (the menu itself is
//! `glossa_platform::app_menu`). Elsewhere there is no menu and nothing
//! arrives here.

use std::sync::mpsc::{self, Receiver};

use glossa_core::Locale;
use glossa_platform::app_menu::{MenuCommand, MenuLabels};

use crate::app::GlossaApp;

/// The channel the menu's commands arrive on.
pub struct AppMenu {
    rx: Receiver<MenuCommand>,
}

impl AppMenu {
    /// Put the menu up (again, after the interface language changed). Main
    /// thread only, like everything in the GUI.
    pub fn install(ctx: &egui::Context, locale: &Locale) -> AppMenu {
        let (tx, rx) = mpsc::channel();
        if cfg!(target_os = "macos") {
            let ctx = ctx.clone();
            glossa_platform::app_menu::install(&labels(locale), move |c| {
                let _ = tx.send(c);
                ctx.request_repaint();
            });
        }
        AppMenu { rx }
    }
}

fn labels(l: &Locale) -> MenuLabels {
    let t = |k: &str| l.get(k).to_owned();
    MenuLabels {
        about: t("menu.about"),
        support: t("menu.support"),
        settings: t("macmenu.settings"),
        hide: t("macmenu.hide"),
        hide_others: t("macmenu.hide_others"),
        show_all: t("macmenu.show_all"),
        quit: t("macmenu.quit"),
        edit: t("macmenu.edit"),
        undo: t("edit.undo"),
        redo: t("macmenu.redo"),
        cut: t("edit.cut"),
        copy: t("edit.copy"),
        paste: t("edit.paste"),
        select_all: t("edit.select_all"),
        window: t("macmenu.window"),
        minimize: t("macmenu.minimize"),
        close: t("macmenu.close"),
    }
}

/// Act on the commands chosen since the last frame.
pub fn poll(app: &mut GlossaApp) {
    let commands: Vec<MenuCommand> = app.app_menu.rx.try_iter().collect();
    for c in commands {
        match c {
            MenuCommand::About => app.open_about(),
            MenuCommand::Support => app.open_url(glossa_core::SUPPORT_URL),
            MenuCommand::Settings => crate::options::open(app),
            MenuCommand::Quit => app.quit(),
            MenuCommand::CloseWindow => close_focused(&app.ctx),
        }
    }
}

/// Close the focused window the way its close button would, so each window's
/// own `close_requested` handling applies (the main window may go to the
/// menu bar instead, by the settings).
fn close_focused(ctx: &egui::Context) {
    let focused = ctx.input(|i| i.raw.viewports.iter().find(|(_, v)| v.focused == Some(true)).map(|(id, _)| *id));
    if let Some(id) = focused {
        ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Close);
    }
}
