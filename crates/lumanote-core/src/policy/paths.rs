//! Path-scope checking: lexical validation first, file system second.
//!
//! # Order of work (handbook 8.1)
//!
//! 1. [`lexical_check`]: pure string validation. UNC paths, device paths, verbatim paths
//!    other than `\\?\X:\...`, drive-relative paths (`C:foo`), alternate data streams
//!    (`file:stream`), NUL, reserved device names and traversal are rejected **before the
//!    file system is touched**. It takes an explicit `windows` flag so the Windows rules are
//!    tested on every platform.
//! 2. Grant matching is done on the lexically normalised path ([`path_within_with`]).
//! 3. Only a path that lies lexically under a granted root is resolved
//!    ([`resolve_target_with`]: canonicalize + symlink/junction escape check).
//!
//! All file-system access of the policy goes through [`FsPort`], so tests can prove that a
//! path no grant covers is never looked at.
//!
//! Windows specifics (the product ships on Windows only): `fs::canonicalize` returns
//! `\\?\` verbatim paths there, which must still compare equal to the same location
//! written the ordinary way, and file systems are case-insensitive. Both are handled by
//! [`path_within`] / [`strip_verbatim_prefix`]; the pure string logic is platform
//! independent (so Linux CI tests it), only the choice of case rule uses `cfg!(windows)`.

use std::ffi::OsString;
use std::fmt::Debug;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::schema::{LumaError, Result};

/// The file-system operations policy decisions may use. The production implementation is
/// [`OsFs`]; tests inject a spy to prove that nothing is touched before authorisation.
pub trait FsPort: Send + Sync + Debug {
    /// Whether Windows path rules apply (`cfg!(windows)` in production).
    fn windows_rules(&self) -> bool;
    /// `std::fs::canonicalize`.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;
    /// Whether `std::fs::symlink_metadata(path)` succeeds.
    fn exists_no_follow(&self, path: &Path) -> bool;
}

/// The real file system with the host's path rules.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsFs;

impl FsPort for OsFs {
    fn windows_rules(&self) -> bool {
        cfg!(windows)
    }
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(path)
    }
    fn exists_no_follow(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok()
    }
}

/// Characters Windows does not allow in a file name (`:` is handled separately because it
/// opens an alternate data stream).
const WIN_FORBIDDEN: &[char] = &['<', '>', '"', '|', '?', '*'];

fn win_sep(c: char) -> bool {
    c == '\\' || c == '/'
}

/// Pure, file-system-free validation of a path given as the model (or anyone) wrote it.
/// Returns the lexically normalised absolute path (`.` and empty parts removed, one kind of
/// separator, drive letter upper-cased on Windows) or the reason it is refused.
///
/// With `windows` set this refuses: UNC (`\\server\share`, `//server/share`), device and
/// verbatim forms other than `\\?\X:\...` (`\\.\`, `\\?\GLOBALROOT`, `\\?\UNC\`,
/// `\\?\Volume{...}`, `\??\`), paths without a drive (`\x`, `x\y`), drive-relative paths
/// (`C:foo`), alternate data streams (a `:` anywhere after the drive specifier), NUL and
/// control characters, characters Windows forbids, reserved device names (`CON`, `NUL.txt`,
/// `COM1 `, ...) as any component, and `..` (also spelled `...` or `.. `).
/// Without it: absolute POSIX paths without NUL and `..`.
pub fn lexical_check(path: &Path, windows: bool) -> std::result::Result<PathBuf, String> {
    let shown = path.display().to_string();
    let Some(text) = path.to_str() else {
        return Err(format!("path is not valid Unicode: {shown}"));
    };
    if text.contains('\0') {
        return Err(format!("path contains a NUL character: {shown:?}"));
    }
    if windows {
        lexical_windows(text).map_err(|why| format!("{why}: {shown}"))
    } else {
        lexical_posix(text).map_err(|why| format!("{why}: {shown}"))
    }
}

