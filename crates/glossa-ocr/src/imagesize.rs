//! The pixel size of an encoded picture, read from its header.
//!
//! Apple Vision reports boxes as fractions of the picture, so they need the
//! size to become pixels; decoding the whole image for two numbers (or adding
//! an image library to the command line for them) would be waste.

/// Width and height of PNG, JPEG, GIF, BMP or WebP data, or `None` for
/// anything else or a truncated header.
pub fn dimensions(b: &[u8]) -> Option<(u32, u32)> {
    let be32 = |i: usize| b.get(i..i + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]));
    let le16 = |i: usize| b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as u32);
    let le32 = |i: usize| b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let size = if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        (be32(16)?, be32(20)?)
    } else if b.starts_with(&[0xFF, 0xD8]) {
        jpeg(b)?
    } else if b.starts_with(b"GIF8") {
        (le16(6)?, le16(8)?)
    } else if b.starts_with(b"BM") {
        // Height is negative for top-down bitmaps.
        (le32(18)?, (le32(22)? as i32).unsigned_abs())
    } else if b.len() >= 30 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        match &b[12..16] {
            b"VP8 " => (le16(26)? & 0x3FFF, le16(28)? & 0x3FFF),
            b"VP8L" => {
                let bits = le32(21)?;
                ((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1)
            }
            b"VP8X" => {
                let le24 = |i: usize| b.get(i..i + 3).map(|s| u32::from_le_bytes([s[0], s[1], s[2], 0]));
                (le24(24)? + 1, le24(27)? + 1)
            }
            _ => return None,
        }
    } else {
        return None;
    };
    (size.0 > 0 && size.1 > 0).then_some(size)
}

/// Walk the JPEG markers to the first start-of-frame.
fn jpeg(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        if marker == 0xFF {
            i += 1; // fill byte
            continue;
        }
        if matches!(marker, 0xD8 | 0x01 | 0xD0..=0xD7) {
            i += 2; // no length
            continue;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        // SOF0–SOF15, except DHT (C4), JPG (C8) and DAC (CC).
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let s = b.get(i + 5..i + 9)?;
            return Some((u16::from_be_bytes([s[2], s[3]]) as u32, u16::from_be_bytes([s[0], s[1]]) as u32));
        }
        i += 2 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png() {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        b.extend(640u32.to_be_bytes());
        b.extend(480u32.to_be_bytes());
        assert_eq!(dimensions(&b), Some((640, 480)));
        assert_eq!(dimensions(&b[..20]), None, "truncated");
    }

    #[test]
    fn jpeg_skips_to_the_frame_header() {
        let mut b = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]; // APP0, 2 bytes of payload
        b.extend([0xFF, 0xC4, 0x00, 0x02]); // an empty DHT, which is not a frame
        b.extend([0xFF, 0xC2, 0x00, 0x11, 0x08, 0x01, 0xE0, 0x02, 0x80]); // progressive, 640×480
        assert_eq!(dimensions(&b), Some((640, 480)));
        assert_eq!(dimensions(&[0xFF, 0xD8, 0x00]), None);
    }

    #[test]
    fn gif_bmp_webp() {
        assert_eq!(dimensions(b"GIF89a\x20\x03\x58\x02rest"), Some((800, 600)));
        let mut bmp = b"BM".to_vec();
        bmp.resize(18, 0);
        bmp.extend(100u32.to_le_bytes());
        bmp.extend((-50i32).to_le_bytes());
        assert_eq!(dimensions(&bmp), Some((100, 50)), "top-down");
        let mut vp8x = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
        vp8x.resize(24, 0);
        vp8x.extend([0x1F, 0x03, 0x00]); // width - 1 = 799
        vp8x.extend([0x57, 0x02, 0x00]); // height - 1 = 599
        assert_eq!(dimensions(&vp8x), Some((800, 600)));
        assert_eq!(dimensions(b"not an image at all"), None);
    }
}
