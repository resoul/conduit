use std::path::{Component, Path, PathBuf};

use crate::vm::value::Obj;

pub const PATH_JOIN: u8 = 0;
pub const PATH_JOIN_ALL: u8 = 1;
pub const PATH_DIRNAME: u8 = 2;
pub const PATH_BASENAME: u8 = 3;
pub const PATH_EXT: u8 = 4;
pub const PATH_STEM: u8 = 5;
pub const PATH_NORMALIZE: u8 = 6;
pub const PATH_IS_ABS: u8 = 7;
pub const PATH_WITH_EXT: u8 = 8;
pub const PATH_SAFE_JOIN: u8 = 9;

pub fn path_join(a: &str, b: &str) -> Obj {
    #[cfg(unix)]
    if !a.is_empty() && !b.is_empty() && !b.starts_with('/') {
        let need_sep = !a.ends_with('/');
        let mut out = String::with_capacity(a.len() + usize::from(need_sep) + b.len());
        out.push_str(a);
        if need_sep {
            out.push('/');
        }
        out.push_str(b);
        return Obj::str(out);
    }
    #[cfg(windows)]
    if !a.is_empty()
        && !b.is_empty()
        && !b.starts_with(['/', '\\'])
        && !b.contains(':')
        && !a.starts_with("\\\\")
    {
        let need_sep = !a.ends_with(['/', '\\']);
        let mut out = String::with_capacity(a.len() + usize::from(need_sep) + b.len());
        out.push_str(a);
        if need_sep {
            out.push('\\');
        }
        out.push_str(b);
        return Obj::str(out);
    }
    let joined = Path::new(a).join(b);
    Obj::str(joined.to_string_lossy().into_owned())
}

pub fn path_join_all(parts: &[Obj]) -> Obj {
    let mut pb = PathBuf::new();
    for item in parts {
        if let Obj::Str(s) = item {
            pb.push(s.as_str());
        }
    }
    Obj::str(pb.to_string_lossy().into_owned())
}

pub fn path_dirname(p: &str) -> Obj {
    let s = Path::new(p)
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    Obj::str(s)
}

pub fn path_basename(p: &str) -> Obj {
    let s = Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Obj::str(s)
}

pub fn path_ext(p: &str) -> Obj {
    let s = Path::new(p)
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    Obj::str(s)
}

pub fn path_stem(p: &str) -> Obj {
    let s = Path::new(p)
        .file_stem()
        .map(|st| st.to_string_lossy().into_owned())
        .unwrap_or_default();
    Obj::str(s)
}

pub fn path_normalize(p: &str) -> Obj {
    #[cfg(unix)]
    if !p.contains('\\') {
        let is_abs = p.starts_with('/');
        let mut segs: Vec<&str> = Vec::with_capacity(8);
        for part in p.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    if let Some(&last) = segs.last() {
                        if last != ".." {
                            segs.pop();
                        } else if !is_abs {
                            segs.push("..");
                        }
                    } else if !is_abs {
                        segs.push("..");
                    }
                }
                other => segs.push(other),
            }
        }
        if segs.is_empty() {
            return Obj::str(if is_abs { "/" } else { "." });
        }
        let total_len: usize =
            usize::from(is_abs) + segs.iter().map(|s| s.len()).sum::<usize>() + segs.len() - 1;
        let mut out = String::with_capacity(total_len);
        if is_abs {
            out.push('/');
        }
        for (i, seg) in segs.iter().enumerate() {
            if i > 0 {
                out.push('/');
            }
            out.push_str(seg);
        }
        return Obj::str(out);
    }

    #[cfg(windows)]
    if !p.contains(':') && !p.starts_with("\\\\") {
        let is_abs = p.starts_with(['/', '\\']);
        let mut segs: Vec<&str> = Vec::with_capacity(8);
        for part in p.split(['/', '\\']) {
            match part {
                "" | "." => {}
                ".." => {
                    if let Some(&last) = segs.last() {
                        if last != ".." {
                            segs.pop();
                        } else if !is_abs {
                            segs.push("..");
                        }
                    } else if !is_abs {
                        segs.push("..");
                    }
                }
                other => segs.push(other),
            }
        }
        if segs.is_empty() {
            return Obj::str(if is_abs { "\\" } else { "." });
        }
        let total_len: usize =
            usize::from(is_abs) + segs.iter().map(|s| s.len()).sum::<usize>() + segs.len() - 1;
        let mut out = String::with_capacity(total_len);
        if is_abs {
            out.push('\\');
        }
        for (i, seg) in segs.iter().enumerate() {
            if i > 0 {
                out.push('\\');
            }
            out.push_str(seg);
        }
        return Obj::str(out);
    }

    let mut stack: Vec<Component<'_>> = Vec::new();
    for comp in Path::new(p).components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match stack.last() {
                Some(Component::Normal(_)) => {
                    stack.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => stack.push(comp),
            },
            other => stack.push(other),
        }
    }
    if stack.is_empty() {
        return Obj::str(".");
    }
    let out: PathBuf = stack.iter().collect();
    Obj::str(out.to_string_lossy().into_owned())
}

