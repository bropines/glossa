//! Themes.
//!
//! A theme is data, not code: every colour and metric the UI draws with comes
//! from a [`Theme`], and a theme is a file a user can edit while the program
//! runs. Two formats are read:
//!
//! * **Native** — TOML (or JSON), the shape of [`Theme`] itself. It may say
//!   `extends = "Glossa Dark"` and override only what it wants to change.
//! * **Classic** — the JSON themes the original program shipped (`Common`,
//!   `Window`, `Button`, `Edit`, `WindowButton`, `Icon`, each with `Normal` /
//!   `Hover` / `Pressed` / `Disabled` / `Focused` states, colours in the
//!   shorthand described in [`crate::color`], `//` comments allowed). These
//!   load unchanged, so an existing theme collection carries over.
//!
//! The toolkit layer maps a [`Theme`] onto its own style system; nothing here
//! knows which toolkit that is.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::color::Color;
use crate::jsonc;

/// A fully resolved theme: every field has a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub author: String,
    /// Which built-in theme this one starts from. Resolved at load time and
    /// kept only for display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    /// Whether the toolkit should start from its dark or light defaults for
    /// anything the theme does not cover.
    pub dark: bool,

    pub window: WindowStyle,
    pub button: States<ButtonStyle>,
    pub edit: States<EditStyle>,
    /// The caption buttons: minimise, maximise, close.
    pub window_button: States<GlyphStyle>,
    /// Toolbar and in-pane icons: listen, copy, microphone.
    pub icon: States<GlyphStyle>,

    /// The accent: selection, focus ring, the active service tab, links.
    pub accent: Color,
    /// Text drawn on top of the accent.
    pub accent_text: Color,
    pub link: Color,
    pub selection: Color,
    /// Error and warning text.
    pub error: Color,
    pub warning: Color,

    pub popup: PopupStyle,
    pub metrics: Metrics,
    pub fonts: Fonts,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowStyle {
    /// The window body.
    pub back: Color,
    pub text: Color,
    pub text_disabled: Color,
    pub border: Color,
    /// An inner highlight line just inside the border, if any.
    pub border_inner: Option<Color>,
    /// The title bar, when it differs from the body.
    pub caption: Option<Color>,
    /// The area inside the frame — panes, lists.
    pub client_back: Color,
    /// The rule and caption of a group box.
    pub group_box: Color,
    /// Draw the classic dotted focus rectangle, instead of an accent ring.
    pub classic_focus: bool,
}

impl Default for WindowStyle {
    fn default() -> Self {
        builtin::glossa_dark().window
    }
}

/// One style per interaction state. A state the theme leaves out falls back
/// to `normal` (and `hover` additionally to a slightly shifted `normal`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct States<T> {
    pub normal: T,
    pub hover: Option<T>,
    pub pressed: Option<T>,
    pub disabled: Option<T>,
    pub focused: Option<T>,
}

impl<T: Clone> States<T> {
    pub fn same(style: T) -> Self {
        Self { normal: style, hover: None, pressed: None, disabled: None, focused: None }
    }

    pub fn get(&self, state: WidgetState) -> &T {
        let pick = match state {
            WidgetState::Normal => None,
            WidgetState::Hover => self.hover.as_ref(),
            WidgetState::Pressed => self.pressed.as_ref().or(self.hover.as_ref()),
            WidgetState::Disabled => self.disabled.as_ref(),
            WidgetState::Focused => self.focused.as_ref().or(self.hover.as_ref()),
        };
        pick.unwrap_or(&self.normal)
    }
}

impl<T: Default + Clone> Default for States<T> {
    fn default() -> Self {
        Self::same(T::default())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetState {
    Normal,
    Hover,
    Pressed,
    Disabled,
    Focused,
}

/// A push button. The classic themes draw a vertical gradient with a one-pixel
/// "gleam" highlight and "shadow" edge; a flat theme sets top and bottom the
/// same and leaves the rest unset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ButtonStyle {
    pub back_top: Color,
    pub back_bottom: Color,
    pub border: Color,
    pub text: Color,
    pub gleam_top: Option<Color>,
    pub gleam_bottom: Option<Color>,
    pub shadow_top: Option<Color>,
    pub shadow_bottom: Option<Color>,
}

impl Default for ButtonStyle {
    fn default() -> Self {
        Self {
            back_top: Color::grey(0x57),
            back_bottom: Color::grey(0x57),
            border: Color::grey(0x76),
            text: Color::grey(0xbb),
            gleam_top: None,
            gleam_bottom: None,
            shadow_top: None,
            shadow_bottom: None,
        }
    }
}

impl ButtonStyle {
    /// The single colour to use when the toolkit cannot draw a gradient.
    pub fn back(&self) -> Color {
        self.back_top.lerp(self.back_bottom, 0.5)
    }
}

/// A text field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditStyle {
    pub back: Color,
    pub border: Color,
    pub text: Color,
    pub shadow_top: Option<Color>,
    pub shadow_bottom: Option<Color>,
}

