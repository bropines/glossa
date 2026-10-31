//! Shows each window backdrop on a real window, to check them by eye.
//!
//! ```text
//! cargo run --example backdrop -p glossa-platform -- [options]
//!   --kind K          hold one backdrop (none, mica, micaalt, acrylic, blur)
//!                     instead of cycling through them
//!   --popup K         the popup's backdrop (default acrylic)
//!   --secs S          seconds per backdrop while cycling (default 2.5)
//!   --exit-after S    close after S seconds
//!   --light           the light theme (default dark)
//!   --undecorated     no system title bar, as Glossa's custom frame
//!   --wgpu            draw with wgpu instead of OpenGL
//!   --no-canvas       no striped window behind (Acrylic then blurs whatever
//!                     is under the window)
//! ```
//!
//! Three windows open: a striped "canvas" so Acrylic and blur have
//! something visible to blur, the test window above it (eframe's root
//! viewport, given its backdrop through `eframe::Frame`), and a small
//! undecorated "popup" (an immediate viewport, found by title with
//! `find_own_window` and given its backdrop by HWND, as an unfocused
//! window). Each line printed says what was asked for and what the system
//! did. Mica and the system backdrops turn solid while their window is
//! inactive: click the test window to see them.

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32};
use glossa_core::theme::{builtin, Surface, Theme};
use glossa_platform::backdrop::{self, Backdrop};

const TITLE: &str = "Glossa backdrop demo";
const POPUP_TITLE: &str = "Glossa backdrop demo popup";
const CANVAS_TITLE: &str = "Glossa backdrop demo canvas";

struct Args {
    hold: Option<Backdrop>,
    popup: Backdrop,
    secs: f32,
    exit_after: Option<f32>,
    dark: bool,
    undecorated: bool,
    wgpu: bool,
    canvas: bool,
}

fn parse_args() -> Args {
    let mut a = Args {
        hold: None,
        popup: Backdrop::Acrylic,
        secs: 2.5,
        exit_after: None,
        dark: true,
        undecorated: false,
        wgpu: false,
        canvas: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().unwrap_or_default();
        match arg.as_str() {
            "--kind" => a.hold = value().parse().ok(),
            "--popup" => a.popup = value().parse().unwrap_or(Backdrop::Acrylic),
            "--secs" => a.secs = value().parse().unwrap_or(2.5),
            "--exit-after" => a.exit_after = value().parse().ok(),
            "--light" => a.dark = false,
            "--undecorated" => a.undecorated = true,
            "--wgpu" => a.wgpu = true,
            "--no-canvas" => a.canvas = false,
            other => eprintln!("unknown argument {other:?}"),
        }
    }
    a
}

struct Demo {
    args: Args,
    theme: Theme,
    started: Instant,
    index: usize,
    switched: Instant,
    applied_root: Option<Backdrop>,
    root_result: String,
    popup_hwnd: Option<isize>,
    applied_popup: bool,
    popup_result: String,
}

impl Demo {
    fn new(args: Args) -> Self {
        let mut theme = if args.dark { builtin::fluent_dark() } else { builtin::fluent_light() };
        let accent = glossa_platform::appearance::system_accent();
        println!(
            "windows build {:?}, system accent {:?}, system prefers dark {:?}",
            backdrop::windows_build(),
            accent.map(|a| a.base),
            glossa_platform::appearance::system_prefers_dark()
        );
        theme.resolve_system_accent(accent.as_ref());
        Demo {
            args,
            theme,
            started: Instant::now(),
            index: 0,
            switched: Instant::now(),
            applied_root: None,
            root_result: String::new(),
            popup_hwnd: None,
            applied_popup: false,
            popup_result: String::new(),
        }
    }

    fn current(&self) -> Backdrop {
        self.args.hold.unwrap_or(Backdrop::ALL[self.index % Backdrop::ALL.len()])
    }
}

fn c(color: glossa_core::Color) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

impl eframe::App for Demo {
    /// Fully transparent: whatever the panels leave unpainted shows the
    /// backdrop. eframe's default is a translucent dark grey, which would
    /// veil it.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let elapsed = self.started.elapsed().as_secs_f32();
        if self.args.exit_after.is_some_and(|s| elapsed >= s) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.args.hold.is_none() && self.switched.elapsed() >= Duration::from_secs_f32(self.args.secs) {
            self.index += 1;
            self.switched = Instant::now();
        }
        let kind = self.current();

