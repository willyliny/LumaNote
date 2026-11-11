//! File-name and text helpers shared by the file tools.
//!
//! The product ships on Windows only, so file names are always sanitised with the
//! Windows rules (reserved device names, forbidden characters, trailing dots/spaces),
//! on every platform. The helpers are pure and are tested on Linux CI too.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::schema::{LumaError, Result};

/// Characters that are not allowed in Windows file names (plus `:` which would open an
/// NTFS alternate data stream).
const FORBIDDEN: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Longest file-name stem kept (the full path must stay below the 260 character limit).
pub const MAX_STEM_CHARS: usize = 100;

/// How many `name (2).md`, `name (3).md`, ... candidates are tried before giving up.
const MAX_NAME_ATTEMPTS: u32 = 1000;

fn is_bidi_or_zero_width(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// True for Windows device names (`CON`, `NUL`, `COM1`, ...), with or without an extension.
pub fn is_reserved_windows_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end_matches(' ');
    let upper = stem.to_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let mut chars = upper.chars();
    let head: String = chars.by_ref().take(3).collect();
    if head != "COM" && head != "LPT" {
        return false;
    }
    let rest: Vec<char> = chars.collect();
    matches!(rest.as_slice(), [d] if d.is_ascii_digit() || matches!(d, '\u{B9}' | '\u{B2}' | '\u{B3}'))
}

/// Makes one path component safe on Windows: forbidden and control characters become `_`,
/// trailing dots and spaces are removed, reserved device names get a `_` prefix, and the
/// result is never empty.
pub fn sanitize_component(raw: &str) -> String {
    let replaced: String = raw
        .chars()
        .map(|c| {
            if FORBIDDEN.contains(&c) || c.is_control() || is_bidi_or_zero_width(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = replaced.trim_start().trim_end_matches(['.', ' ']);
    let mut out = if trimmed.is_empty() {
        "untitled".to_owned()
    } else {
        trimmed.to_owned()
    };
    if is_reserved_windows_name(&out) {
        out.insert(0, '_');
    }
    out
}

/// True if `raw` names an absolute location on Windows or Unix (`C:\x`, `\x`, `/x`, `\\srv\x`).
pub fn looks_absolute(raw: &str) -> bool {
    let b = raw.as_bytes();
    raw.starts_with('/')
        || raw.starts_with('\\')
        || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
        || Path::new(raw).is_absolute()
}

/// Turns a model-supplied relative path into sanitised components ending in a `.md` file
/// name. Rejects `..`, other extensions and empty input. `.` and empty parts are skipped.
pub fn sanitize_relative_md(raw: &str) -> Result<Vec<String>> {
    let parts: Vec<&str> = raw
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let Some((last, dirs)) = parts.split_last() else {
        return Err(LumaError::invalid_input("path is empty"));
    };
    if parts.contains(&"..") {
        return Err(LumaError::NoPermission {
            action: "write".into(),
            resource: format!("{raw} (path traversal)"),
        });
    }
    let mut out: Vec<String> = dirs.iter().map(|d| sanitize_component(d)).collect();
    let lower = last.to_lowercase();
    let stem_raw = if lower.ends_with(".md") {
        &last[..last.len() - 3]
    } else {
        // `.txt`, `.exe`, `.ps1`: a short alphanumeric extension with a letter that is not .md
        // (`v1.2` is just a name containing a dot)
        if let Some((_, ext)) = last.rsplit_once('.') {
            if (1..=5).contains(&ext.chars().count())
                && ext.chars().all(|c| c.is_alphanumeric())
                && ext.chars().any(|c| c.is_alphabetic())
            {
                return Err(LumaError::invalid_input(format!(
                    "write_markdown only writes .md files (got extension .{ext}); omit the extension or use .md"
                )));
            }
        }
        last
    };
    let mut stem = sanitize_component(stem_raw);
    if stem.chars().count() > MAX_STEM_CHARS {
        stem = stem.chars().take(MAX_STEM_CHARS).collect();
        stem = stem.trim_end_matches(['.', ' ']).to_owned();
    }
    if is_reserved_windows_name(&stem) {
        stem.insert(0, '_');
    }
    out.push(format!("{stem}.md"));
    Ok(out)
}

/// `a.md` for attempt 1, `a (2).md`, `a (3).md`, ... afterwards.
pub fn numbered_name(file_name: &str, attempt: u32) -> String {
    if attempt <= 1 {
        return file_name.to_owned();
    }
    match file_name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem} ({attempt}).{ext}"),
        None => format!("{file_name} ({attempt})"),
    }
}

fn io_failure(what: &str, e: &std::io::Error) -> LumaError {
    LumaError::ExternalFailure {
        message: format!("{what}: {e}"),
        retryable: false,
    }
}

/// Creates a new file in `dir` and writes `bytes`, never overwriting: the name is
/// tried as given and then with ` (2)`, ` (3)`, ... until `create_new` succeeds
/// (atomic, so a concurrent writer cannot be clobbered). Returns the path used.
pub fn create_unique_file(dir: &Path, file_name: &str, bytes: &[u8]) -> Result<PathBuf> {
    for attempt in 1..=MAX_NAME_ATTEMPTS {
        let path = dir.join(numbered_name(file_name, attempt));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                let written = f.write_all(bytes).and_then(|()| f.sync_all());
                if let Err(e) = written {
                    drop(f);
                    // only remove the file this call just created
                    let _ = std::fs::remove_file(&path);
                    return Err(io_failure("cannot write file", &e));
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            // Windows reports "access denied" when the name belongs to a directory.
            Err(_) if std::fs::symlink_metadata(&path).is_ok() => {}
            Err(e) => return Err(io_failure("cannot create file", &e)),
        }
    }
    Err(LumaError::ExternalFailure {
        message: format!("no free file name for {file_name} after {MAX_NAME_ATTEMPTS} attempts"),
        retryable: false,
    })
}

/// Decodes file bytes as text: UTF-8 (BOM removed) or UTF-16 with a BOM, as written by
/// Windows Notepad. `cut` says the bytes were truncated at a size cap, so a character
/// split at the end is dropped instead of being an error. Binary data (NUL bytes) and
/// other encodings are rejected. Returns the text and the encoding name.
pub fn decode_text(bytes: &[u8], cut: bool) -> Result<(String, &'static str)> {
    let bad = |m: String| LumaError::invalid_input(m);
    let text = if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        (utf8(rest, cut)?, "utf-8")
    } else if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        (utf16(rest, cut, u16::from_le_bytes)?, "utf-16le")
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        (utf16(rest, cut, u16::from_be_bytes)?, "utf-16be")
    } else {
        (utf8(bytes, cut)?, "utf-8")
    };
    if text.0.contains('\0') {
        return Err(bad("file looks binary (contains NUL bytes)".into()));
    }
    Ok(text)
}

