//! The fonts installed on the system, for a font picker: their family names
//! and the files that hold them.
//!
//! On Windows and macOS the font folders are scanned and each file's `name`
//! table is read (only the table directory and the name table, not the whole
//! file); on Linux fontconfig is asked (`fc-list`), with the same scan of the
//! usual folders where it is missing. The first call does the work, which
//! takes a moment with hundreds of fonts installed: call it from a worker
//! thread. Later calls return the cached answer.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// One face of an installed font.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontFace {
    /// The family name (`name` ID 1, in English where the font has it).
    pub family: String,
    /// The style within the family ("Regular", "Bold"…).
    pub style: String,
    pub path: PathBuf,
    /// The face's index in a collection (`.ttc`), else 0.
    pub index: u32,
}

/// Every installed face. Slow the first time; see the module docs.
pub fn installed() -> &'static [FontFace] {
    static FACES: OnceLock<Vec<FontFace>> = OnceLock::new();
    FACES.get_or_init(scan)
}

/// The installed family names, sorted, each once.
pub fn families() -> Vec<String> {
    let mut names: Vec<String> = installed().iter().map(|f| f.family.clone()).collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// The file (and collection index) of `family`'s regular face, or of its
/// first face when it has no regular one. Matching ignores case.
pub fn find_family(family: &str) -> Option<(PathBuf, u32)> {
    let faces: Vec<&FontFace> = installed().iter().filter(|f| f.family.eq_ignore_ascii_case(family.trim())).collect();
    let regular = |f: &&&FontFace| matches!(f.style.to_lowercase().as_str(), "regular" | "normal" | "book" | "roman");
    faces.iter().find(regular).or(faces.first()).map(|f| (f.path.clone(), f.index))
}

/// `family`'s file from the system's own list of installed fonts, without
/// opening any font file: quick enough for the UI thread. Windows keeps that
/// list in the registry ("Times New Roman (TrueType)" → `times.ttf`); other
/// systems give `None` here (use [`find_family`]).
pub fn registered_family(family: &str) -> Option<(PathBuf, u32)> {
    #[cfg(windows)]
    {
        win::registered(family.trim())
    }
    #[cfg(not(windows))]
    {
        let _ = family;
        None
    }
}

/// Whether a registry font entry named `name` ("Cambria & Cambria Math
/// (TrueType)") holds `family`, and at which index of the collection.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn registry_entry_index(name: &str, family: &str) -> Option<u32> {
    let name = name.trim();
    // The kind in parentheses at the end, if any.
    let faces = match name.rfind(" (") {
        Some(at) if name.ends_with(')') => &name[..at],
        _ => name,
    };
    faces.split(" & ").position(|f| f.trim().eq_ignore_ascii_case(family)).map(|i| i as u32)
}

#[cfg(windows)]
mod win {
    use std::path::PathBuf;

    use windows::core::{w, PWSTR};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ,
    };

    pub fn registered(family: &str) -> Option<(PathBuf, u32)> {
        let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let fonts = windir.join("Fonts");
        // Per-user fonts (with full paths) first: a user's copy wins.
        for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let mut key = HKEY::default();
            // SAFETY: a constant, NUL-terminated key name and a valid out
            // pointer; the key is closed below.
            let rc = unsafe {
                RegOpenKeyExW(root, w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts"), None, KEY_READ, &mut key)
            };
            if rc != ERROR_SUCCESS {
                continue;
            }
            let found = find_in(key, family, &fonts);
            // SAFETY: `key` was opened above.
            unsafe {
                let _ = RegCloseKey(key);
            }
            if found.is_some() {
                return found;
            }
        }
        None
    }

    fn find_in(key: HKEY, family: &str, fonts: &std::path::Path) -> Option<(PathBuf, u32)> {
        let mut name = [0u16; 512];
        let mut data = [0u16; 520];
        for i in 0..4096u32 {
            let mut name_len = name.len() as u32;
            let mut data_len = (data.len() * 2) as u32;
            let mut kind = 0u32;
            // SAFETY: the buffers and their lengths (characters for the
            // name, bytes for the data) describe the arrays above.
            let rc = unsafe {
                RegEnumValueW(
                    key,
                    i,
                    Some(PWSTR(name.as_mut_ptr())),
                    &mut name_len,
                    None,
                    Some(&mut kind),
                    Some(data.as_mut_ptr().cast()),
                    Some(&mut data_len),
                )
            };
            if rc.0 == 259 {
                // ERROR_NO_MORE_ITEMS
                break;
            }
            if rc != ERROR_SUCCESS || kind != REG_SZ.0 {
                continue;
            }
            let entry = String::from_utf16_lossy(&name[..name_len as usize]);
            let Some(index) = super::registry_entry_index(&entry, family) else { continue };
            let chars = (data_len as usize / 2).min(data.len());
            let file = String::from_utf16_lossy(&data[..chars]);
            let file = file.trim_end_matches('\0').trim();
            let path = PathBuf::from(file);
            let path = if path.is_absolute() { path } else { fonts.join(path) };
            if path.is_file() {
                return Some((path, index));
            }
        }
        None
    }
}

