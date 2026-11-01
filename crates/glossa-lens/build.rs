// Rust types for Chromium's lens_server_proto, from the descriptors in
// proto/lens.fds (see scripts/lens-descriptors.py and proto/README.md). No
// protoc is needed: the descriptors were compiled once, by chrome-lens-py's
// pinned protoc, and are checked in.
//
// 28 of the files use the editions syntax, which prost-build does not read,
// so each is handed over as the older syntax with the same field presence:
// proto2 for edition 2023's default (explicit presence), proto3 for a file
// that declares `option features.field_presence = IMPLICIT;`. The two write
// different bytes for a field set to its default (proto2 writes it, proto3
// and Chromium leave it out), so the choice is read from each file's own
// options rather than assumed. prost-types' FileOptions predates editions and
// drops the features field, hence the raw read below.
use prost::Message;

/// `FeatureSet.FieldPresence.IMPLICIT`.
const IMPLICIT: u64 = 2;

/// Fields that set their own presence against their file's. prost-build
/// decides presence from the file's syntax alone, so these keep the file's:
///
/// - `TextLayout.Word.text_separator` is EXPLICIT in an IMPLICIT file, and
///   comes out a `String`, so an absent separator reads as an empty one.
///   chrome-lens-py does the same (it joins `plain_text + text_separator`
///   without asking whether the separator is there), and Glossa follows it:
///   the distinction only matters when one line of text is rebuilt across
///   line and paragraph breaks, which neither does. Not a bug to "fix" into
///   a difference from chrome-lens-py.
/// - `ImageCrop.crop_id` is IMPLICIT in an EXPLICIT file, and comes out an
///   `Option<String>`. Neither Glossa nor chrome-lens-py sends it; mind it
///   when region (interaction) requests are added.
///
/// A re-pin that brings a new override fails the build here, so it is looked
/// at rather than lost quietly.
const KNOWN_FIELD_OVERRIDES: &[&str] = &["TextLayout.Word.text_separator", "ImageCrop.crop_id"];

fn main() {
    let path = "proto/lens.fds";
    println!("cargo:rerun-if-changed={path}");
    let bytes = std::fs::read(path).expect("proto/lens.fds");
    let implicit = implicit_presence_files(&bytes);
    let overrides = field_presence_overrides(&bytes);
    let unknown: Vec<&String> = overrides.iter().filter(|f| !KNOWN_FIELD_OVERRIDES.contains(&f.as_str())).collect();
    let gone: Vec<&&str> = KNOWN_FIELD_OVERRIDES.iter().filter(|k| !overrides.iter().any(|f| f == *k)).collect();
    assert!(
        unknown.is_empty() && gone.is_empty(),
        "field presence overrides changed (new: {unknown:?}, gone: {gone:?}): prost-build ignores them; update KNOWN_FIELD_OVERRIDES in build.rs"
    );
    let mut set = prost_types::FileDescriptorSet::decode(bytes.as_slice()).expect("a FileDescriptorSet");
    for file in &mut set.file {
        if file.syntax.as_deref() == Some("editions") {
            let name = file.name.as_deref().unwrap_or_default();
            file.syntax = Some(if implicit.iter().any(|n| n == name) { "proto3" } else { "proto2" }.into());
        }
    }
    prost_build::Config::new().compile_fds(set).expect("generating the Lens protos");
}

/// The files whose options set `features.field_presence = IMPLICIT`:
/// FileDescriptorSet.file (1) → FileDescriptorProto.options (8) →
/// FileOptions.features (50) → FeatureSet.field_presence (1).
fn implicit_presence_files(set: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for (number, value) in fields(set) {
        let Field::Bytes(file) = value else { continue };
        if number != 1 {
            continue;
        }
        let mut name = None;
        let mut implicit = false;
        for (n, v) in fields(file) {
            match (n, v) {
                (1, Field::Bytes(b)) => name = Some(String::from_utf8_lossy(b).into_owned()),
                (8, Field::Bytes(options)) => {
                    for (on, ov) in fields(options) {
                        if let (50, Field::Bytes(features)) = (on, ov) {
                            implicit |= fields(features).any(|f| matches!(f, (1, Field::Varint(IMPLICIT))));
                        }
                    }
                }
                _ => {}
            }
        }
        if let (Some(name), true) = (name, implicit) {
            out.push(name);
        }
    }
    out
}

/// Every field whose options set `features.field_presence` (FieldOptions
/// field 21), as `Message.Nested.field`.
fn field_presence_overrides(set: &[u8]) -> Vec<String> {
    fn message(msg: &[u8], prefix: &str, out: &mut Vec<String>) {
        let name = fields(msg)
            .find_map(|f| match f {
                (1, Field::Bytes(b)) => Some(String::from_utf8_lossy(b).into_owned()),
                _ => None,
            })
            .unwrap_or_default();
        let path = format!("{prefix}{name}");
        for f in fields(msg) {
            match f {
                // FieldDescriptorProto: its name (1) and options (8).
                (2, Field::Bytes(field)) => {
                    let mut field_name = String::new();
                    let mut overrides = false;
                    for (n, v) in fields(field) {
                        match (n, v) {
                            (1, Field::Bytes(b)) => field_name = String::from_utf8_lossy(b).into_owned(),
                            (8, Field::Bytes(options)) => {
                                overrides |= fields(options).any(|(on, ov)| {
                                    on == 21
                                        && matches!(ov, Field::Bytes(features)
                                            if fields(features).any(|(fnum, _)| fnum == 1))
                                })
                            }
                            _ => {}
                        }
                    }
                    if overrides {
                        out.push(format!("{path}.{field_name}"));
                    }
                }
                // A nested message.
                (3, Field::Bytes(nested)) => message(nested, &format!("{path}."), out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for (number, value) in fields(set) {
        if let (1, Field::Bytes(file)) = (number, value) {
            for f in fields(file) {
                if let (4, Field::Bytes(msg)) = f {
                    message(msg, "", &mut out);
                }
            }
        }
    }
    out
}

enum Field<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Other,
}

/// The top-level fields of a protobuf message.
fn fields(mut data: &[u8]) -> impl Iterator<Item = (u64, Field<'_>)> {
    std::iter::from_fn(move || {
        if data.is_empty() {
            return None;
        }
        let key = varint(&mut data)?;
        let field = match key & 7 {
            0 => Field::Varint(varint(&mut data)?),
            2 => {
                let len = varint(&mut data)? as usize;
                let (value, rest) = data.split_at_checked(len)?;
                data = rest;
                Field::Bytes(value)
            }
            1 => {
                data = data.get(8..)?;
                Field::Other
            }
            5 => {
                data = data.get(4..)?;
                Field::Other
            }
            _ => return None,
        };
        Some((key >> 3, field))
    })
}

fn varint(data: &mut &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = data.split_first()?;
        *data = rest;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}