fn utf8(bytes: &[u8], cut: bool) -> Result<String> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s.to_owned()),
        Err(e) if cut && e.error_len().is_none() => {
            Ok(String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned())
        }
        Err(e) => Err(LumaError::invalid_input(format!(
            "file is not valid UTF-8 text (invalid byte at offset {})",
            e.valid_up_to()
        ))),
    }
}

fn utf16(bytes: &[u8], cut: bool, from: fn([u8; 2]) -> u16) -> Result<String> {
    let mut units: Vec<u16> = bytes.chunks_exact(2).map(|c| from([c[0], c[1]])).collect();
    if cut && units.last().is_some_and(|u| (0xD800..0xDC00).contains(u)) {
        units.pop(); // high surrogate whose partner was cut off
    }
    String::from_utf16(&units).map_err(|_| LumaError::invalid_input("file is not valid UTF-16 text"))
}

/// Largest index `<= max` that lies on a UTF-8 character boundary of `s`.
pub fn floor_char_boundary(s: &str, max: usize) -> usize {
    let mut i = max.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_and_special_names_are_made_safe() {
        for n in [
            "CON",
            "con",
            "Nul.txt",
            "COM1",
            "lpt9.md",
            "AUX ",
            "COM\u{b9}",
            "CONIN$",
        ] {
            assert!(is_reserved_windows_name(n), "{n}");
        }
        for n in ["CONSOLE", "COM10", "COM", "notes", "LPT", "xCON"] {
            assert!(!is_reserved_windows_name(n), "{n}");
        }
        assert_eq!(sanitize_component("a<b>c:d\"e|f?g*h"), "a_b_c_d_e_f_g_h");
        assert_eq!(sanitize_component("report. "), "report");
        assert_eq!(sanitize_component("..."), "untitled");
        assert_eq!(sanitize_component("   "), "untitled");
        assert_eq!(sanitize_component("CON"), "_CON");
        assert_eq!(sanitize_component("tab\tnew\nline"), "tab_new_line");
        assert_eq!(sanitize_component("evil\u{202E}gnp"), "evil_gnp");
        assert_eq!(sanitize_component("會議紀錄 11/12"), "會議紀錄 11_12");
        assert_eq!(sanitize_component("note.txt:stream"), "note.txt_stream");
    }

    #[test]
    fn relative_markdown_paths() {
        let s = |r: &str| sanitize_relative_md(r).unwrap();
        assert_eq!(s("notes"), vec!["notes.md"]);
        assert_eq!(s("Notes.MD"), vec!["Notes.md"]);
        assert_eq!(s("sub/dir\\file"), vec!["sub", "dir", "file.md"]);
        assert_eq!(s("./a//b.md"), vec!["a", "b.md"]);
        assert_eq!(s("con"), vec!["_con.md"]);
        assert_eq!(s("v1.2 plan"), vec!["v1.2 plan.md"]);
        assert_eq!(s("bad:name?.md"), vec!["bad_name_.md"]);
        assert_eq!(s("nul.md"), vec!["_nul.md"]);
        assert_eq!(
            s(&"x".repeat(300)).concat().chars().count(),
            MAX_STEM_CHARS + 3
        );
        assert!(matches!(
            sanitize_relative_md("../x"),
            Err(LumaError::NoPermission { .. })
        ));
        assert!(matches!(
            sanitize_relative_md("a/../../x"),
            Err(LumaError::NoPermission { .. })
        ));
        for bad in ["", "/", ".", "run.exe", "notes.txt", "a/b.ps1"] {
            assert!(
                matches!(
                    sanitize_relative_md(bad),
                    Err(LumaError::InvalidInput { .. })
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn absolute_detection_covers_windows_spellings() {
        for a in ["/etc/x", "\\x", "C:\\x", "c:/x", "\\\\srv\\share\\x", "D:x"] {
            assert!(looks_absolute(a), "{a}");
        }
        for r in ["x", "sub/x", ".\\x", "notes.md"] {
            assert!(!looks_absolute(r), "{r}");
        }
    }

    #[test]
    fn never_overwrites_and_picks_numbered_names() {
        let d = tempfile::tempdir().unwrap();
        let a = create_unique_file(d.path(), "n.md", b"one").unwrap();
        let b = create_unique_file(d.path(), "n.md", b"two").unwrap();
        let c = create_unique_file(d.path(), "n.md", b"three").unwrap();
        assert_eq!(a.file_name().unwrap(), "n.md");
        assert_eq!(b.file_name().unwrap(), "n (2).md");
        assert_eq!(c.file_name().unwrap(), "n (3).md");
        assert_eq!(std::fs::read(&a).unwrap(), b"one");
        assert_eq!(std::fs::read(&b).unwrap(), b"two");
        // a directory squatting on the name is skipped too
        std::fs::create_dir(d.path().join("m.md")).unwrap();
        let m = create_unique_file(d.path(), "m.md", b"x").unwrap();
        assert_eq!(m.file_name().unwrap(), "m (2).md");
        assert_eq!(numbered_name("x", 2), "x (2)");
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_not_followed() {
        let d = tempfile::tempdir().unwrap();
        let outside = d.path().join("outside.md");
        std::os::unix::fs::symlink(&outside, d.path().join("n.md")).unwrap();
        let p = create_unique_file(d.path(), "n.md", b"x").unwrap();
        assert_eq!(p.file_name().unwrap(), "n (2).md");
        assert!(!outside.exists(), "must not create the symlink target");
    }

    #[test]
    fn text_decoding() {
        assert_eq!(
            decode_text("héllo 世界".as_bytes(), false).unwrap().0,
            "héllo 世界"
        );
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(b"hi");
        assert_eq!(
            decode_text(&bom, false).unwrap(),
            ("hi".to_owned(), "utf-8")
        );
        let le: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("a世".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(
            decode_text(&le, false).unwrap(),
            ("a世".to_owned(), "utf-16le")
        );
        let be: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain("a世".encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        assert_eq!(
            decode_text(&be, false).unwrap(),
            ("a世".to_owned(), "utf-16be")
        );
        // a multi-byte character cut by a size cap is dropped only when `cut`
        let cut = &"世界".as_bytes()[..4];
        assert_eq!(decode_text(cut, true).unwrap().0, "世");
        assert!(decode_text(cut, false).is_err());
        // genuinely invalid or binary data is rejected
        assert!(decode_text(&[0xFF, 0x41, 0x42], false).is_err());
        assert!(decode_text(b"ab\0cd", false).is_err());
        assert_eq!(decode_text(b"", false).unwrap().0, "");
        assert_eq!(floor_char_boundary("世界", 4), 3);
        assert_eq!(floor_char_boundary("ab", 10), 2);
    }

    #[cfg(windows)]
    #[test]
    fn windows_rejects_what_the_sanitiser_removes() {
        // Creating these names directly fails or misbehaves on Windows; the sanitised
        // forms must be creatable.
        let d = tempfile::tempdir().unwrap();
        for raw in ["CON", "aux.md", "a<b>.md", "trailing. ", "COM1.txt"] {
            let comps = sanitize_relative_md(raw)
                .or_else(|_| sanitize_relative_md(&raw.replace(".txt", "")))
                .unwrap();
            let p = create_unique_file(d.path(), comps.last().unwrap(), b"x").unwrap();
            assert!(p.exists(), "{raw} -> {}", p.display());
        }
        // case-insensitive file system: an existing README.md is not overwritten by readme.md
        let a = create_unique_file(d.path(), "README.md", b"1").unwrap();
        let b = create_unique_file(d.path(), "readme.md", b"2").unwrap();
        assert_ne!(
            a.to_string_lossy().to_lowercase(),
            b.to_string_lossy().to_lowercase()
        );
        assert_eq!(std::fs::read(&a).unwrap(), b"1");
    }
}
