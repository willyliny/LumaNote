//! Path-scope checking: absolute paths only, no `..`, symlinks resolved.
//!
//! Windows specifics (the product ships on Windows only): `fs::canonicalize` returns
//! `\\?\` verbatim paths there, which must still compare equal to the same location
//! written the ordinary way, and file systems are case-insensitive. Both are handled by
//! [`path_within`] / [`strip_verbatim_prefix`]; the pure string logic is platform
//! independent (so Linux CI tests it), only the choice of case rule uses `cfg!(windows)`.

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::schema::{ElfError, Result};

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
    let stripped = strip_verbatim_prefix(path);
    let text = stripped.to_string_lossy().into_owned();
    let is_sep = |c: char| c == '/' || (windows && c == '\\');
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
pub fn resolve_target(path: &Path) -> std::result::Result<PathBuf, String> {
    if !path.is_absolute() {
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
        if fs::symlink_metadata(&existing).is_ok() {
            break fs::canonicalize(&existing)
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
    let Ok(scope_real) = fs::canonicalize(scope) else {
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
    let denied = |why: String| ElfError::NoPermission {
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
            Err(ElfError::NoPermission { .. })
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
}
