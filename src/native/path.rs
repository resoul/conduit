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
