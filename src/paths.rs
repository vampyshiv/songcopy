//! Path helpers that behave like Python's os.path, on Linux and Windows.
//! Paths are kept as Strings (shown on the page, stored in JSON).

use std::path::Path;

pub const AUDIO_EXT: &[&str] = &[
    ".flac", ".mp3", ".m4a", ".opus", ".ogg", ".wav", ".aiff", ".aif", ".wma", ".alac", ".ape",
    ".wv",
];

#[cfg(windows)]
pub const SEP: char = '\\';
#[cfg(not(windows))]
pub const SEP: char = '/';

pub const WINDOWS: bool = cfg!(windows);

/// tests that change environment variables (songcopy's folder) take turns
#[cfg(test)]
pub static TEST_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn is_sep(c: char) -> bool {
    c == '/' || (WINDOWS && c == '\\')
}

fn has_drive(p: &str) -> bool {
    let b = p.as_bytes();
    WINDOWS && b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic()
}

pub fn is_absolute(p: &str) -> bool {
    p.starts_with(is_sep) || has_drive(p)
}

/// os.path.join(a, b) for the cases songcopy needs
pub fn join(a: &str, b: &str) -> String {
    if is_absolute(b) || a.is_empty() {
        return b.to_string();
    }
    if a.ends_with(is_sep) {
        format!("{a}{b}")
    } else {
        format!("{a}{SEP}{b}")
    }
}

pub fn basename(p: &str) -> &str {
    match p.rfind(is_sep) {
        Some(i) => &p[i + 1..],
        None if has_drive(p) => &p[2..],
        None => p,
    }
}

pub fn dirname(p: &str) -> &str {
    match p.rfind(is_sep) {
        Some(i) => {
            let head = &p[..i + 1];
            // like Python: strip trailing separators unless it's the root
            let trimmed = head.trim_end_matches(is_sep);
            if trimmed.is_empty() || (has_drive(trimmed) && trimmed.len() == 2) {
                head
            } else {
                trimmed
            }
        }
        None if has_drive(p) => &p[..2],
        None => "",
    }
}

/// os.path.splitext: ("name", ".ext"); leading dots don't start an extension
pub fn splitext(p: &str) -> (&str, &str) {
    let start = p.rfind(is_sep).map(|i| i + 1).unwrap_or(0);
    let name = &p[start..];
    if let Some(dot) = name.rfind('.') {
        if name[..dot].chars().any(|c| c != '.') {
            return (&p[..start + dot], &p[start + dot..]);
        }
    }
    (p, "")
}

pub fn is_audio(name: &str) -> bool {
    let ext = splitext(name).1.to_lowercase();
    AUDIO_EXT.contains(&ext.as_str())
}

pub fn home() -> String {
    let v = if WINDOWS { "USERPROFILE" } else { "HOME" };
    std::env::var(v).unwrap_or_else(|_| ".".into())
}

pub fn expanduser(p: &str) -> String {
    if p == "~" {
        home()
    } else if let Some(rest) =
        p.strip_prefix("~/")
            .or_else(|| if WINDOWS { p.strip_prefix("~\\") } else { None })
    {
        join(&home(), rest)
    } else {
        p.to_string()
    }
}

/// os.path.abspath: absolute and tidied up ('.', '..', doubled separators)
pub fn abspath(p: &str) -> String {
    let p = if WINDOWS {
        p.replace('/', "\\")
    } else {
        p.to_string()
    };
    let p = if is_absolute(&p) {
        p
    } else {
        let cwd = std::env::current_dir()
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_default();
        join(&cwd, &p)
    };
    normpath(&p)
}