pub fn path_is_abs(p: &str) -> bool {
    Path::new(p).is_absolute()
}

pub fn path_with_ext(p: &str, ext: &str) -> Obj {
    let clean = ext.strip_prefix('.').unwrap_or(ext);
    let out = Path::new(p).with_extension(clean);
    Obj::str(out.to_string_lossy().into_owned())
}

/// Safely joins `rel` under `root`, resolving symlinks step-by-step against the
/// canonicalized `root` and returning `Obj::Empty` (`nil`) on any traversal,
/// absolute/prefixed path, NUL byte, dangling symlink, or symlink escape.
/// Not-yet-created leaf/subdirectory components inside an existing `root` are allowed.
pub fn path_safe_join(root: &str, rel: &str) -> Obj {
    if root.is_empty()
        || root.contains('\0')
        || rel.contains('\0')
        || rel.starts_with(['/', '\\'])
    {
        return Obj::Empty;
    }
    let Ok(canon_root) = std::fs::canonicalize(root) else {
        return Obj::Empty;
    };

    let mut cur = canon_root.clone();
    let mut missing_depth: usize = 0;

    for comp in Path::new(rel).components() {
        match comp {
            Component::RootDir | Component::Prefix(_) => return Obj::Empty,
            Component::CurDir => {}
            Component::ParentDir => {
                if missing_depth > 0 {
                    cur.pop();
                    missing_depth -= 1;
                } else if cur == canon_root || !cur.pop() || !cur.starts_with(&canon_root) {
                    return Obj::Empty;
                }
            }
            Component::Normal(seg) => {
                if seg.is_empty() {
                    return Obj::Empty;
                }
                cur.push(seg);
                if missing_depth > 0 {
                    missing_depth += 1;
                } else {
                    match std::fs::canonicalize(&cur) {
                        Ok(resolved) => {
                            if !resolved.starts_with(&canon_root) {
                                return Obj::Empty;
                            }
                            cur = resolved;
                        }
                        Err(_) => match std::fs::symlink_metadata(&cur) {
                            // Entry exists on disk (e.g., dangling symlink or inaccessible dir)
                            // but failed canonicalize — never skip symlink resolution for it.
                            Ok(_) => return Obj::Empty,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                missing_depth = 1;
                            }
                            Err(_) => return Obj::Empty,
                        },
                    }
                }
            }
        }
    }

    if !cur.starts_with(&canon_root) {
        return Obj::Empty;
    }
    Obj::str(cur.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQ: AtomicU64 = AtomicU64::new(0);

    struct TempDirGuard(PathBuf);

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn make_temp_root(tag: &str) -> TempDirGuard {
        let id = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "conduit_path_safe_join_{}_{}_{}",
            tag,
            std::process::id(),
            id
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDirGuard(dir)
    }

    #[test]
    fn safe_join_allows_containment_and_blocks_traversal_and_prefix_tricks() {
        let base = make_temp_root("basic");
        let root = base.0.join("dir");
        let sibling = base.0.join("dir-evil");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(root.join("sub/hello.txt"), "ok").unwrap();
        std::fs::write(sibling.join("secret.txt"), "nope").unwrap();

        let root_str = root.to_str().unwrap();
        let canon_root = std::fs::canonicalize(&root).unwrap();

        assert_eq!(
            path_safe_join(root_str, "sub/hello.txt"),
            Obj::str(canon_root.join("sub/hello.txt").to_string_lossy().into_owned())
        );

        let missing_leaf = path_safe_join(root_str, "sub/new_dir/file.txt");
        assert_eq!(
            missing_leaf,
            Obj::str(canon_root.join("sub/new_dir/file.txt").to_string_lossy().into_owned())
        );

        assert_eq!(
            path_safe_join(root_str, "sub/./new/../ok.txt"),
            Obj::str(canon_root.join("sub/ok.txt").to_string_lossy().into_owned())
        );

        assert_eq!(path_safe_join(root_str, "../outside"), Obj::Empty);
        assert_eq!(path_safe_join(root_str, "sub/../../outside"), Obj::Empty);
        assert_eq!(path_safe_join(root_str, "../dir-evil/secret.txt"), Obj::Empty);
        assert_eq!(path_safe_join(root_str, "/etc/passwd"), Obj::Empty);
        assert_eq!(path_safe_join(root_str, "\\windows\\system32"), Obj::Empty);
        assert_eq!(path_safe_join(root_str, "sub\0/evil"), Obj::Empty);
        assert_eq!(path_safe_join("", "hello.txt"), Obj::Empty);
        assert_eq!(
            path_safe_join(base.0.join("does-not-exist").to_str().unwrap(), "hello.txt"),
            Obj::Empty
        );
    }

    #[cfg(unix)]
    #[test]
    fn safe_join_enforces_unix_symlink_containment() {
        use std::os::unix::fs::symlink;

        let base = make_temp_root("sym");
        let root = base.0.join("dir");
        let sibling = base.0.join("dir-evil");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(root.join("real.txt"), "inside").unwrap();
        std::fs::write(sibling.join("secret.txt"), "outside").unwrap();

        // Symlink inside root pointing outside root (including sibling prefix collision)
        symlink(&sibling, root.join("link_out")).unwrap();
        // Symlink inside root pointing to another file inside root
        symlink(root.join("real.txt"), root.join("nested/link_in.txt")).unwrap();
        // Dangling symlink pointing outside root
        symlink(sibling.join("future_secret.txt"), root.join("dangling_out")).unwrap();
        // Symlink to nested dir inside root, used to test "link_nested/../../outside"
        symlink(root.join("nested"), root.join("link_nested")).unwrap();

        let root_str = root.to_str().unwrap();
        let canon_root = std::fs::canonicalize(&root).unwrap();

        assert_eq!(path_safe_join(root_str, "link_out"), Obj::Empty);
        assert_eq!(path_safe_join(root_str, "link_out/secret.txt"), Obj::Empty);
        assert_eq!(path_safe_join(root_str, "dangling_out"), Obj::Empty);
        assert_eq!(path_safe_join(root_str, "nonexistent/../link_out/secret.txt"), Obj::Empty);
        assert_eq!(path_safe_join(root_str, "nonexistent/../dangling_out"), Obj::Empty);
        assert_eq!(path_safe_join(root_str, "link_nested/../../dir-evil/secret.txt"), Obj::Empty);

        assert_eq!(
            path_safe_join(root_str, "nested/link_in.txt"),
            Obj::str(canon_root.join("real.txt").to_string_lossy().into_owned())
        );
        assert_eq!(
            path_safe_join(root_str, "link_nested/new_file.txt"),
            Obj::str(canon_root.join("nested/new_file.txt").to_string_lossy().into_owned())
        );
    }
}