impl Default for EditStyle {
    fn default() -> Self {
        Self {
            back: Color::grey(0x16),
            border: Color::grey(0x33),
            text: Color::grey(0xbb),
            shadow_top: None,
            shadow_bottom: None,
        }
    }
}

/// A monochrome glyph: an icon or a caption button.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlyphStyle {
    pub main: Color,
    pub shadow: Option<Color>,
    pub gleam: Option<Color>,
    /// A plate behind the glyph, for hover and pressed states.
    pub back: Option<Color>,
}

impl Default for GlyphStyle {
    fn default() -> Self {
        Self { main: Color::grey(0x96), shadow: None, gleam: None, back: None }
    }
}

/// The selection popup. In the classic program these lived in the options
/// rather than the theme; they are here so a theme can ship a matching popup,
/// and the options can still override them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PopupStyle {
    pub back: Color,
    pub text: Color,
    pub frame: Color,
    /// Frame thickness in logical pixels.
    pub frame_thickness: f32,
    /// 0.0 (invisible) to 1.0 (opaque).
    pub opacity: f32,
}

impl Default for PopupStyle {
    fn default() -> Self {
        Self {
            back: Color::grey(0x20),
            text: Color::grey(0xdd),
            frame: Color::rgb(0x5d, 0x6c, 0x7a),
            frame_thickness: 2.0,
            opacity: 0.94,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Metrics {
    pub button_radius: f32,
    pub window_radius: f32,
    /// Which window corners get rounded.
    pub window_radius_corners: RadiusCorners,
    /// Space between widgets.
    pub spacing: f32,
    /// Padding inside buttons and fields.
    pub padding: f32,
    pub border_width: f32,
    /// Height of the service tab strip under the output pane.
    pub service_tab_height: f32,
    pub icon_size: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            button_radius: 2.0,
            window_radius: 0.0,
            window_radius_corners: RadiusCorners::None,
            spacing: 6.0,
            padding: 6.0,
            border_width: 1.0,
            service_tab_height: 28.0,
            icon_size: 16.0,
        }
    }
}

/// The classic `WindowBorderRadius` enum: `0` none, `1` top, `2` both, `3`
/// small.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RadiusCorners {
    #[default]
    None,
    Top,
    All,
    Small,
}

