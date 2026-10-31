//! The handful of legacy code pages anything here still meets.
//!
//! Classic service scripts declared a code page per request — UTF-8,
//! Windows-1251 (Russian sites) or ISO-8859-1 — and some dictionary sites
//! still serve Windows-1251 today. A full encoding library is a lot of tables
//! for three encodings, so the two single-byte ones are decoded here.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Codepage {
    Utf8,
    Windows1251,
    Latin1,
}

impl Codepage {
    /// From a classic script's numeric code page.
    pub fn from_number(n: u32) -> Option<Codepage> {
        match n {
            65001 => Some(Codepage::Utf8),
            1251 => Some(Codepage::Windows1251),
            28591 | 1252 => Some(Codepage::Latin1),
            _ => None,
        }
    }

    /// From an HTTP `charset=` label.
    pub fn from_label(label: &str) -> Option<Codepage> {
        match label.trim().to_ascii_lowercase().as_str() {
            "utf-8" | "utf8" => Some(Codepage::Utf8),
            "windows-1251" | "cp1251" | "x-cp1251" | "win-1251" => Some(Codepage::Windows1251),
            "iso-8859-1" | "latin1" | "latin-1" | "windows-1252" | "cp1252" | "us-ascii" | "ascii" => {
                Some(Codepage::Latin1)
            }
            _ => None,
        }
    }

    pub fn decode(self, bytes: &[u8]) -> String {
        match self {
            Codepage::Utf8 => {
                let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
                String::from_utf8_lossy(bytes).into_owned()
            }
            Codepage::Latin1 => bytes.iter().map(|&b| b as char).collect(),
            Codepage::Windows1251 => {
                bytes.iter().map(|&b| if b < 0x80 { b as char } else { CP1251_HIGH[(b - 0x80) as usize] }).collect()
            }
        }
    }

    /// Encode text for a request body that must go out in this code page.
    /// Characters the code page cannot hold become `?`.
    pub fn encode(self, text: &str) -> Vec<u8> {
        match self {
            Codepage::Utf8 => text.as_bytes().to_vec(),
            Codepage::Latin1 => text.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect(),
            Codepage::Windows1251 => text
                .chars()
                .map(|c| {
                    if (c as u32) < 0x80 {
                        c as u8
                    } else {
                        CP1251_HIGH.iter().position(|&h| h == c).map(|i| 0x80 + i as u8).unwrap_or(b'?')
                    }
                })
                .collect(),
        }
    }
}

/// Windows-1251, bytes 0x80..=0xFF. 0x98 is unassigned and maps to U+FFFD.
const CP1251_HIGH: [char; 128] = [
    'Ђ', 'Ѓ', '‚', 'ѓ', '„', '…', '†', '‡', '€', '‰', 'Љ', '‹', 'Њ', 'Ќ', 'Ћ', 'Џ', //
    'ђ', '‘', '’', '“', '”', '•', '–', '—', '\u{fffd}', '™', 'љ', '›', 'њ', 'ќ', 'ћ', 'џ', //
    '\u{a0}', 'Ў', 'ў', 'Ј', '¤', 'Ґ', '¦', '§', 'Ё', '©', 'Є', '«', '¬', '\u{ad}', '®', 'Ї', //
    '°', '±', 'І', 'і', 'ґ', 'µ', '¶', '·', 'ё', '№', 'є', '»', 'ј', 'Ѕ', 'ѕ', 'ї', //
    'А', 'Б', 'В', 'Г', 'Д', 'Е', 'Ж', 'З', 'И', 'Й', 'К', 'Л', 'М', 'Н', 'О', 'П', //
    'Р', 'С', 'Т', 'У', 'Ф', 'Х', 'Ц', 'Ч', 'Ш', 'Щ', 'Ъ', 'Ы', 'Ь', 'Э', 'Ю', 'Я', //
    'а', 'б', 'в', 'г', 'д', 'е', 'ж', 'з', 'и', 'й', 'к', 'л', 'м', 'н', 'о', 'п', //
    'р', 'с', 'т', 'у', 'ф', 'х', 'ц', 'ч', 'ш', 'щ', 'ъ', 'ы', 'ь', 'э', 'ю', 'я', //
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp1251_round_trips_cyrillic() {
        let text = "Съешь же ещё этих мягких французских булок, да выпей чаю. Ёё №";
        let bytes = Codepage::Windows1251.encode(text);
        assert_eq!(bytes.len(), text.chars().count(), "one byte per character");
        assert_eq!(Codepage::Windows1251.decode(&bytes), text);
    }

    #[test]
    fn cp1251_known_bytes() {
        assert_eq!(Codepage::Windows1251.decode(&[0xc0, 0xff, 0xa8, 0xb8, 0xb9]), "АяЁё№");
        assert_eq!(Codepage::Windows1251.encode("日"), b"?");
    }

    #[test]
    fn latin1_and_utf8() {
        assert_eq!(Codepage::Latin1.decode(&[0x63, 0x61, 0x66, 0xe9]), "café");
        assert_eq!(Codepage::Utf8.decode(b"\xef\xbb\xbfhi"), "hi", "BOM is dropped");
        assert_eq!(Codepage::Utf8.decode(b"\xff"), "\u{fffd}", "invalid bytes are replaced, not fatal");
    }

    #[test]
    fn labels_and_numbers() {
        assert_eq!(Codepage::from_label("Windows-1251"), Some(Codepage::Windows1251));
        assert_eq!(Codepage::from_label(" UTF-8 "), Some(Codepage::Utf8));
        assert_eq!(Codepage::from_label("koi8-r"), None);
        assert_eq!(Codepage::from_number(1251), Some(Codepage::Windows1251));
        assert_eq!(Codepage::from_number(65001), Some(Codepage::Utf8));
        assert_eq!(Codepage::from_number(28591), Some(Codepage::Latin1));
    }
}