fn normpath(p: &str) -> String {
    let (prefix, rest) = if WINDOWS && p.starts_with("\\\\") {
        ("\\\\", &p[2..]) // \\server\share
    } else if has_drive(p) {
        (&p[..2], &p[2..])
    } else {
        ("", p)
    };
    let rooted = rest.starts_with(is_sep);
    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split(is_sep) {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            x => parts.push(x),
        }
    }
    let sep = SEP.to_string();
    let body = parts.join(&sep);
    if prefix == "\\\\" {
        format!("\\\\{body}")
    } else if rooted {
        format!("{prefix}{SEP}{body}")
    } else if body.is_empty() {
        if prefix.is_empty() {
            ".".into()
        } else {
            prefix.to_string()
        }
    } else {
        format!("{prefix}{body}")
    }
}

/// os.path.normcase
pub fn normcase(p: &str) -> String {
    if WINDOWS {
        p.to_lowercase().replace('/', "\\")
    } else {
        p.to_string()
    }
}

pub fn relpath(path: &str, root: &str) -> String {
    path.strip_prefix(root)
        .map(|r| r.trim_start_matches(is_sep).to_string())
        .unwrap_or_else(|| path.to_string())
}

pub fn isfile(p: &str) -> bool {
    Path::new(p).is_file()
}

pub fn isdir(p: &str) -> bool {
    Path::new(p).is_dir()
}

pub fn exists(p: &str) -> bool {
    Path::new(p).exists()
}

pub fn getsize(p: &str) -> std::io::Result<u64> {
    std::fs::metadata(p).map(|m| m.len())
}

/// the longest existing folder at or above p
pub fn nearest_existing(p: &str) -> String {
    let mut cur = p.to_string();
    while !cur.is_empty() && !exists(&cur) {
        let up = dirname(&cur).to_string();
        if up == cur {
            break;
        }
        cur = up;
    }
    if cur.is_empty() {
        if WINDOWS {
            "C:\\".into()
        } else {
            "/".into()
        }
    } else {
        cur
    }
}

/// For handing a path to another program (ffmpeg) on Windows: the \\?\ form works past 260 characters.
/// (songcopy's own file access handles long paths by itself.)
pub fn longp(p: &str) -> String {
    if !WINDOWS || p.starts_with("\\\\?\\") {
        return p.to_string();
    }
    let p = abspath(p);
    if let Some(unc) = p.strip_prefix("\\\\") {
        format!("\\\\?\\UNC\\{unc}")
    } else {
        format!("\\\\?\\{p}")
    }
}

/// songcopy's own folder for its memory: %LOCALAPPDATA%\songcopy or ~/.cache/songcopy
pub fn app_dir() -> String {
    let base = std::env::var("LOCALAPPDATA")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| join(&home(), ".cache"));
    join(&base, "songcopy")
}

pub fn write_atomic(path: &str, data: &[u8]) -> std::io::Result<()> {
    if let Some(d) = Path::new(path).parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// 25027 -> "25,027"
pub fn thousands(n: usize) -> String {
    let d = n.to_string();
    let mut out = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// bytes -> "12.3 MB"
pub fn human(n: u64) -> String {
    let mut x = n as f64;
    for u in ["B", "KB", "MB", "GB", "TB"] {
        if x < 1024.0 || u == "TB" {
            return if u == "B" {
                format!("{n} B")
            } else {
                format!("{x:.1} {u}")
            };
        }
        x /= 1024.0;
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_python() {
        assert_eq!(splitext("a/b.c/name.flac"), ("a/b.c/name", ".flac"));
        assert_eq!(splitext(".bashrc"), (".bashrc", ""));
        assert_eq!(splitext("x.tar.gz"), ("x.tar", ".gz"));
        assert_eq!(basename("a/b/c.mp3"), "c.mp3");
        if !WINDOWS {
            assert_eq!(dirname("a/b/c.mp3"), "a/b");
            assert_eq!(abspath("/a/./b/../c//d"), "/a/c/d");
            assert_eq!(join("/a", "b"), "/a/b");
            assert_eq!(join("/a/", "/b"), "/b");
        }
        assert_eq!(human(1536), "1.5 KB");
        assert_eq!(human(10), "10 B");
        assert_eq!(thousands(25027), "25,027");
        assert_eq!(thousands(999), "999");
    }
}