fn scan() -> Vec<FontFace> {
    #[cfg(target_os = "linux")]
    if let Some(faces) = fc_list() {
        return faces;
    }
    let mut faces = Vec::new();
    for dir in font_dirs() {
        scan_dir(&dir, &mut faces, 0);
    }
    faces
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        dirs.push(windir.join("Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/Library/Fonts"));
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join("Library/Fonts"));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        dirs.push(PathBuf::from("/usr/share/fonts"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".local/share/fonts"));
            dirs.push(home.join(".fonts"));
        }
    }
    dirs
}

fn scan_dir(dir: &Path, faces: &mut Vec<FontFace>, depth: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < 4 {
                scan_dir(&path, faces, depth + 1);
            }
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !matches!(ext.as_str(), "ttf" | "ttc" | "otf" | "otc") {
            continue;
        }
        for (index, family, style) in read_names(&path).unwrap_or_default() {
            faces.push(FontFace { family, style, path: path.clone(), index });
        }
    }
}

/// fontconfig's list of faces, if `fc-list` is there.
#[cfg(target_os = "linux")]
fn fc_list() -> Option<Vec<FontFace>> {
    let out = std::process::Command::new("fc-list")
        .args(["--format", "%{family[0]}\t%{style[0]}\t%{file}\t%{index}\n"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let faces: Vec<FontFace> = text
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let family = parts.next()?.trim().to_owned();
            let style = parts.next()?.trim().to_owned();
            let path = PathBuf::from(parts.next()?.trim());
            let index = parts.next().and_then(|i| i.trim().parse().ok()).unwrap_or(0);
            (!family.is_empty()).then_some(FontFace { family, style, path, index })
        })
        .collect();
    (!faces.is_empty()).then_some(faces)
}

/// The (face index, family, style) of every face in a font file, from its
/// `name` tables.
pub fn read_names(path: &Path) -> std::io::Result<Vec<(u32, String, String)>> {
    let mut f = File::open(path)?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head)?;
    let offsets: Vec<u32> = if &head[0..4] == b"ttcf" {
        let n = u32::from_be_bytes([head[8], head[9], head[10], head[11]]).min(256);
        let mut buf = vec![0u8; n as usize * 4];
        f.read_exact(&mut buf)?;
        buf.as_chunks::<4>().0.iter().map(|c| u32::from_be_bytes(*c)).collect()
    } else {
        vec![0]
    };
    let mut out = Vec::new();
    for (index, offset) in offsets.into_iter().enumerate() {
        if let Some((family, style)) = face_names(&mut f, offset as u64)? {
            out.push((index as u32, family, style));
        }
    }
    Ok(out)
}

/// The family and style of the face whose table directory is at `offset`.
fn face_names(f: &mut File, offset: u64) -> std::io::Result<Option<(String, String)>> {
    f.seek(SeekFrom::Start(offset))?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head)?;
    let tables = u16::from_be_bytes([head[4], head[5]]).min(512) as usize;
    let mut dir = vec![0u8; tables * 16];
    f.read_exact(&mut dir)?;
    let Some(record) = dir.as_chunks::<16>().0.iter().find(|r| &r[0..4] == b"name") else { return Ok(None) };
    let at = u32::from_be_bytes([record[8], record[9], record[10], record[11]]) as u64;
    let len = u32::from_be_bytes([record[12], record[13], record[14], record[15]]).min(1 << 20) as usize;
    f.seek(SeekFrom::Start(at))?;
    let mut table = vec![0u8; len];
    f.read_exact(&mut table)?;
    Ok(parse_name_table(&table))
}

