//! Path-scope checking: absolute paths only, no `..`, symlinks resolved.

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::schema::{ElfError, Result};

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
    if real.starts_with(&scope_real) {
        PathMatch::Inside
    } else if raw.starts_with(scope) || raw.starts_with(&scope_real) {
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