fn lexical_posix(text: &str) -> std::result::Result<PathBuf, &'static str> {
    if !text.starts_with('/') {
        return Err("path is not absolute");
    }
    let mut out = String::new();
    for part in text.split('/').filter(|p| !p.is_empty() && *p != ".") {
        if part == ".." {
            return Err("path traversal (..)");
        }
        out.push('/');
        out.push_str(part);
    }
    if out.is_empty() {
        out.push('/');
    }
    Ok(PathBuf::from(out))
}

fn lexical_windows(text: &str) -> std::result::Result<PathBuf, &'static str> {
    // `\\?\X:\...` is the form `canonicalize` returns; it is the only verbatim form
    // accepted, and inside it `/` is a literal character, not a separator.
    let body = if let Some(rest) = text.strip_prefix(r"\\?\") {
        if rest.contains('/') {
            return Err("'/' inside a verbatim path");
        }
        let b = rest.as_bytes();
        if !(b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\') {
            return Err("verbatim, device or UNC path form is not allowed");
        }
        rest
    } else {
        let mut chars = text.chars();
        if chars.next().is_some_and(win_sep) && chars.next().is_some_and(win_sep) {
            return Err("UNC or device path is not allowed");
        }
        text
    };
    let b = body.as_bytes();
    if b.len() < 2 || !b[0].is_ascii_alphabetic() || b[1] != b':' {
        return Err("path must start with a drive letter");
    }
    if b.len() < 3 || !win_sep(char::from(b[2])) {
        return Err("drive-relative path is not allowed");
    }
    let drive = char::from(b[0]).to_ascii_uppercase();
    let mut out = format!("{drive}:");
    for part in body[3..]
        .split(win_sep)
        .filter(|p| !p.is_empty() && *p != ".")
    {
        if part.contains(':') {
            return Err("alternate data stream or ':' in a path component");
        }
        if part
            .chars()
            .any(|c| WIN_FORBIDDEN.contains(&c) || c.is_control())
        {
            return Err("character that Windows does not allow in a name");
        }
        let trimmed = part.trim_end_matches(['.', ' ']);
        if part == ".." || trimmed.is_empty() {
            return Err("path traversal (..) or a name made of dots and spaces");
        }
        if crate::tools::fsutil::is_reserved_windows_name(part) {
            return Err("reserved device name");
        }
        out.push('\\');
        out.push_str(part);
    }
    if out.len() == 2 {
        out.push('\\');
    }
    Ok(PathBuf::from(out))
}

/// Removes a Windows verbatim prefix: `\\?\C:\x` becomes `C:\x` and `\\?\UNC\srv\share\x`
/// becomes `\\srv\share\x`. Other paths are returned unchanged. Pure string logic.
pub fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

/// Like [`strip_verbatim_prefix`] but keeps the verbatim form for very long paths, where
/// Windows needs it to open the file at all.
pub fn display_path(path: &Path) -> PathBuf {
    let stripped = strip_verbatim_prefix(path);
    if stripped.as_os_str().len() >= 240 {
        path.to_path_buf()
    } else {
        stripped
    }
}