impl RadiusCorners {
    fn from_legacy(v: i64) -> Self {
        match v {
            1 => RadiusCorners::Top,
            2 => RadiusCorners::All,
            3 => RadiusCorners::Small,
            _ => RadiusCorners::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fonts {
    /// The UI face. Empty for the toolkit default.
    pub family: String,
    /// Body text size, in points.
    pub size: f32,
    /// A separate face for the source and translation panes, where reading
    /// comfort matters more than matching the chrome.
    pub text_family: String,
    pub text_size: f32,
    /// Fallback faces, tried in order for characters the main face lacks — the
    /// fix for tofu boxes in CJK, Arabic or Devanagari output.
    pub fallbacks: Vec<String>,
}

impl Default for Fonts {
    fn default() -> Self {
        Self {
            family: String::new(),
            size: 10.0,
            text_family: String::new(),
            text_size: 11.0,
            fallbacks: Vec::new(),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        builtin::glossa_dark()
    }
}

impl Theme {
    /// Read a theme file of either format. `resolve` is asked for a theme by
    /// name when the file says `extends`; return `None` if there is no such
    /// theme and the built-ins will be tried.
    pub fn load(
        text: &str,
        file_name_hint: &str,
        resolve: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Theme, ThemeError> {
        let lower = file_name_hint.to_ascii_lowercase();
        if lower.ends_with(".toml") {
            let value: toml::Value =
                toml::from_str(text).map_err(|e| ThemeError::Parse(e.to_string()))?;
            let json = serde_json::to_value(value).map_err(|e| ThemeError::Parse(e.to_string()))?;
            return Theme::from_native_value(json, file_name_hint, resolve, 0);
        }

        let value: Value = serde_json::from_str(&jsonc::strip(text))
            .map_err(|e| ThemeError::Parse(e.to_string()))?;
        if is_classic(&value) {
            let mut theme = Theme::from_classic_value(&value)?;
            if theme.name.is_empty() {
                theme.name = stem(file_name_hint);
            }
            Ok(theme)
        } else {
            Theme::from_native_value(value, file_name_hint, resolve, 0)
        }
    }

    fn from_native_value(
        mut value: Value,
        file_name_hint: &str,
        resolve: &dyn Fn(&str) -> Option<String>,
        depth: u8,
    ) -> Result<Theme, ThemeError> {
        if depth > 8 {
            return Err(ThemeError::Parse("`extends` chain is too deep (a cycle?)".into()));
        }
        let parent_name = value.get("extends").and_then(Value::as_str).map(str::to_owned);
        let base_value = match &parent_name {
            Some(parent) => {
                let parent_theme = match resolve(parent) {
                    Some(text) => {
                        // Recursion through `load` handles both formats.
                        let v: Value = if text.trim_start().starts_with('{') {
                            serde_json::from_str(&jsonc::strip(&text))
                                .map_err(|e| ThemeError::Parse(e.to_string()))?
                        } else {
                            let t: toml::Value = toml::from_str(&text)
                                .map_err(|e| ThemeError::Parse(e.to_string()))?;
                            serde_json::to_value(t).map_err(|e| ThemeError::Parse(e.to_string()))?
                        };
                        if is_classic(&v) {
                            Theme::from_classic_value(&v)?
                        } else {
                            Theme::from_native_value(v, parent, resolve, depth + 1)?
                        }
                    }
                    None => builtin::by_name(parent)
                        .ok_or_else(|| ThemeError::UnknownParent(parent.clone()))?,
                };
                serde_json::to_value(parent_theme).map_err(|e| ThemeError::Parse(e.to_string()))?
            }
            None => serde_json::to_value(Theme::default())
                .map_err(|e| ThemeError::Parse(e.to_string()))?,
        };

        let mut merged = base_value;
        if let Some(obj) = value.as_object_mut() {
            obj.remove("extends");
        }
        merge(&mut merged, value);
        let mut theme: Theme =
            serde_json::from_value(merged).map_err(|e| ThemeError::Parse(e.to_string()))?;
        theme.extends = parent_name;
        if theme.name.is_empty() || theme.name == Theme::default().name && depth == 0 {
            let s = stem(file_name_hint);
            if !s.is_empty() {
                theme.name = s;
            }
        }
        Ok(theme)
    }

    /// Convert a classic theme. Unset states stay unset so they fall back the
    /// way the original drew them; the popup and accent, which classic themes
    /// did not carry, are derived from the window colours.
    pub fn from_classic_value(v: &Value) -> Result<Theme, ThemeError> {
        let common = v.get("Common");
        let get = |path: &[&str]| -> Result<Option<Color>, ThemeError> {
            let mut cur = v;
            for key in path {
                match cur.get(*key) {
                    Some(next) => cur = next,
                    None => return Ok(None),
                }
            }
            match cur.as_str() {
                Some(s) => Color::parse_legacy(s).map_err(|e| ThemeError::Parse(e.to_string())),
                None => Ok(None),
            }
        };
        let req = |path: &[&str], fallback: Color| -> Result<Color, ThemeError> {
            Ok(get(path)?.unwrap_or(fallback))
        };

        let window_back = req(&["Window", "Normal", "Back"], Color::grey(0x20))?;
        let window_text = req(&["Window", "Normal", "Text"], Color::grey(0xbb))?;
        let dark = window_back.is_dark();

        let button = |state: &str| -> Result<Option<ButtonStyle>, ThemeError> {
            if v.get("Button").and_then(|b| b.get(state)).is_none() {
                return Ok(None);
            }
            let p = |k: &str| get(&["Button", state, k]);
            let top = p("BackTop")?.unwrap_or(Color::grey(0x57));
            Ok(Some(ButtonStyle {
                back_top: top,
                back_bottom: p("BackBottom")?.unwrap_or(top),
                border: p("Border")?.unwrap_or(Color::grey(0x76)),
                text: p("Text")?.unwrap_or(window_text),
                gleam_top: p("GleamTop")?,
                gleam_bottom: p("GleamBottom")?,
                shadow_top: p("ShadowTop")?,
                shadow_bottom: p("ShadowBottom")?,
            }))
        };
        let edit = |state: &str| -> Result<Option<EditStyle>, ThemeError> {
            if v.get("Edit").and_then(|b| b.get(state)).is_none() {
                return Ok(None);
            }
            let p = |k: &str| get(&["Edit", state, k]);
            Ok(Some(EditStyle {
                back: p("Back")?.unwrap_or(Color::grey(0x16)),
                border: p("Border")?.unwrap_or(Color::grey(0x33)),
                text: p("Text")?.unwrap_or(window_text),
                shadow_top: p("ShadowTop")?,
                shadow_bottom: p("ShadowBottom")?,
            }))
        };
        let glyph = |section: &str, state: &str| -> Result<Option<GlyphStyle>, ThemeError> {
            if v.get(section).and_then(|b| b.get(state)).is_none() {
                return Ok(None);
            }
            let p = |k: &str| get(&[section, state, k]);
            Ok(Some(GlyphStyle {
                main: p("Main")?.unwrap_or(window_text),
                shadow: p("Shadow")?,
                gleam: p("Gleam")?,
                back: p("Back")?,
            }))
        };
        fn states_of<T>(
            f: &dyn Fn(&str) -> Result<Option<T>, ThemeError>,
            fallback: T,
        ) -> Result<States<T>, ThemeError> {
            Ok(States {
                normal: f("Normal")?.unwrap_or(fallback),
                hover: f("Hover")?,
                pressed: f("Pressed")?,
                disabled: f("Disabled")?,
                focused: f("Focused")?,
            })
        }

        let button_states: States<ButtonStyle> = states_of(&button, ButtonStyle::default())?;
        let edit_states: States<EditStyle> = states_of(&edit, EditStyle::default())?;
        let window_button: States<GlyphStyle> =
            states_of(&|s| glyph("WindowButton", s), GlyphStyle::default())?;
        let icon: States<GlyphStyle> = states_of(&|s| glyph("Icon", s), GlyphStyle::default())?;

        let client_back = get(&["Common", "ClientBackColor"])?.unwrap_or(window_back);
        let group_box = get(&["Common", "GroupBoxColor"])?.unwrap_or(window_text);
        let int = |k: &str| common.and_then(|c| c.get(k)).and_then(Value::as_i64).unwrap_or(0);

        // Classic themes have no accent; the focused/pressed button or the
        // group box colour is the closest thing they have to one.
        let accent = button_states
            .focused
            .as_ref()
            .map(|b| b.border)
            .filter(|c| c.contrast(client_back) > 1.5)
            .unwrap_or(if dark { Color::rgb(0x00, 0x78, 0xd7) } else { Color::rgb(0x00, 0x66, 0xcc) });

        let mut theme = Theme {
            name: String::new(),
            author: String::new(),
            extends: None,
            dark,
            window: WindowStyle {
                back: window_back,
                text: window_text,
                text_disabled: req(&["Window", "Disabled", "Text"], window_text.lerp(window_back, 0.5))?,
                border: req(&["Window", "Normal", "Border"], window_back.shifted(0.15))?,
                border_inner: get(&["Window", "Normal", "BorderInner"])?,
                caption: get(&["Window", "Normal", "Caption"])?,
                client_back,
                group_box,
                classic_focus: common
                    .and_then(|c| c.get("PaintClassicFocus"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
            button: button_states,
            edit: edit_states,
            window_button,
            icon,
            accent,
            accent_text: if accent.is_dark() { Color::WHITE } else { Color::BLACK },
            link: if dark { Color::rgb(0x6f, 0xb5, 0xf1) } else { Color::rgb(0x00, 0x55, 0xad) },
            selection: accent.with_alpha(0x80),
            error: Color::rgb(0xe0, 0x4f, 0x4f),
            warning: Color::rgb(0xe0, 0xa8, 0x3c),
            popup: PopupStyle {
                back: client_back,
                text: window_text,
                frame: req(&["Window", "Normal", "Border"], Color::rgb(0x5d, 0x6c, 0x7a))?,
                ..PopupStyle::default()
            },
            metrics: Metrics {
                button_radius: int("ButtonRadius") as f32 * 2.0,
                window_radius: match RadiusCorners::from_legacy(int("WindowBorderRadius")) {
                    RadiusCorners::None => 0.0,
                    RadiusCorners::Small => 3.0,
                    _ => 6.0,
                },
                window_radius_corners: RadiusCorners::from_legacy(int("WindowBorderRadius")),
                ..Metrics::default()
            },
            fonts: Fonts::default(),
        };
        if let Some(name) = v.get("Name").and_then(Value::as_str) {
            theme.name = name.to_owned();
        }
        Ok(theme)
    }

    /// Serialise in the native format.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("theme file could not be parsed: {0}")]
    Parse(String),
    #[error("theme extends {0:?}, which does not exist")]
    UnknownParent(String),
}

/// A classic theme file is recognisable by its top-level sections.
fn is_classic(v: &Value) -> bool {
    v.get("Common").is_some() || (v.get("Window").is_some() && v.get("Button").is_some())
}

/// Deep-merge `overlay` into `base`: objects merge key by key, anything else
/// replaces.
fn merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(existing) => merge(existing, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (slot, value) => *slot = value,
    }
}

fn stem(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The themes compiled into the binary, so a fresh install has something to
/// show before any file is read.
pub mod builtin {
    use super::*;

    pub const NAMES: &[&str] = &["Glossa Dark", "Glossa Light", "Glossa Midnight"];

    pub fn by_name(name: &str) -> Option<Theme> {
        match name.to_ascii_lowercase().replace(['-', '_'], " ").as_str() {
            "glossa dark" | "dark" => Some(glossa_dark()),
            "glossa light" | "light" => Some(glossa_light()),
            "glossa midnight" | "midnight" => Some(glossa_midnight()),
            _ => None,
        }
    }

    pub fn all() -> Vec<Theme> {
        vec![glossa_dark(), glossa_light(), glossa_midnight()]
    }

    /// Close to the classic "Flat Dark" — the look most people had.
    pub fn glossa_dark() -> Theme {
        let back = Color::grey(0x20);
        let text = Color::grey(0xc8);
        let accent = Color::rgb(0x2a, 0x8d, 0xe4);
        Theme {
            name: "Glossa Dark".into(),
            author: String::new(),
            extends: None,
            dark: true,
            window: WindowStyle {
                back,
                text,
                text_disabled: Color::grey(0x70),
                border: Color::grey(0x33),
                border_inner: None,
                caption: None,
                client_back: Color::grey(0x16),
                group_box: Color::grey(0x48),
                classic_focus: false,
            },
            button: States {
                normal: ButtonStyle {
                    back_top: Color::grey(0x3a),
                    back_bottom: Color::grey(0x3a),
                    border: Color::grey(0x4a),
                    text,
                    ..ButtonStyle::default()
                },
                hover: Some(ButtonStyle {
                    back_top: Color::grey(0x48),
                    back_bottom: Color::grey(0x48),
                    border: Color::grey(0x5a),
                    text: Color::WHITE,
                    ..ButtonStyle::default()
                }),
                pressed: Some(ButtonStyle {
                    back_top: Color::grey(0x1c),
                    back_bottom: Color::grey(0x1c),
                    border: Color::grey(0x4a),
                    text: Color::WHITE,
                    ..ButtonStyle::default()
                }),
                disabled: Some(ButtonStyle {
                    back_top: Color::grey(0x2a),
                    back_bottom: Color::grey(0x2a),
                    border: Color::grey(0x33),
                    text: Color::grey(0x66),
                    ..ButtonStyle::default()
                }),
                focused: None,
            },
            edit: States::same(EditStyle {
                back: Color::grey(0x16),
                border: Color::grey(0x33),
                text: Color::grey(0xdd),
                shadow_top: None,
                shadow_bottom: None,
            }),
            window_button: States {
                normal: GlyphStyle { main: Color::grey(0x96), ..GlyphStyle::default() },
                hover: Some(GlyphStyle {
                    main: Color::WHITE,
                    back: Some(Color::grey(0x48)),
                    ..GlyphStyle::default()
                }),
                pressed: Some(GlyphStyle {
                    main: Color::WHITE,
                    back: Some(Color::grey(0x1c)),
                    ..GlyphStyle::default()
                }),
                disabled: None,
                focused: None,
            },
            icon: States {
                normal: GlyphStyle { main: Color::grey(0x8a), ..GlyphStyle::default() },
                hover: Some(GlyphStyle { main: Color::grey(0xe1), ..GlyphStyle::default() }),
                pressed: None,
                disabled: Some(GlyphStyle { main: Color::grey(0x48), ..GlyphStyle::default() }),
                focused: None,
            },
            accent,
            accent_text: Color::WHITE,
            link: Color::rgb(0x6f, 0xb5, 0xf1),
            selection: accent.with_alpha(0x70),
            error: Color::rgb(0xe8, 0x5c, 0x5c),
            warning: Color::rgb(0xe0, 0xa8, 0x3c),
            popup: PopupStyle::default(),
            metrics: Metrics::default(),
            fonts: Fonts::default(),
        }
    }

    pub fn glossa_light() -> Theme {
        let back = Color::grey(0xf3);
        let text = Color::grey(0x1e);
        let accent = Color::rgb(0x00, 0x67, 0xc0);
        let mut t = glossa_dark();
        t.name = "Glossa Light".into();
        t.dark = false;
        t.window = WindowStyle {
            back,
            text,
            text_disabled: Color::grey(0x9a),
            border: Color::grey(0xc8),
            border_inner: None,
            caption: None,
            client_back: Color::WHITE,
            group_box: Color::grey(0xb4),
            classic_focus: false,
        };
        t.button = States {
            normal: ButtonStyle {
                back_top: Color::grey(0xfb),
                back_bottom: Color::grey(0xf0),
                border: Color::grey(0xc0),
                text,
                ..ButtonStyle::default()
            },
            hover: Some(ButtonStyle {
                back_top: Color::rgb(0xe8, 0xf2, 0xfc),
                back_bottom: Color::rgb(0xdc, 0xec, 0xfc),
                border: Color::rgb(0x7e, 0xb4, 0xea),
                text,
                ..ButtonStyle::default()
            }),
            pressed: Some(ButtonStyle {
                back_top: Color::rgb(0xc4, 0xe0, 0xfc),
                back_bottom: Color::rgb(0xc4, 0xe0, 0xfc),
                border: Color::rgb(0x56, 0x9d, 0xe5),
                text,
                ..ButtonStyle::default()
            }),
            disabled: Some(ButtonStyle {
                back_top: Color::grey(0xf4),
                back_bottom: Color::grey(0xf4),
                border: Color::grey(0xd8),
                text: Color::grey(0xa0),
                ..ButtonStyle::default()
            }),
            focused: None,
        };
        t.edit = States::same(EditStyle {
            back: Color::WHITE,
            border: Color::grey(0xc8),
            text: Color::grey(0x10),
            shadow_top: None,
            shadow_bottom: None,
        });
        t.window_button = States {
            normal: GlyphStyle { main: Color::grey(0x50), ..GlyphStyle::default() },
            hover: Some(GlyphStyle {
                main: Color::BLACK,
                back: Some(Color::grey(0xe0)),
                ..GlyphStyle::default()
            }),
            pressed: Some(GlyphStyle {
                main: Color::BLACK,
                back: Some(Color::grey(0xcc)),
                ..GlyphStyle::default()
            }),
            disabled: None,
            focused: None,
        };
        t.icon = States {
            normal: GlyphStyle { main: Color::grey(0x6a), ..GlyphStyle::default() },
            hover: Some(GlyphStyle { main: Color::grey(0x10), ..GlyphStyle::default() }),
            pressed: None,
            disabled: Some(GlyphStyle { main: Color::grey(0xc0), ..GlyphStyle::default() }),
            focused: None,
        };
        t.accent = accent;
        t.link = Color::rgb(0x00, 0x55, 0xad);
        t.selection = accent.with_alpha(0x50);
        t.error = Color::rgb(0xc4, 0x2b, 0x1c);
        t.warning = Color::rgb(0x9d, 0x5d, 0x00);
        t.popup = PopupStyle {
            back: Color::grey(0xf0),
            text: Color::BLACK,
            frame: Color::rgb(0x5d, 0x6c, 0x7a),
            frame_thickness: 2.0,
            opacity: 0.96,
        };
        t
    }

    /// A deep blue-black with a teal accent, for OLED screens and late nights.
    pub fn glossa_midnight() -> Theme {
        let mut t = glossa_dark();
        t.name = "Glossa Midnight".into();
        t.window.back = Color::rgb(0x0d, 0x11, 0x17);
        t.window.client_back = Color::rgb(0x08, 0x0b, 0x10);
        t.window.border = Color::rgb(0x1f, 0x29, 0x37);
        t.window.group_box = Color::rgb(0x2b, 0x37, 0x48);
        t.window.text = Color::rgb(0xc9, 0xd1, 0xd9);
        t.edit.normal.back = Color::rgb(0x08, 0x0b, 0x10);
        t.edit.normal.border = Color::rgb(0x1f, 0x29, 0x37);
        t.edit.normal.text = Color::rgb(0xe6, 0xed, 0xf3);
        t.button.normal.back_top = Color::rgb(0x16, 0x1d, 0x27);
        t.button.normal.back_bottom = Color::rgb(0x16, 0x1d, 0x27);
        t.button.normal.border = Color::rgb(0x2b, 0x37, 0x48);
        t.accent = Color::rgb(0x2d, 0xd4, 0xbf);
        t.accent_text = Color::BLACK;
        t.link = Color::rgb(0x58, 0xa6, 0xff);
        t.selection = Color::rgb(0x2d, 0xd4, 0xbf).with_alpha(0x50);
        t.popup.back = Color::rgb(0x0d, 0x11, 0x17);
        t.popup.text = Color::rgb(0xe6, 0xed, 0xf3);
        t.popup.frame = Color::rgb(0x2d, 0xd4, 0xbf);
        t.metrics.button_radius = 4.0;
        t.metrics.window_radius = 8.0;
        t.metrics.window_radius_corners = RadiusCorners::All;
        t
    }
}

/// The `Map` alias is re-exported for callers building theme values in code.
pub type ThemeValue = Map<String, Value>;

#[cfg(test)]
mod tests {
    use super::*;

    const FLAT_DARK: &str = r#"{
        "Common": { "ButtonRadius": 0, "WindowBorderRadius": 0, "ClientBackColor": "20", "GroupBoxColor": "57" },
        "Window": {
            "Normal":   { "Back": "20", "Text": "b",  "Border": "33", "BorderInner": "", "Caption": "" },
            "Disabled": { "Back": "20", "Text": "96", "Border": "33", "BorderInner": "", "Caption": "" }
        },
        "WindowButton": {
            "Normal":  { "Main": "96", "Shadow": "96" },
            "Hover":   { "Main": "b", "Shadow": "b", "Back": "57" }
        },
        "Button": {
            "Normal":  { "ShadowTop": "1c", "Border": "76", "GleamTop": "", "BackTop": "57", "BackBottom": "57", "GleamBottom": "", "ShadowBottom": "1c", "Text": "b" },
            "Pressed": { "ShadowTop": "1c", "Border": "76", "GleamTop": "", "BackTop": "1c", "BackBottom": "1c", "GleamBottom": "", "ShadowBottom": "1c", "Text": "b" }
        },
        "Edit":  { "Normal": { "ShadowTop": "0e", "Border": "33", "Back": "16", "ShadowBottom": "0e", "Text": "b" } },
        "Icon":  { "Normal": { "Gleam": "57", "Main": "e1", "Shadow": "e1" } }
    }"#;

    fn no_resolver(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn classic_theme_loads_with_its_colours() {
        let t = Theme::load(FLAT_DARK, "Flat Dark.json", &no_resolver).unwrap();
        assert_eq!(t.name, "Flat Dark");
        assert!(t.dark);
        assert_eq!(t.window.back, Color::grey(0x20));
        assert_eq!(t.window.text, Color::grey(0xbb));
        assert_eq!(t.window.text_disabled, Color::grey(0x96));
        assert_eq!(t.window.border_inner, None, "empty string means unset");
        assert_eq!(t.button.normal.back(), Color::grey(0x57));
        assert_eq!(t.button.normal.shadow_top, Some(Color::grey(0x1c)));
        assert_eq!(t.button.normal.gleam_top, None);
        assert_eq!(t.edit.normal.back, Color::grey(0x16));
        assert_eq!(t.icon.normal.main, Color::grey(0xe1));
        assert_eq!(t.icon.normal.gleam, Some(Color::grey(0x57)));
    }

    #[test]
    fn missing_states_fall_back_to_normal_and_pressed_to_hover() {
        let t = Theme::load(FLAT_DARK, "Flat Dark.json", &no_resolver).unwrap();
        // Button has Pressed but no Hover.
        assert_eq!(t.button.get(WidgetState::Hover), &t.button.normal);
        assert_eq!(t.button.get(WidgetState::Pressed).back(), Color::grey(0x1c));
        // WindowButton has Hover but no Pressed: Pressed borrows Hover.
        assert_eq!(t.window_button.get(WidgetState::Pressed).back, Some(Color::grey(0x57)));
    }

    #[test]
    fn classic_theme_comments_are_tolerated() {
        let with_comments = r#"{
            // a comment
            "Common": { "WindowBorderRadius": 3, // 0 - None, 1 - Top, 2 - Both, 3 - Small
                        "ClientBackColor": "1d8abd" },
            "Window": { "Normal": { "Back": "1586bb", "Text": "fe" } },
            "Button": { "Normal": { "BackTop": "1d8abd" } }
        }"#;
        let t = Theme::load(with_comments, "Blue.json", &no_resolver).unwrap();
        assert_eq!(t.metrics.window_radius_corners, RadiusCorners::Small);
        assert_eq!(t.window.client_back, Color::rgb(0x1d, 0x8a, 0xbd));
        assert!(!t.window.back.is_dark() || t.window.back == Color::rgb(0x15, 0x86, 0xbb));
    }

    #[test]
    fn native_theme_extends_a_builtin_and_overrides_a_little() {
        let native = r##"
            extends = "Glossa Dark"
            name = "Mine"
            accent = "#ff8800"
            [popup]
            opacity = 0.5
        "##;
        let t = Theme::load(native, "mine.toml", &no_resolver).unwrap();
        assert_eq!(t.name, "Mine");
        assert_eq!(t.extends.as_deref(), Some("Glossa Dark"));
        assert_eq!(t.accent, Color::rgb(0xff, 0x88, 0x00));
        assert_eq!(t.popup.opacity, 0.5);
        // Untouched fields come from the parent.
        let parent = builtin::glossa_dark();
        assert_eq!(t.window, parent.window);
        assert_eq!(t.popup.frame, parent.popup.frame);
    }

    #[test]
    fn native_theme_can_extend_a_classic_file() {
        let child = r##"{ "extends": "Flat Dark", "name": "Flat Dark Orange", "accent": "#ff8800" }"##;
        let resolver = |name: &str| (name == "Flat Dark").then(|| FLAT_DARK.to_owned());
        let t = Theme::load(child, "orange.json", &resolver).unwrap();
        assert_eq!(t.name, "Flat Dark Orange");
        assert_eq!(t.window.back, Color::grey(0x20));
        assert_eq!(t.accent, Color::rgb(0xff, 0x88, 0x00));
    }

    #[test]
    fn unknown_parent_is_an_error() {
        let child = r#"extends = "Nope""#;
        assert!(matches!(
            Theme::load(child, "x.toml", &no_resolver),
            Err(ThemeError::UnknownParent(_))
        ));
    }

    #[test]
    fn extends_cycles_are_caught() {
        let a = r#"{ "extends": "b" }"#;
        let b = r#"{ "extends": "a" }"#;
        let resolver = |name: &str| match name {
            "a" => Some(a.to_owned()),
            "b" => Some(b.to_owned()),
            _ => None,
        };
        assert!(Theme::load(a, "a.json", &resolver).is_err());
    }

    #[test]
    fn builtins_round_trip_through_toml() {
        for theme in builtin::all() {
            let text = theme.to_toml();
            let back = Theme::load(&text, "x.toml", &no_resolver).unwrap();
            assert_eq!(back.window, theme.window, "{} window", theme.name);
            assert_eq!(back.accent, theme.accent, "{} accent", theme.name);
            assert_eq!(back.button, theme.button, "{} button", theme.name);
        }
    }

    #[test]
    fn builtin_text_is_readable_on_its_background() {
        for theme in builtin::all() {
            let ratio = theme.window.text.contrast(theme.window.back);
            assert!(ratio >= 4.5, "{}: body text contrast {ratio:.1} is below WCAG AA", theme.name);
            let ratio = theme.edit.normal.text.contrast(theme.edit.normal.back);
            assert!(ratio >= 4.5, "{}: edit text contrast {ratio:.1} is below WCAG AA", theme.name);
        }
    }
}
