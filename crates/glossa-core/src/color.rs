//! Colours, and the shorthand the classic theme files wrote them in.
//!
//! The classic themes spell colours as bare hex of varying length, and the
//! length carries meaning:
//!
//! | written   | means            |
//! |-----------|------------------|
//! | `"b"`     | grey `#bbbbbb`   |
//! | `"57"`    | grey `#575757`   |
//! | `"39f"`   | `#3399ff` (CSS)  |
//! | `"1586bb"`| `#1586bb`        |
//! | `""`      | "not set"        |
//!
//! The one- and two-digit forms are grey levels, which is most of what a
//! dark theme needs. The options file, separately, stores colours as Win32
//! `COLORREF` integers (`0x00BBGGRR`); [`Color::from_colorref`] reads those.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn grey(level: u8) -> Self {
        Self::rgb(level, level, level)
    }

    /// Read a Win32 `COLORREF` (`0x00BBGGRR`), as the classic options file
    /// stores the popup colours.
    pub const fn from_colorref(value: u32) -> Self {
        Self::rgb((value & 0xff) as u8, ((value >> 8) & 0xff) as u8, ((value >> 16) & 0xff) as u8)
    }

    pub const fn to_colorref(self) -> u32 {
        (self.r as u32) | ((self.g as u32) << 8) | ((self.b as u32) << 16)
    }

    /// Parse the classic theme shorthand. `Ok(None)` for the empty string,
    /// which the theme files use for "not set".
    pub fn parse_legacy(s: &str) -> Result<Option<Color>, ColorParseError> {
        let s = s.trim().trim_start_matches('#');
        if s.is_empty() {
            return Ok(None);
        }
        if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ColorParseError(s.to_owned()));
        }
        let hex = |t: &str| u8::from_str_radix(t, 16).map_err(|_| ColorParseError(s.to_owned()));
        Ok(Some(match s.len() {
            1 => {
                let v = hex(s)?;
                Color::grey(v * 17) // `b` -> 0xbb
            }
            2 => Color::grey(hex(s)?),
            3 => {
                let d = |i: usize| hex(&s[i..=i]).map(|v| v * 17);
                Color::rgb(d(0)?, d(1)?, d(2)?)
            }
            6 => Color::rgb(hex(&s[0..2])?, hex(&s[2..4])?, hex(&s[4..6])?),
            8 => Color::rgba(hex(&s[0..2])?, hex(&s[2..4])?, hex(&s[4..6])?, hex(&s[6..8])?),
            _ => return Err(ColorParseError(s.to_owned())),
        }))
    }

    /// Mix toward `other` by `t` (0.0 keeps `self`, 1.0 gives `other`).
    pub fn lerp(self, other: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Color::rgba(mix(self.r, other.r), mix(self.g, other.g), mix(self.b, other.b), mix(self.a, other.a))
    }

    pub fn with_alpha(self, a: u8) -> Color {
        Color { a, ..self }
    }

    /// Relative luminance, 0.0 (black) to 1.0 (white), per WCAG.
    pub fn luminance(self) -> f32 {
        fn channel(c: u8) -> f32 {
            let c = c as f32 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    pub fn is_dark(self) -> bool {
        self.luminance() < 0.25
    }

    /// WCAG contrast ratio against another colour, 1.0 to 21.0.
    pub fn contrast(self, other: Color) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Lighter or darker by `amount`, whichever keeps it readable — useful
    /// for deriving a hover state the theme did not spell out.
    pub fn shifted(self, amount: f32) -> Color {
        if self.is_dark() {
            self.lerp(Color::WHITE, amount)
        } else {
            self.lerp(Color::BLACK, amount)
        }
    }
}

impl fmt::Debug for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.a == 255 {
            write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            write!(f, "#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a colour: {0:?}")]
pub struct ColorParseError(pub String);

impl FromStr for Color {
    type Err = ColorParseError;

    /// Our own files write `#rrggbb` or `#rrggbbaa`; the legacy shorthand is
    /// accepted too, so a theme can be half-converted.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Color::parse_legacy(s)?.ok_or_else(|| ColorParseError(s.to_owned()))
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_shorthand_lengths() {
        assert_eq!(Color::parse_legacy("b").unwrap(), Some(Color::grey(0xbb)));
        assert_eq!(Color::parse_legacy("0").unwrap(), Some(Color::BLACK));
        assert_eq!(Color::parse_legacy("f").unwrap(), Some(Color::WHITE));
        assert_eq!(Color::parse_legacy("57").unwrap(), Some(Color::grey(0x57)));
        assert_eq!(Color::parse_legacy("0e").unwrap(), Some(Color::grey(0x0e)));
        assert_eq!(Color::parse_legacy("39f").unwrap(), Some(Color::rgb(0x33, 0x99, 0xff)));
        assert_eq!(Color::parse_legacy("1586bb").unwrap(), Some(Color::rgb(0x15, 0x86, 0xbb)));
        assert_eq!(Color::parse_legacy("#1586bb").unwrap(), Some(Color::rgb(0x15, 0x86, 0xbb)));
        assert_eq!(Color::parse_legacy("").unwrap(), None);
        assert_eq!(Color::parse_legacy("  ").unwrap(), None);
    }

    #[test]
    fn bad_colours_are_rejected() {
        assert!(Color::parse_legacy("zz").is_err());
        assert!(Color::parse_legacy("12345").is_err());
        assert!("".parse::<Color>().is_err());
    }

    #[test]
    fn colorref_is_bgr() {
        // The frame colour from a real options file: a slate blue-grey.
        assert_eq!(Color::from_colorref(8023133), Color::rgb(0x5d, 0x6c, 0x7a));
        // And the popup background, near-white.
        assert_eq!(Color::from_colorref(15790320), Color::grey(0xf0));
        let c = Color::rgb(1, 2, 3);
        assert_eq!(Color::from_colorref(c.to_colorref()), c);
    }

    #[test]
    fn display_round_trips_through_from_str() {
        for c in [Color::rgb(0x12, 0x34, 0x56), Color::rgba(1, 2, 3, 4)] {
            assert_eq!(c.to_string().parse::<Color>().unwrap(), c);
        }
    }

    #[test]
    fn contrast_and_darkness() {
        assert!((Color::BLACK.contrast(Color::WHITE) - 21.0).abs() < 0.01);
        assert!((Color::WHITE.contrast(Color::WHITE) - 1.0).abs() < 0.01);
        assert!(Color::grey(0x20).is_dark());
        assert!(!Color::grey(0xf0).is_dark());
        // A shift moves away from the extreme the colour is near.
        assert!(Color::grey(0x20).shifted(0.2).r > 0x20);
        assert!(Color::grey(0xf0).shifted(0.2).r < 0xf0);
    }

    #[test]
    fn lerp_endpoints() {
        let a = Color::rgb(0, 0, 0);
        let b = Color::rgb(200, 100, 50);
        assert_eq!(a.lerp(b, 0.0), a);
        assert_eq!(a.lerp(b, 1.0), b);
        assert_eq!(a.lerp(b, 0.5), Color::rgb(100, 50, 25));
    }
}