        // The root window: through eframe's own handle, whenever the kind
        // changes (the first frame included).
        if self.applied_root != Some(kind) {
            self.applied_root = Some(kind);
            // Tell winit the theme too, or it re-applies the system theme
            // and the material's tint flips to match the system.
            let theme = if self.theme.dark { egui::SystemTheme::Dark } else { egui::SystemTheme::Light };
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(theme));
            let r = backdrop::apply(frame, kind, self.theme.dark);
            self.root_result = format!("{r:?}");
            println!("{:6.2}s root  {kind:8} -> {r:?}", elapsed);
        }

        let t = &self.theme;
        let window_fill = c(t.backdrop_fill(Surface::Window, kind));
        let client_fill = c(t.backdrop_fill(Surface::Client, kind));
        let text = c(t.window.text);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(window_fill).inner_margin(16)).show(ui, |ui| {
            ui.visuals_mut().override_text_color = Some(text);
            ui.heading(format!("Backdrop: {kind}"));
            ui.label(format!("root: {}", self.root_result));
            ui.label(format!("popup ({}): {}", self.args.popup, self.popup_result));
            ui.add_space(12.0);
            egui::Frame::new()
                .fill(client_fill)
                .stroke(egui::Stroke::new(1.0, c(t.window.border)))
                .corner_radius(t.metrics.window_radius as u8)
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.label("A pane drawn with the theme's backdrop fill for panes.");
                    ui.label("The window body around it is the backdrop itself.");
                });
            ui.add_space(8.0);
            let accent = c(t.accent);
            egui::Frame::new().fill(accent).corner_radius(t.metrics.button_radius as u8).inner_margin(8).show(
                ui,
                |ui| {
                    ui.colored_label(c(t.accent_text), "Accent (system accent if the system has one)");
                },
            );
        });

        if self.args.canvas {
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("canvas"),
                egui::ViewportBuilder::default()
                    .with_title(CANVAS_TITLE)
                    .with_position([60.0, 60.0])
                    .with_inner_size([900.0, 560.0])
                    .with_active(true)
                    .with_transparent(false),
                |ui, _| {
                    let rect = ui.max_rect();
                    let painter = ui.painter();
                    let colours = [
                        Color32::from_rgb(0xe8, 0x3a, 0x3a),
                        Color32::from_rgb(0xf5, 0xc2, 0x1b),
                        Color32::from_rgb(0x2e, 0xb8, 0x5c),
                        Color32::from_rgb(0x1f, 0x7a, 0xe0),
                        Color32::from_rgb(0x9b, 0x3a, 0xe8),
                        Color32::WHITE,
                        Color32::BLACK,
                    ];
                    let w = 40.0;
                    let mut x = rect.left();
                    let mut i = 0;
                    while x < rect.right() {
                        let stripe =
                            egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x + w, rect.bottom()));
                        painter.rect_filled(stripe, 0.0, colours[i % colours.len()]);
                        x += w;
                        i += 1;
                    }
                },
            );
        }

        let popup_kind = self.args.popup;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("popup"),
            egui::ViewportBuilder::default()
                .with_title(POPUP_TITLE)
                .with_decorations(false)
                .with_resizable(false)
                .with_taskbar(false)
                .with_window_level(egui::WindowLevel::AlwaysOnTop)
                .with_transparent(true)
                .with_active(false)
                .with_position([75.0, 540.0])
                .with_inner_size([320.0, 110.0]),
            |ui, _| {
                let fill = c(t.backdrop_fill(Surface::Popup, popup_kind));
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new().fill(fill).stroke(egui::Stroke::new(1.0, c(t.popup.frame))).inner_margin(12),
                    )
                    .show(ui, |ui| {
                        ui.visuals_mut().override_text_color = Some(c(t.popup.text));
                        ui.heading(format!("Popup: {popup_kind}"));
                        ui.label("An immediate viewport, given its backdrop by HWND.");
                    });
            },
        );
        // The popup's window exists from the frame after it was first shown;
        // its raw handle is not exposed, so find it by title.
        if !self.applied_popup {
            if self.popup_hwnd.is_none() {
                self.popup_hwnd = backdrop::find_own_window(POPUP_TITLE);
            }
            if let Some(hwnd) = self.popup_hwnd {
                self.applied_popup = true;
                // It is shown without taking focus, so ask for a material
                // that does not turn solid while the window is inactive.
                let r = backdrop::apply_hwnd_with(hwnd, popup_kind, backdrop::Options::unfocused(self.theme.dark));
                self.popup_result = format!("{r:?}");
                println!("{:6.2}s popup {popup_kind:8} -> {r:?} (hwnd {hwnd:#x})", elapsed);
            } else if !cfg!(windows) && elapsed > 1.0 {
                self.applied_popup = true;
                self.popup_result = "no HWND off Windows".into();
            }
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

fn main() -> eframe::Result {
    let args = parse_args();
    let viewport = egui::ViewportBuilder::default()
        .with_title(TITLE)
        .with_position([160.0, 140.0])
        .with_inner_size([560.0, 360.0])
        .with_decorations(!args.undecorated)
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        // Required for any backdrop: without it the GL (or wgpu) surface
        // has no alpha channel and DWM never sees through it.
        .with_transparent(true);
    let options = eframe::NativeOptions {
        viewport,
        renderer: if args.wgpu { eframe::Renderer::Wgpu } else { eframe::Renderer::Glow },
        ..Default::default()
    };
    eframe::run_native(TITLE, options, Box::new(|_cc| Ok(Box::new(Demo::new(args)))))
}