/// The best English family (ID 1) and style (ID 2) names in a `name` table.
pub fn parse_name_table(t: &[u8]) -> Option<(String, String)> {
    let u16_at = |i: usize| t.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let count = u16_at(2)? as usize;
    let strings = u16_at(4)? as usize;
    // (score, text) for each wanted name ID.
    let mut best: [(u8, String); 2] = [(0, String::new()), (0, String::new())];
    for i in 0..count {
        let r = 6 + i * 12;
        let (platform, encoding, language, id) = (u16_at(r)?, u16_at(r + 2)?, u16_at(r + 4)?, u16_at(r + 6)?);
        let (len, off) = (u16_at(r + 8)? as usize, u16_at(r + 10)? as usize);
        let slot = match id {
            1 => 0,
            2 => 1,
            _ => continue,
        };
        let score = match (platform, encoding, language) {
            (3, 1 | 10, 0x0409) => 4,
            (0, _, _) => 3,
            (1, 0, 0) => 2,
            (3, 1 | 10, _) => 1,
            _ => continue,
        };
        if score <= best[slot].0 {
            continue;
        }
        let Some(bytes) = t.get(strings + off..strings + off + len) else { continue };
        let text = if platform == 1 {
            bytes.iter().map(|&b| b as char).collect::<String>()
        } else {
            let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
            String::from_utf16_lossy(&units)
        };
        let text = text.trim().to_owned();
        if !text.is_empty() {
            best[slot] = (score, text);
        }
    }
    let [(_, family), (_, style)] = best;
    (!family.is_empty()).then_some((family, style))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `name` table with the given (platform, encoding, language, id, text)
    /// records, UTF-16BE except on platform 1.
    fn name_table(records: &[(u16, u16, u16, u16, &str)]) -> Vec<u8> {
        let mut strings = Vec::new();
        let mut recs = Vec::new();
        for &(p, e, l, id, text) in records {
            let bytes: Vec<u8> = if p == 1 {
                text.bytes().collect()
            } else {
                text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
            };
            for v in [p, e, l, id, bytes.len() as u16, strings.len() as u16] {
                recs.extend_from_slice(&v.to_be_bytes());
            }
            strings.extend(bytes);
        }
        let mut t = Vec::new();
        for v in [0u16, records.len() as u16, (6 + recs.len()) as u16] {
            t.extend_from_slice(&v.to_be_bytes());
        }
        t.extend(recs);
        t.extend(strings);
        t
    }

    #[test]
    fn registry_entries_name_their_faces() {
        assert_eq!(registry_entry_index("Times New Roman (TrueType)", "times new roman"), Some(0));
        assert_eq!(registry_entry_index("Cambria & Cambria Math (TrueType)", "Cambria Math"), Some(1));
        assert_eq!(registry_entry_index("Segoe UI Bold (TrueType)", "Segoe UI"), None);
        assert_eq!(registry_entry_index("Inter", "Inter"), Some(0));
        assert_eq!(registry_entry_index("Arial (TrueType)", "Arial Black"), None);
    }

    #[test]
    fn english_windows_names_win() {
        let t = name_table(&[
            (1, 0, 0, 1, "Mac Family"),
            (3, 1, 0x0419, 1, "Семейство"),
            (3, 1, 0x0409, 1, "Segoe UI"),
            (3, 1, 0x0409, 2, "Regular"),
            (3, 1, 0x0409, 4, "Segoe UI Regular"),
        ]);
        assert_eq!(parse_name_table(&t), Some(("Segoe UI".into(), "Regular".into())));
    }

    #[test]
    fn other_platforms_stand_in() {
        let t = name_table(&[(1, 0, 0, 1, "Helvetica"), (1, 0, 0, 2, "Bold")]);
        assert_eq!(parse_name_table(&t), Some(("Helvetica".into(), "Bold".into())));
        let t = name_table(&[(3, 1, 0x0411, 1, "メイリオ")]);
        assert_eq!(parse_name_table(&t), Some(("メイリオ".into(), String::new())));
    }

    #[test]
    fn broken_tables_give_nothing() {
        assert_eq!(parse_name_table(&[]), None);
        assert_eq!(parse_name_table(&[0, 0, 0, 5, 0, 66]), None);
        let mut t = name_table(&[(3, 1, 0x0409, 1, "Cut short")]);
        t.truncate(t.len() - 3);
        assert_eq!(parse_name_table(&t), None);
    }

    #[test]
    fn a_font_file_is_read_from_its_directory() {
        // A minimal sfnt: header, one table record, the name table.
        let name = name_table(&[(3, 1, 0x0409, 1, "Tiny"), (3, 1, 0x0409, 2, "Regular")]);
        let mut file = Vec::new();
        file.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        file.extend_from_slice(&1u16.to_be_bytes());
        file.extend_from_slice(&[0; 6]);
        file.extend_from_slice(b"name");
        file.extend_from_slice(&0u32.to_be_bytes());
        file.extend_from_slice(&28u32.to_be_bytes());
        file.extend_from_slice(&(name.len() as u32).to_be_bytes());
        file.extend(name);
        let dir = std::env::temp_dir().join(format!("glossa-fonts-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tiny.ttf");
        std::fs::write(&path, &file).unwrap();
        assert_eq!(read_names(&path).unwrap(), vec![(0, "Tiny".to_owned(), "Regular".to_owned())]);
        let mut faces = Vec::new();
        scan_dir(&dir, &mut faces, 0);
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].family, "Tiny");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
