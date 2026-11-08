//! Reading one metadata entry out of an `.onnx` file.
//!
//! RapidOCR's PP-OCRv5 recognisers carry their character list in the
//! model's metadata (`character`, one character per line) instead of a
//! separate dictionary file, so one asset per script is enough. An ONNX file
//! is a protobuf `ModelProto`; its `metadata_props` (field 14) are
//! `{key = 1, value = 2}` string pairs. Walking the top-level fields is a
//! few dozen lines, which saves opening an ONNX Runtime session just to
//! read a string.

const METADATA_PROPS: u64 = 14;

/// The value stored under `key`, if the model has one.
pub fn metadata(model: &[u8], key: &str) -> Option<String> {
    let mut r = Reader { buf: model, pos: 0 };
    while let Some((field, wire)) = r.tag() {
        match (field, wire) {
            (METADATA_PROPS, 2) => {
                let entry = r.bytes()?;
                if let Some((k, v)) = entry_pair(entry) {
                    if k == key {
                        return Some(v);
                    }
                }
            }
            _ => r.skip(wire)?,
        }
    }
    None
}

fn entry_pair(buf: &[u8]) -> Option<(String, String)> {
    let mut r = Reader { buf, pos: 0 };
    let (mut key, mut value) = (None, None);
    while let Some((field, wire)) = r.tag() {
        match (field, wire) {
            (1, 2) => key = Some(String::from_utf8_lossy(r.bytes()?).into_owned()),
            (2, 2) => value = Some(String::from_utf8_lossy(r.bytes()?).into_owned()),
            _ => r.skip(wire)?,
        }
    }
    Some((key?, value.unwrap_or_default()))
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn varint(&mut self) -> Option<u64> {
        let mut out = 0u64;
        for shift in (0..64).step_by(7) {
            let b = *self.buf.get(self.pos)?;
            self.pos += 1;
            out |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Some(out);
            }
        }
        None
    }

    fn tag(&mut self) -> Option<(u64, u8)> {
        if self.pos >= self.buf.len() {
            return None;
        }
        let t = self.varint()?;
        Some((t >> 3, (t & 7) as u8))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = usize::try_from(self.varint()?).ok()?;
        let end = self.pos.checked_add(len)?;
        let out = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(out)
    }

    fn skip(&mut self, wire: u8) -> Option<()> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => self.pos = self.pos.checked_add(8).filter(|&p| p <= self.buf.len())?,
            2 => {
                self.bytes()?;
            }
            5 => self.pos = self.pos.checked_add(4).filter(|&p| p <= self.buf.len())?,
            // Groups (3, 4) are not used by ONNX; anything else is damage.
            _ => return None,
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(mut n: u64, out: &mut Vec<u8>) {
        loop {
            let b = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    fn field_bytes(field: u64, data: &[u8], out: &mut Vec<u8>) {
        varint(field << 3 | 2, out);
        varint(data.len() as u64, out);
        out.extend_from_slice(data);
    }

    fn entry(k: &str, v: &str) -> Vec<u8> {
        let mut e = Vec::new();
        field_bytes(1, k.as_bytes(), &mut e);
        field_bytes(2, v.as_bytes(), &mut e);
        e
    }

    #[test]
    fn finds_the_character_list_among_other_fields() {
        let mut model = Vec::new();
        varint(1 << 3, &mut model); // ir_version = 8
        varint(8, &mut model);
        field_bytes(2, b"paddle", &mut model); // producer_name
        field_bytes(7, &[0u8; 300], &mut model); // a "graph"
        model.extend_from_slice(&[0x5d, 1, 2, 3, 4]); // field 11, fixed32
        field_bytes(14, &entry("shape", "[3, 48, 320]"), &mut model);
        field_bytes(14, &entry("character", "a\nб\n日"), &mut model);
        assert_eq!(metadata(&model, "character").as_deref(), Some("a\nб\n日"));
        assert_eq!(metadata(&model, "shape").as_deref(), Some("[3, 48, 320]"));
        assert_eq!(metadata(&model, "missing"), None);
    }

    #[test]
    fn damaged_input_is_none_not_a_panic() {
        assert_eq!(metadata(&[], "character"), None);
        assert_eq!(metadata(&[0x72, 0xff, 0xff, 0xff, 0xff, 0x0f], "character"), None);
        assert_eq!(metadata(&[0x0b], "character"), None);
    }
}
