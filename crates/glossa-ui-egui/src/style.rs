//! A [`Theme`] applied to egui.
//!
//! egui draws every widget from one `Visuals` struct with a style per
//! interaction state; a Glossa theme has the same idea with different names.
//! This module maps one onto the other and keeps the theme around for the
//! things egui has no slot for (popup frame, service tabs, custom title bar).

use egui::{Color32, CornerRadius, Margin, Shadow, Stroke, Visuals};
use glossa_core::config::Backdrop;
use glossa_core::theme::Surface;
use glossa_core::theme::{Theme, WidgetState};
use glossa_core::Color;

pub fn c(color: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

pub fn from32(c: Color32) -> Color {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    Color::rgba(r, g, b, a)
}

/// The resolved look, cheap to clone into viewports.
#[derive(Clone)]
pub struct Look {
    pub theme: Theme,
    /// The popup's colours after applying the options' overrides.
    pub popup_back: Color32,
    pub popup_text: Color32,
    pub popup_frame: Color32,
    pub popup_frame_width: f32,
    pub popup_opacity: f32,
    pub text_size: f32,
    /// Alpha for window and pane backgrounds (1.0 = opaque).
    pub window_alpha: f32,
    /// Multiplier for icon buttons (Options → Appearance).
    pub icon_scale: f32,
    /// The backdrop in effect for windows and for the popup: what the
    /// settings ask for, where the platform and renderer can show it.
    pub window_backdrop: Backdrop,
    pub popup_backdrop: Backdrop,
}

/// Whether window backdrops can show at all: Windows or macOS, with the
/// OpenGL renderer (under wgpu the client area stays opaque).
pub fn backdrops_supported(cfg: &glossa_core::Config) -> bool {
    (cfg!(windows) || cfg!(target_os = "macos")) && cfg.appearance.renderer == glossa_core::config::Renderer::OpenGl
}

/// Whether the main window has to be created with an alpha channel: child
/// windows can only be translucent if it is.
pub fn needs_transparent_root(cfg: &glossa_core::Config) -> bool {
    let a = &cfg.appearance;
    a.window_transparency > 0
        || (backdrops_supported(cfg) && (a.backdrop != Backdrop::None || a.popup.backdrop != Backdrop::None))
}

impl Look {
    pub fn new(mut theme: Theme, cfg: &glossa_core::Config) -> Look {
        let p = &cfg.appearance.popup;
        // `accent = "system"` themes take the system's accent colour now.
        theme.resolve_system_accent(glossa_platform::appearance::system_accent().as_ref());
        let supported = backdrops_supported(cfg);
        let window_backdrop = if supported { cfg.appearance.backdrop } else { Backdrop::None };
        let popup_backdrop = if supported { p.backdrop } else { Backdrop::None };
        // With a backdrop the theme's fills turn translucent, so the material
        // shows through everywhere they are used.
        if window_backdrop != Backdrop::None {
            theme.window.back = theme.backdrop_fill(Surface::Window, window_backdrop);
            theme.window.client_back = theme.backdrop_fill(Surface::Client, window_backdrop);
            let edit = theme.backdrop_fill(Surface::Edit, window_backdrop);
            theme.edit.normal.back = edit;
        }
        if popup_backdrop != Backdrop::None {
            theme.popup.back = theme.backdrop_fill(Surface::Popup, popup_backdrop);
        }
        // With window style on (the default) the popup follows the theme; the
        // separate popup colours apply only when it is off — as the classic
        // options page greyed them out.
        let themed = cfg.appearance.custom_frame;
        let pick = |own: Option<Color>, theme_c: Color| if themed { theme_c } else { own.unwrap_or(theme_c) };
        Look {
            popup_back: c(pick(p.back, theme.popup.back)),
            popup_text: c(pick(p.text, theme.popup.text)),
            popup_frame: c(pick(p.frame, theme.popup.frame)),
            popup_frame_width: p.frame_thickness,
            popup_opacity: p.opacity().min(theme.popup.opacity.max(0.1)).max(0.1),
            text_size: cfg.appearance.text_size,
            icon_scale: cfg.appearance.icon_scale.clamp(0.75, 3.0),
            // A backdrop replaces the transparency slider for the body.
            window_alpha: if window_backdrop != Backdrop::None {
                1.0
            } else {
                1.0 - cfg.appearance.window_transparency.min(90) as f32 / 100.0
            },
            window_backdrop,
            popup_backdrop,
            theme,
        }
    }

    pub fn accent(&self) -> Color32 {
        c(self.theme.accent)
    }

    pub fn text(&self) -> Color32 {
        c(self.theme.window.text)
    }

    pub fn weak_text(&self) -> Color32 {
        c(self.theme.window.text_disabled)
    }

    pub fn icon(&self, state: WidgetState) -> Color32 {
        c(self.theme.icon.get(state).main)
    }

    pub fn error(&self) -> Color32 {
        c(self.theme.error)
    }

    pub fn edit_back(&self) -> Color32 {
        c(self.theme.edit.normal.back).gamma_multiply(self.window_alpha.max(0.3))
    }

    pub fn window_back(&self) -> Color32 {
        c(self.theme.window.back).gamma_multiply(self.window_alpha)
    }

    pub fn border(&self) -> Color32 {
        c(self.theme.window.border)
    }

    /// Apply to an egui context: visuals, spacing, rounding.
    pub fn apply(&self, ctx: &egui::Context, ui_scale: f32) {
        let t = &self.theme;
        let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
        let radius = CornerRadius::same(t.metrics.button_radius.round().clamp(0.0, 20.0) as u8);
        let win_radius = CornerRadius::same(t.metrics.window_radius.round().clamp(0.0, 20.0) as u8);

        v.override_text_color = Some(c(t.window.text));
        v.panel_fill = c(t.window.back);
        v.window_fill = c(t.window.back);
        v.window_stroke = Stroke::new(t.metrics.border_width, c(t.window.border));
        v.window_corner_radius = win_radius;
        v.menu_corner_radius = radius;
        v.extreme_bg_color = c(t.edit.normal.back);
        v.text_edit_bg_color = Some(c(t.edit.normal.back));
        v.faint_bg_color = c(t.window.client_back);
        v.code_bg_color = c(t.window.client_back);
        v.hyperlink_color = c(t.link);
        v.error_fg_color = c(t.error);
        v.warn_fg_color = c(t.warning);
        v.selection.bg_fill = c(t.selection);
        v.selection.stroke = Stroke::new(1.0, c(t.accent));
        v.window_shadow = Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(if t.dark { 110 } else { 50 }),
        };
        v.popup_shadow = Shadow {
            offset: [0, 3],
            blur: 8,
            spread: 0,
            color: Color32::from_black_alpha(if t.dark { 90 } else { 40 }),
        };

        let set = |w: &mut egui::style::WidgetVisuals, state: WidgetState| {
            let b = t.button.get(state);
            w.bg_fill = c(b.back());
            w.weak_bg_fill = c(b.back());
            w.bg_stroke = Stroke::new(t.metrics.border_width, c(b.border));
            w.fg_stroke = Stroke::new(1.0, c(b.text));
            w.corner_radius = radius;
        };
        set(&mut v.widgets.inactive, WidgetState::Normal);
        set(&mut v.widgets.hovered, WidgetState::Hover);
        set(&mut v.widgets.active, WidgetState::Pressed);
        set(&mut v.widgets.open, WidgetState::Pressed);
        // egui draws `strong` text (dictionary headwords) in the pressed
        // widget's text colour, on the window body; a classic theme's white
        // pressed text vanished there. The body text reads on both.
        let pressed = t.button.get(WidgetState::Pressed).text;
        if pressed.contrast(t.window.back) < glossa_core::theme::MIN_GLYPH_CONTRAST {
            v.widgets.active.fg_stroke.color = c(t.window.text);
        }
        // Labels, separators and frames.
        v.widgets.noninteractive.bg_fill = c(t.window.back);
        v.widgets.noninteractive.weak_bg_fill = c(t.window.client_back);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, c(t.window.group_box));
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, c(t.window.text));
        v.widgets.noninteractive.corner_radius = radius;

        if self.window_alpha < 1.0 {
            let a = self.window_alpha;
            v.panel_fill = v.panel_fill.gamma_multiply(a);
            v.window_fill = v.window_fill.gamma_multiply(a);
            v.extreme_bg_color = v.extreme_bg_color.gamma_multiply(a.max(0.3));
            v.text_edit_bg_color = v.text_edit_bg_color.map(|c| c.gamma_multiply(a.max(0.3)));
            v.faint_bg_color = v.faint_bg_color.gamma_multiply(a);
        }
        // egui keeps a dark and a light style and picks one per window by the
        // system theme that window reports; `set_visuals` fills only the one
        // in use at the moment. A window reporting the other theme (a light
        // system under a dark Glossa theme, or the system theme changing)
        // then drew with egui's defaults. Both get the theme, and the choice
        // is pinned to the theme's own darkness.
        ctx.set_theme(if t.dark { egui::Theme::Dark } else { egui::Theme::Light });
        ctx.all_styles_mut(|s| s.visuals = v.clone());
        ctx.all_styles_mut(|s| {
            let sp = t.metrics.spacing.clamp(0.0, 24.0);
            let pad = t.metrics.padding.clamp(0.0, 24.0);
            s.spacing.item_spacing = egui::vec2(sp, sp * 0.75);
            s.spacing.button_padding = egui::vec2(pad + 2.0, pad * 0.5 + 1.0);
            s.spacing.window_margin = Margin::same(8);
            s.spacing.menu_margin = Margin::same(4);
            s.spacing.interact_size.y = (t.fonts.size * 2.0).max(22.0);
            s.interaction.tooltip_delay = 0.35;
            s.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(t.fonts.size * 1.33));
            s.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(t.fonts.size * 1.33));
            s.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(t.fonts.size * 1.1));
            s.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(t.fonts.size * 1.8));
            s.text_styles.insert(egui::TextStyle::Monospace, egui::FontId::monospace(t.fonts.size * 1.25));
        });
        ctx.set_zoom_factor(ui_scale.clamp(0.5, 3.0));
    }

    /// The font for the source and translation panes.
    pub fn text_font(&self) -> egui::FontId {
        egui::FontId::proportional(self.text_size * 1.33)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_convert_both_ways() {
        // Opaque colours are exact; translucent ones go through egui's
        // premultiplied storage and may move by one step.
        let col = Color::rgb(10, 20, 30);
        assert_eq!(from32(c(col)), col);
        let translucent = from32(c(Color::rgba(10, 20, 30, 200)));
        assert_eq!(translucent.a, 200);
        assert!((translucent.b as i32 - 30).abs() <= 1);
    }

    #[test]
    fn popup_colours_follow_the_theme_while_window_style_is_on() {
        let mut cfg = glossa_core::Config::default();
        cfg.appearance.popup.back = Some(Color::WHITE);
        let theme = glossa_core::theme::builtin::glossa_dark();
        let themed = Look::new(theme.clone(), &cfg);
        assert_eq!(themed.popup_back, c(theme.popup.back));
        cfg.appearance.custom_frame = false;
        let own = Look::new(theme, &cfg);
        assert_eq!(own.popup_back, Color32::WHITE);
    }
}