/// Splits a path into its components, keeping the original spelling. With `windows` set,
/// both `/` and `\` separate components and the verbatim prefix is ignored. A leading
/// separator yields an empty first component (the root).
fn path_parts(path: &Path, windows: bool) -> Vec<String> {
    // In a verbatim path (`\\?\...`) `/` is an ordinary character of a name, not a separator.
    let verbatim = windows && path.to_string_lossy().starts_with(r"\\?\");
    let stripped = strip_verbatim_prefix(path);
    let text = stripped.to_string_lossy().into_owned();
    let is_sep = |c: char| (c == '/' && !verbatim) || (windows && c == '\\');
    let mut parts: Vec<String> = Vec::new();
    if text.starts_with(is_sep) {
        parts.push(String::new());
    }
    parts.extend(
        text.split(is_sep)
            .filter(|p| !p.is_empty() && *p != ".")
            .map(str::to_owned),
    );
    parts
}

/// Comparison keys of [`path_parts`]: lower-cased on Windows.
fn path_keys(path: &Path, windows: bool) -> Vec<String> {
    path_parts(path, windows)
        .into_iter()
        .map(|p| if windows { p.to_lowercase() } else { p })
        .collect()
}

/// The components of `path` below `prefix` in their original spelling, or `None` if
/// `path` is not inside `prefix` (see [`path_within`] for the comparison rules).
pub fn relative_below(path: &Path, prefix: &Path) -> Option<Vec<String>> {
    relative_below_with(path, prefix, cfg!(windows))
}

/// [`relative_below`] with explicit platform rules.
pub fn relative_below_with(path: &Path, prefix: &Path, windows: bool) -> Option<Vec<String>> {
    if !path_within_with(path, prefix, windows) {
        return None;
    }
    let skip = path_parts(prefix, windows).len();
    Some(path_parts(path, windows).into_iter().skip(skip).collect())
}

/// Component-wise prefix test with explicit platform rules (see [`path_within`]).
pub fn path_within_with(path: &Path, prefix: &Path, windows: bool) -> bool {
    let (p, q) = (path_keys(path, windows), path_keys(prefix, windows));
    p.len() >= q.len() && p.iter().zip(q.iter()).all(|(a, b)| a == b)
}

/// True if `path` is `prefix` or lies below it. On Windows the comparison ignores case,
/// accepts both separators and treats `\\?\C:\x` the same as `C:\x`; elsewhere it behaves
/// like [`Path::starts_with`].
pub fn path_within(path: &Path, prefix: &Path) -> bool {
    path_within_with(path, prefix, cfg!(windows))
}

/// How a target relates to a granted directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMatch {
    /// The real target lies inside the scope.
    Inside,
    /// The target is unrelated to the scope.
    Outside,
    /// The path looks inside the scope but resolves outside it (symlink escape).
    Escape,
}

/// Resolves `path` to its real location. The path must be absolute and free of `..`.
/// The deepest existing ancestor is canonicalised (following symlinks); components
/// that do not exist yet are appended as-is. A dangling symlink is an error.
///
/// This touches the file system: call it only for a path that already passed
/// [`lexical_check`] and lies under a granted root.
pub fn resolve_target(path: &Path) -> std::result::Result<PathBuf, String> {
    resolve_target_with(&OsFs, path)
}

/// [`resolve_target`] through an explicit [`FsPort`].
pub fn resolve_target_with(fs: &dyn FsPort, path: &Path) -> std::result::Result<PathBuf, String> {
    if !path.is_absolute() && lexical_check(path, fs.windows_rules()).is_err() {
        return Err(format!("path is not absolute: {}", path.display()));
    }
    let mut normalized = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                return Err(format!("path traversal (..) in {}", path.display()))
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    let mut existing = normalized;
    let mut rest: Vec<OsString> = Vec::new();
    let base = loop {
        if fs.exists_no_follow(&existing) {
            break fs
                .canonicalize(&existing)
                .map_err(|e| format!("cannot resolve {}: {e}", existing.display()))?;
        }
        match existing.file_name().map(OsString::from) {
            Some(name) if existing.pop() => rest.push(name),
            _ => return Err(format!("no existing ancestor for {}", path.display())),
        }
    };
    Ok(rest.into_iter().rev().fold(base, |mut acc, n| {
        acc.push(n);
        acc
    }))
}

/// Compares a target against a granted directory.
///
/// `raw` is the path as requested (no `..`), `real` its resolved form.
pub fn classify(scope: &Path, raw: &Path, real: &Path) -> PathMatch {
    classify_with(&OsFs, scope, raw, real)
}

