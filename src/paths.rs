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

/// tests that change environment variables (dubplate's folder) take turns
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

/// os.path.join(a, b) for the cases dubplate needs
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
/// (dubplate's own file access handles long paths by itself.)
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

/// where programs keep their own data: %LOCALAPPDATA% or ~/.cache
pub fn app_base() -> String {
    std::env::var("LOCALAPPDATA")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| join(&home(), ".cache"))
}

/// dubplate's own folder for its memory: %LOCALAPPDATA%\dubplate or ~/.cache/dubplate
pub fn app_dir() -> String {
    let base = app_base();
    let dir = join(&base, "dubplate");
    migrate_from_songcopy(&base, &dir);
    dir
}

static MIGRATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
const MIGRATED_MARK: &str = "moved-to-dubplate";

/// dubplate used to be called songcopy: bring its settings, copy memory and saved song lists
/// over, once. Files are copied (the old folder is left as it was), nothing that's already in
/// dubplate's folder is overwritten, and a mark in the old folder records that it's done - only
/// after every file made it, so a failed attempt is simply tried again next time.
fn migrate_from_songcopy(base: &str, dir: &str) {
    let old = join(base, "songcopy");
    if !isdir(&old) || exists(&join(&old, MIGRATED_MARK)) {
        return;
    }
    let _g = MIGRATION.lock().unwrap_or_else(|e| e.into_inner());
    if exists(&join(&old, MIGRATED_MARK)) {
        return; // another thread just did it
    }
    if exists(dir) {
        // dubplate has run here before (e.g. 2.1.0, which copied everything already): leave its
        // folder as the user has it - files deleted on purpose must stay deleted
        let _ = std::fs::write(
            join(&old, MIGRATED_MARK),
            b"dubplate copied this folder's settings\n",
        );
        return;
    }
    let Ok(rd) = std::fs::read_dir(&old) else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let mut all_ok = true;
    for e in rd.flatten() {
        let p = e.path();
        if !(p.is_file() && p.extension().is_some_and(|x| x == "json")) {
            continue;
        }
        let to = Path::new(dir).join(e.file_name());
        if to.exists() {
            continue; // never overwrite what dubplate already has
        }
        // copy under a temporary name, then rename: a half-copied file never looks real
        let tmp = Path::new(dir).join(format!(".migrating-{}", e.file_name().to_string_lossy()));
        let ok = std::fs::copy(&p, &tmp).is_ok()
            && match rename_noreplace(&tmp.to_string_lossy(), &to.to_string_lossy()) {
                Ok(()) => true,
                Err(e) => e.kind() == std::io::ErrorKind::AlreadyExists, // another dubplate was quicker: fine
            };
        let _ = std::fs::remove_file(&tmp);
        if !ok {
            all_ok = false;
        }
    }
    if all_ok {
        let _ = std::fs::write(
            join(&old, MIGRATED_MARK),
            b"dubplate copied this folder's settings\n",
        );
    }
}

/// Rename `from` to `to` only if `to` doesn't exist: an ErrorKind::AlreadyExists error otherwise.
/// Never replaces a file, even one created a moment before (unlike std::fs::rename).
pub fn rename_noreplace(from: &str, to: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let w = |p: &str| -> Vec<u16> {
            std::ffi::OsStr::new(&longp(p))
                .encode_wide()
                .chain(Some(0))
                .collect()
        };
        // flags: no MOVEFILE_REPLACE_EXISTING (so it fails if `to` exists), but write through
        if unsafe { MoveFileExW(w(from).as_ptr(), w(to).as_ptr(), 0x8) } != 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        return match e.raw_os_error() {
            Some(80) | Some(183) => Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, e)),
            _ => Err(e),
        };
    }
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        let (Ok(f), Ok(t)) = (CString::new(from), CString::new(to)) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "bad file name",
            ));
        };
        const RENAME_NOREPLACE: libc::c_uint = 1;
        // SAFETY: valid C strings; renameat2 only reads them
        let r = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                f.as_ptr(),
                libc::AT_FDCWD,
                t.as_ptr(),
                RENAME_NOREPLACE,
            )
        };
        if r == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EEXIST) => {
                return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, e))
            }
            Some(libc::EINVAL) | Some(libc::ENOSYS) | Some(libc::EOPNOTSUPP) => {} // file system can't: below
            _ => return Err(e),
        }
    }
    #[cfg(not(windows))]
    {
        // a hard link also fails if `to` exists; FAT/exFAT sticks have no hard links, so
        // as a last resort check-then-rename (the gap is a few microseconds)
        match std::fs::hard_link(from, to) {
            Ok(()) => {
                let _ = std::fs::remove_file(from);
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
            Err(_) if exists(to) => Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "already exists",
            )),
            Err(_) => std::fs::rename(from, to),
        }
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
}

/// Windows: an antivirus or the search indexer briefly holding a brand-new file makes moving
/// it fail with "access denied" / "in use" - worth waiting a moment and trying again
pub fn transient(e: &std::io::Error) -> bool {
    WINDOWS
        && (e.kind() == std::io::ErrorKind::PermissionDenied
            || matches!(e.raw_os_error(), Some(5) | Some(32) | Some(33)))
}

/// remove our own leftover "<name>.*.tmp" files (from writes cut off by a crash) over an hour old
fn clean_stale_tmp(path: &str) {
    let (dir, name) = (dirname(path), basename(path));
    let Ok(rd) = std::fs::read_dir(if dir.is_empty() { "." } else { dir }) else {
        return;
    };
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with(&format!("{name}."))
            && n.ends_with(".tmp")
            && e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t < hour_ago)
        {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

static TMP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// write a whole file safely: to a temporary name first (unique, so two writers can't mix their
/// data), then renamed over the old one
pub fn write_atomic(path: &str, data: &[u8]) -> std::io::Result<()> {
    write_atomic_as(path, data, false)
}

/// like write_atomic, for files with secrets in them (settings: the slskd API key): only the
/// user can read them (on Linux/macOS; on Windows the user's own app folder already is private)
pub fn write_private(path: &str, data: &[u8]) -> std::io::Result<()> {
    write_atomic_as(path, data, true)
}

fn write_atomic_as(path: &str, data: &[u8], private: bool) -> std::io::Result<()> {
    if let Some(d) = Path::new(path).parent() {
        std::fs::create_dir_all(d)?;
    }
    clean_stale_tmp(path);
    let n = TMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = format!("{path}.{}-{n}.tmp", std::process::id());
    let mut r = std::fs::write(&tmp, data);
    #[cfg(unix)]
    if private && r.is_ok() {
        use std::os::unix::fs::PermissionsExt;
        r = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = private;
    if r.is_ok() {
        for attempt in 1..=10 {
            r = std::fs::rename(&tmp, path);
            match &r {
                Err(e) if transient(e) => {
                    std::thread::sleep(std::time::Duration::from_millis(50 * attempt))
                }
                _ => break,
            }
        }
    }
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
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