/// [`classify`] through an explicit [`FsPort`]. The scope is resolved like a target, so a
/// scope that names a file or directory that does not exist yet (a one-shot approval for a
/// new file) still has a real location.
pub fn classify_with(fs: &dyn FsPort, scope: &Path, raw: &Path, real: &Path) -> PathMatch {
    let Ok(scope_real) = resolve_target_with(fs, scope) else {
        return PathMatch::Outside;
    };
    if path_within(real, &scope_real) {
        PathMatch::Inside
    } else if path_within(raw, scope) || path_within(raw, &scope_real) {
        PathMatch::Escape
    } else {
        PathMatch::Outside
    }
}

/// Resolves `target` and returns its real path only if it stays inside `scope`.
/// For use by tools that need the checked path to operate on.
pub fn resolve_within(scope: &Path, target: &Path) -> Result<PathBuf> {
    let denied = |why: String| LumaError::NoPermission {
        action: "path_access".into(),
        resource: format!("{} ({why})", target.display()),
    };
    let real = resolve_target(target).map_err(denied)?;
    match classify(scope, target, &real) {
        PathMatch::Inside => Ok(real),
        PathMatch::Outside => Err(denied("outside granted directory".into())),
        PathMatch::Escape => Err(denied("symlink escapes granted directory".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_prefixes_are_stripped() {
        let strip = |s: &str| {
            strip_verbatim_prefix(Path::new(s))
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(strip(r"\\?\C:\Users\me\a.md"), r"C:\Users\me\a.md");
        assert_eq!(strip(r"\\?\UNC\srv\share\a.md"), r"\\srv\share\a.md");
        assert_eq!(strip(r"\\?\Volume{1234}\a"), r"\\?\Volume{1234}\a"); // not a drive path
        assert_eq!(strip(r"C:\plain"), r"C:\plain");
        assert_eq!(strip("/unix/path"), "/unix/path");
        // very long paths keep the verbatim form for display
        let long = format!(r"\\?\C:\{}", "d\\".repeat(130));
        assert_eq!(display_path(Path::new(&long)), PathBuf::from(&long));
        assert_eq!(display_path(Path::new(r"\\?\C:\a")), PathBuf::from(r"C:\a"));
    }

    #[test]
    fn windows_prefix_comparison_ignores_case_separators_and_verbatim() {
        let w = |p: &str, q: &str| path_within_with(Path::new(p), Path::new(q), true);
        assert!(w(r"C:\Users\Me\Out\a.md", r"c:\users\me\out"));
        assert!(w(r"\\?\C:\Users\Me\Out\a.md", r"C:\Users\Me\Out"));
        assert!(w(r"C:\Users\Me\Out\a.md", r"\\?\c:\users\me\out"));
        assert!(w(r"C:/Users/Me/Out/sub/a.md", r"C:\Users\Me\Out"));
        assert!(w(r"C:\Users\Me\Out", r"C:\Users\Me\Out"));
        assert!(w(r"C:\Users\Me\.\Out\a", r"C:\Users\Me\Out"));
        // look-alikes and other drives are outside
        assert!(!w(r"C:\Users\Me\Out-evil\a.md", r"C:\Users\Me\Out"));
        assert!(!w(r"D:\Users\Me\Out\a.md", r"C:\Users\Me\Out"));
        assert!(!w(r"C:\Users\Me", r"C:\Users\Me\Out"));
        assert!(!w(r"\\srv\share\x", r"C:\x"));
        // the same case difference is significant when case-sensitive rules apply
        assert!(!path_within_with(Path::new("/A/b"), Path::new("/a"), false));
        assert!(path_within_with(Path::new("/a/b"), Path::new("/a"), false));
        assert!(!path_within_with(Path::new("/ab"), Path::new("/a"), false));
    }

    #[test]
    fn relative_remainder_keeps_original_case() {
        let r = |p: &str, q: &str| relative_below_with(Path::new(p), Path::new(q), true);
        assert_eq!(
            r(r"C:\Out\Sub\Note.md", r"c:\out"),
            Some(vec!["Sub".to_owned(), "Note.md".to_owned()])
        );
        assert_eq!(r(r"C:\Out", r"C:\Out"), Some(vec![]));
        assert_eq!(r(r"C:\Other\x", r"C:\Out"), None);
        assert_eq!(
            relative_below_with(Path::new("/o/a/B"), Path::new("/o"), false),
            Some(vec!["a".to_owned(), "B".to_owned()])
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_canonical_paths_compare_with_user_spelling() {
        let d = tempfile::tempdir().unwrap();
        let canon = fs::canonicalize(d.path()).unwrap();
        // canonicalize yields a \\?\ path on Windows; the user's spelling must still match
        assert!(canon.to_string_lossy().starts_with(r"\\?\"));
        let plain = strip_verbatim_prefix(&canon);
        let upper = PathBuf::from(plain.to_string_lossy().to_uppercase());
        assert!(path_within(&canon.join("x.md"), &plain));
        assert!(path_within(&canon.join("x.md"), &upper));
        assert!(path_within(&plain.join("x.md"), &canon));
        assert_eq!(
            classify(
                &upper,
                &plain.join("x.md"),
                &resolve_target(&canon.join("x.md")).unwrap()
            ),
            PathMatch::Inside
        );
    }

    #[test]
    fn inside_nonexistent_and_prefix_lookalike() {
        let d = tempfile::tempdir().unwrap();
        let scope = fs::canonicalize(d.path()).unwrap();
        fs::create_dir(scope.join("docs")).unwrap();
        fs::write(scope.join("docs/a.txt"), "x").unwrap();
        assert!(resolve_within(&scope, &scope.join("docs/a.txt")).is_ok());
        assert!(resolve_within(&scope, &scope.join("docs/new/deep/file.md")).is_ok());
        assert!(resolve_within(&scope, &scope.join("./docs/./a.txt")).is_ok());
        // sibling whose name starts with the scope name must not match
        let sibling = PathBuf::from(format!("{}-evil", scope.display()));
        assert!(resolve_within(&scope, &sibling.join("a.txt")).is_err());
    }

    #[test]
    fn traversal_and_relative_paths_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        let scope = fs::canonicalize(d.path()).unwrap();
        fs::create_dir(scope.join("docs")).unwrap();
        assert!(resolve_target(&scope.join("docs/../../etc/passwd")).is_err());
        assert!(resolve_target(&scope.join("docs/../docs/a.txt")).is_err());
        assert!(resolve_target(&scope.join("nope/../x")).is_err());
        assert!(resolve_target(Path::new("relative/file")).is_err());
        assert!(matches!(
            resolve_within(&scope, &scope.join("docs/../../x")),
            Err(LumaError::NoPermission { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_detected() {
        use std::os::unix::fs::symlink;
        let d = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(d.path()).unwrap();
        let scope = root.join("granted");
        let outside = root.join("secret");
        fs::create_dir(&scope).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("key.txt"), "s").unwrap();
        symlink(&outside, scope.join("link")).unwrap();
        symlink(outside.join("key.txt"), scope.join("filelink")).unwrap();
        symlink(root.join("missing-target"), scope.join("dangling")).unwrap();
        // existing file through directory symlink
        let t = scope.join("link/key.txt");
        assert_eq!(
            classify(&scope, &t, &resolve_target(&t).unwrap()),
            PathMatch::Escape
        );
        // not-yet-existing file below an escaping directory symlink
        let t = scope.join("link/new.txt");
        assert_eq!(
            classify(&scope, &t, &resolve_target(&t).unwrap()),
            PathMatch::Escape
        );
        // symlink to a file outside
        let t = scope.join("filelink");
        assert_eq!(
            classify(&scope, &t, &resolve_target(&t).unwrap()),
            PathMatch::Escape
        );
        // dangling symlink cannot be resolved -> rejected
        assert!(resolve_target(&scope.join("dangling")).is_err());
        assert!(resolve_within(&scope, &scope.join("link/key.txt")).is_err());
        // a link that stays inside is fine
        fs::create_dir(scope.join("real")).unwrap();
        symlink(scope.join("real"), scope.join("alias")).unwrap();
        assert!(resolve_within(&scope, &scope.join("alias/f.txt")).is_ok());
    }

    // ----- the pure lexical classifier (H3) -----------------------------------------------

    #[test]
    fn lexical_windows_rejects_every_network_device_and_stream_form() {
        let bad = [
            r"\\server\share\x",
            "//server/share/x",
            r"\\.\pipe\x",
            r"\\.\C:\x",
            r"\\?\GLOBALROOT\Device\x",
            r"\\?\UNC\server\share\x",
            r"\\?\Volume{1}\x",
            r"\\?\C:/x",
            "C:foo",
            "C:",
            r"\x\y",
            "x.txt",
            r"C:\a\b.txt:s",
            r"C:\a\b.txt::$DATA",
            "C:\\a\\b\0",
            r"C:\a\..\b",
            r"C:\a\NUL",
            r"C:\a\nul.txt",
            r"C:\a\Com9.log",
            r"C:\a\LPT1",
            r"C:\a\CONIN$",
            r"C:\a\b*.txt",
            r"C:\a\ \x",
            r"C:\a\...\x",
        ];
        for p in bad {
            assert!(lexical_check(Path::new(p), true).is_err(), "{p:?}");
        }
    }

    #[test]
    fn lexical_windows_normalises_good_paths_without_a_file_system() {
        for (given, want) in [
            (r"c:\Users\u\docs", r"C:\Users\u\docs"),
            (r"C:/Users//u/./docs\", r"C:\Users\u\docs"),
            (r"\\?\C:\Users\u", r"C:\Users\u"),
            (r"C:\", r"C:\"),
        ] {
            assert_eq!(
                lexical_check(Path::new(given), true).unwrap(),
                PathBuf::from(want),
                "{given}"
            );
        }
    }

    #[test]
    fn lexical_posix_requires_an_absolute_path_without_traversal_or_nul() {
        assert_eq!(
            lexical_check(Path::new("/a//b/./c"), false).unwrap(),
            PathBuf::from("/a/b/c")
        );
        for bad in ["a/b", "./a", "/a/../b", "/a/\0b", ""] {
            assert!(lexical_check(Path::new(bad), false).is_err(), "{bad:?}");
        }
        // on POSIX a backslash is an ordinary name character and `C:` an ordinary name
        assert!(lexical_check(Path::new("/a/b\\c/d:e"), false).is_ok());
        // the same string means something else under Windows rules
        assert!(lexical_check(Path::new("/a/b"), true).is_err());
    }

    #[test]
    fn slash_is_a_literal_inside_a_verbatim_path() {
        // `\\?\C:\granted/..\secret` names ONE component "granted/.." below C:\ on
        // Windows; it must not be read as C:\granted\..\secret, and never as inside
        // C:\granted
        let scope = Path::new(r"C:\granted");
        assert!(!path_within_with(
            Path::new(r"\\?\C:\granted/..\secret"),
            scope,
            true
        ));
        assert!(!path_within_with(
            Path::new(r"\\?\C:\granted/sub"),
            scope,
            true
        ));
        assert!(path_within_with(
            Path::new(r"\\?\C:\granted\sub"),
            scope,
            true
        ));
        // outside the verbatim form `/` still separates
        assert!(path_within_with(Path::new("C:/granted/sub"), scope, true));
        assert_eq!(
            relative_below_with(Path::new(r"\\?\C:\granted\a/b"), scope, true),
            Some(vec!["a/b".to_owned()])
        );
    }
}
