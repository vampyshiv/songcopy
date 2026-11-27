//! Updates from GitHub Releases: check (at most every 6 hours, or on request), download the
//! build for this computer, verify it against the release's SHA256SUMS, keep the current
//! version for "Go back", replace this program and restart it.

use crate::discover::{load_settings, save_setting};
use crate::paths::{self, join};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const REPO: &str = "vampyshiv/dubplate";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECK_EVERY: u64 = 6 * 3600;

/// where this program was started from, remembered at startup: once the file has been
/// replaced, Linux reports the running program as "dubplate (deleted)"
static EXE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

pub fn remember_exe() {
    if let Ok(p) = std::env::current_exe() {
        let _ = EXE.set(p);
    }
}

fn exe_path() -> std::io::Result<std::path::PathBuf> {
    match EXE.get() {
        Some(p) => Ok(p.clone()),
        None => std::env::current_exe(),
    }
}

pub fn asset_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("dubplate-windows-x86_64.exe"),
        ("linux", "x86_64") => Some("dubplate-linux-x86_64"),
        _ => None,
    }
}

#[derive(Clone, Default)]
struct Release {
    version: String,
    notes: String,
    url: String,
    asset_url: Option<String>,
    sums_url: Option<String>,
}

#[derive(Default)]
struct State {
    checking: bool,
    error: Option<String>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .user_agent(&format!("dubplate-updater/{VERSION}"))
        .build()
}

fn newer(latest: &str, current: &str) -> bool {
    match (
        semver::Version::parse(latest),
        semver::Version::parse(current),
    ) {
        (Ok(l), Ok(c)) => l > c,
        _ => false,
    }
}

fn cached_release() -> Option<Release> {
    let s = load_settings();
    let r = s.get("update_latest")?;
    Some(Release {
        version: r.get("version")?.as_str()?.to_string(),
        notes: r
            .get("notes")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        url: r
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        asset_url: r
            .get("asset_url")
            .and_then(Value::as_str)
            .map(str::to_string),
        sums_url: r
            .get("sums_url")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// is this an alpha / beta (test) build? Test builds are offered newer test builds too;
/// regular builds only regular releases (GitHub's "latest" leaves pre-releases out)
fn test_build(v: &str) -> bool {
    semver::Version::parse(v).is_ok_and(|v| !v.pre.is_empty())
}

/// of GitHub's releases (newest first), the one with the highest version: test builds included
/// or not, drafts never
fn pick_release(list: &[Value], with_tests: bool) -> Option<&Value> {
    list.iter()
        .filter(|r| r.get("draft").and_then(Value::as_bool) != Some(true))
        .filter(|r| with_tests || r.get("prerelease").and_then(Value::as_bool) != Some(true))
        .filter_map(|r| {
            let tag = r.get("tag_name")?.as_str()?.trim_start_matches('v');
            Some((semver::Version::parse(tag).ok()?, r))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, r)| r)
}

/// ask GitHub for the newest release (and remember it)
pub fn check_now() -> Result<(), String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=50");
    let resp = agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(c, _) => format!("GitHub answered {c}"),
            e => format!("no connection to GitHub ({})", e.kind()),
        })?;
    let list: Value = resp.into_json().map_err(|e| e.to_string())?;
    let list = list.as_array().cloned().unwrap_or_default();
    let Some(v) = pick_release(&list, test_build(VERSION)).cloned() else {
        return Err("no releases published yet".to_string());
    };
    let tag = v.get("tag_name").and_then(Value::as_str).unwrap_or("");
    let asset = |name: &str| {
        v.get("assets")?
            .as_array()?
            .iter()
            .find(|a| a.get("name").and_then(Value::as_str) == Some(name))?
            .get("browser_download_url")?
            .as_str()
            .map(str::to_string)
    };
    let r = json!({
        "version": tag.trim_start_matches('v'),
        "notes": v.get("body").and_then(Value::as_str).unwrap_or(""),
        "url": v.get("html_url").and_then(Value::as_str).unwrap_or(""),
        "asset_url": asset_name().and_then(asset),
        "sums_url": asset("SHA256SUMS"),
    });
    save_setting("update_latest", r);
    save_setting("update_checked", Value::from(now()));
    Ok(())
}

fn check_in_background() {
    {
        let mut st = STATE.lock().unwrap();
        if st.checking {
            return;
        }
        st.checking = true;
    }
    std::thread::spawn(|| {
        let r = check_now();
        let mut st = STATE.lock().unwrap();
        st.checking = false;
        st.error = r.err();
    });
}

pub fn auto_enabled() -> bool {
    load_settings()
        .get("update_auto")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn backup_dir() -> String {
    join(&paths::app_dir(), "previous")
}

const SUFFIX: &str = if cfg!(windows) { ".exe" } else { "" };

/// the version kept for "Go back", if any. Also finds the copy songcopy (dubplate's old name)
/// kept when it updated itself onto dubplate.
pub fn previous() -> Option<(String, String)> {
    let dirs = [
        (backup_dir(), "dubplate-"),
        (join(&paths::app_dir(), "previous.new"), "dubplate-"), // a swap that didn't finish tidying up
        (
            join(&join(&paths::app_base(), "songcopy"), "previous"),
            "songcopy-",
        ),
    ];
    for (dir, prefix) in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(v) = name
                .strip_prefix(prefix)
                .and_then(|v| v.strip_suffix(SUFFIX).or(Some(v)))
            else {
                continue;
            };
            if v != VERSION && semver::Version::parse(v).is_ok() && e.path().is_file() {
                return Some((v.to_string(), e.path().to_string_lossy().into_owned()));
            }
        }
    }
    None
}

static WRITABLE: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// can this program replace its own file? It writes a test file next to itself; a "yes" is
/// remembered, a "no" is checked again next time (the user may have fixed the folder)
fn can_replace_self() -> Result<(), String> {
    if WRITABLE.get().is_some() {
        return Ok(());
    }
    let exe = exe_path().map_err(|e| e.to_string())?;
    let dir = exe
        .parent()
        .ok_or("can't find where dubplate is installed")?;
    let probe = dir.join(format!(".dubplate-write-test-{}", std::process::id()));
    std::fs::write(&probe, b"")
        .map_err(|_| format!("dubplate's folder ({}) isn't writable", dir.display()))?;
    let _ = std::fs::remove_file(probe);
    let _ = WRITABLE.set(());
    Ok(())
}

#[derive(Serialize)]
pub struct Status {
    current: String,
    latest: Option<String>,
    available: bool,
    notes: String,
    url: String,
    auto: bool,
    checking: bool,
    error: Option<String>,
    previous: Option<String>,
    can_install: bool,
    why_not: Option<String>,
}

/// what the page shows. Starts a check when asked, or when the last one is over 6 hours old.
pub fn status(force: bool) -> Status {
    let s = load_settings();
    let last = s.get("update_checked").and_then(Value::as_u64).unwrap_or(0);
    if force || (auto_enabled() && now().saturating_sub(last) > CHECK_EVERY) {
        check_in_background();
    }
    let rel = cached_release();
    let available = rel.as_ref().is_some_and(|r| newer(&r.version, VERSION));
    let why_not = if asset_name().is_none() {
        Some(
            "No ready-made build for this kind of computer - update with git and cargo instead."
                .to_string(),
        )
    } else if available
        && rel
            .as_ref()
            .is_some_and(|r| r.asset_url.is_none() || r.sums_url.is_none())
    {
        Some("That release has no download for this computer yet.".to_string())
    } else {
        can_replace_self().err()
    };
    let st = STATE.lock().unwrap();
    Status {
        current: VERSION.into(),
        latest: rel.as_ref().map(|r| r.version.clone()),
        available,
        notes: rel.as_ref().map(|r| r.notes.clone()).unwrap_or_default(),
        url: rel.map(|r| r.url).unwrap_or_default(),
        auto: auto_enabled(),
        checking: st.checking,
        error: st.error.clone(),
        previous: previous().map(|p| p.0),
        can_install: why_not.is_none(),
        why_not,
    }
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let resp = agent()
        .get(url)
        .call()
        .map_err(|e| format!("download failed ({e})"))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(limit)
        .read_to_end(&mut buf)
        .map_err(|e| format!("download failed ({e})"))?;
    Ok(buf)
}

/// download + verify + install the newest release. Returns the version installed.
pub fn apply() -> Result<String, String> {
    check_now()?;
    let rel = cached_release().ok_or("couldn't read the release")?;
    if !newer(&rel.version, VERSION) {
        return Err(format!("you already have the newest version ({VERSION})"));
    }
    let name = asset_name().ok_or("no ready-made build for this kind of computer")?;
    let (Some(asset_url), Some(sums_url)) = (rel.asset_url.clone(), rel.sums_url.clone()) else {
        return Err("that release has no download for this computer".into());
    };
    can_replace_self()?;
    let sums = String::from_utf8(download(&sums_url, 1 << 20)?).map_err(|_| "bad checksum file")?;
    let want = sums
        .lines()
        .find_map(|l| {
            let (h, n) = l.split_once(char::is_whitespace)?;
            (n.trim().trim_start_matches('*') == name).then(|| h.to_lowercase())
        })
        .ok_or("the release's checksum file doesn't list this download")?;
    let data = download(&asset_url, 200 << 20)?;
    let got = format!("{:x}", Sha256::digest(&data));
    if got != want {
        return Err(
            "the download is damaged (checksum doesn't match) - nothing was changed".into(),
        );
    }
    install(&data, &rel.version)?;
    Ok(rel.version)
}

/// keep the running version for "Go back", then swap the new one in
fn install(new_exe: &[u8], version: &str) -> Result<(), String> {
    swap(new_exe, version, "download")
}

/// go back to the version that was kept when updating (the current one is kept in turn,
/// so you can go forward again)
pub fn rollback() -> Result<String, String> {
    let (v, path) = previous().ok_or("there's no earlier version to go back to")?;
    can_replace_self()?;
    let data = std::fs::read(&path).map_err(|e| e.to_string())?;
    swap(&data, &v, "rollback")?;
    // going back to songcopy (dubplate's old name): it only looks in its own folder, so leave
    // this version there too - then songcopy offers "Go back" to dubplate again
    if path.contains(&format!("{}songcopy{}", paths::SEP, paths::SEP)) {
        if let Ok(exe) = std::fs::read(join(&backup_dir(), &format!("dubplate-{VERSION}{SUFFIX}")))
        {
            let dir = paths::dirname(&path).to_string();
            let _ = std::fs::write(join(&dir, &format!("songcopy-{VERSION}{SUFFIX}")), exe);
        }
    }
    Ok(v)
}

/// Replace this program with `new_exe`, keeping the current version for "Go back".
/// The old backup is only dropped once the swap has worked, so a failure never loses it.
fn swap(new_exe: &[u8], version: &str, what: &str) -> Result<(), String> {
    let exe = exe_path().map_err(|e| e.to_string())?;
    // the Windows swap moves the running program aside before copying the new one in:
    // make sure there's room for both, so it can't be left half done
    let dir = exe
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let need = new_exe.len() as u64 * 2 + (16 << 20);
    if crate::server::free_space(&dir) < need {
        return Err(format!(
            "not enough free space next to dubplate (needs {})",
            paths::human(need)
        ));
    }
    let app = paths::app_dir();
    let staged = join(&app, "previous.new");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).map_err(|e| e.to_string())?;
    std::fs::copy(&exe, join(&staged, &format!("dubplate-{VERSION}{SUFFIX}")))
        .map_err(|e| format!("couldn't keep a backup ({e})"))?;
    let tmp = join(&app, &format!("dubplate-{version}-{what}{SUFFIX}"));
    let r = (|| {
        std::fs::write(&tmp, new_exe).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
        }
        self_replace::self_replace(&tmp).map_err(|e| format!("couldn't replace dubplate ({e})"))
    })();
    let _ = std::fs::remove_file(&tmp);
    if r.is_err() {
        let _ = std::fs::remove_dir_all(&staged); // the old backup stays as it was
        return r;
    }
    // the new backup takes the old one's place; the old one is only removed once that worked
    let old = join(&app, "previous.old");
    let _ = std::fs::remove_dir_all(&old);
    let had_old = std::fs::rename(backup_dir(), &old).is_ok();
    if std::fs::rename(&staged, backup_dir()).is_ok() {
        let _ = std::fs::remove_dir_all(&old);
    } else if had_old {
        let _ = std::fs::rename(&old, backup_dir()); // keep what was there; previous() also looks in previous.new
    }
    Ok(())
}

/// was this process started by restart()? (songcopy, dubplate's old name, used SONGCOPY_RESTART)
pub fn is_restart() -> bool {
    std::env::var_os("DUBPLATE_RESTART").is_some() || std::env::var_os("SONGCOPY_RESTART").is_some()
}

/// start the (new) program again in place of this one, keeping the window
pub fn restart() -> ! {
    let exe = exe_path().expect("current exe");
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "--no-browser") {
        args.push("--no-browser".into());
    }
    let mut cmd = std::process::Command::new(exe);
    // both names, so going back to a songcopy version also knows it's a restart
    cmd.args(&args)
        .env("DUBPLATE_RESTART", "1")
        .env("SONGCOPY_RESTART", "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let e = cmd.exec(); // same process, same terminal
        eprintln!("couldn't restart dubplate: {e}");
        std::process::exit(1);
    }
    #[cfg(not(unix))]
    {
        match cmd.spawn() {
            Ok(_) => std::process::exit(0),
            Err(e) => {
                eprintln!("couldn't restart dubplate: {e}");
                std::process::exit(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("2.1.0", "2.0.0"));
        assert!(newer("1.0.0-alpha.2", "1.0.0-alpha.1"));
        assert!(newer("1.0.0-beta.1", "1.0.0-alpha.9"));
        assert!(newer("1.0.0", "1.0.0-beta.3"));
        assert!(test_build("1.0.0-alpha.1") && !test_build("1.0.0"));
        let rel = |tag: &str, pre: bool, draft: bool| json!({"tag_name": tag, "prerelease": pre, "draft": draft});
        let list = vec![rel("v1.0.0-beta.1", true, false), rel("v1.0.0-beta.2", true, true), rel("v1.0.0-alpha.3", true, false)];
        let tag = |r: Option<&Value>| r.and_then(|r| r["tag_name"].as_str()).map(str::to_string);
        assert_eq!(tag(pick_release(&list, true)).as_deref(), Some("v1.0.0-beta.1")); // drafts never
        assert_eq!(tag(pick_release(&list, false)), None); // regular builds: no test builds
        let mut list = list;
        list.push(rel("v1.0.0", false, false));
        assert_eq!(tag(pick_release(&list, false)).as_deref(), Some("v1.0.0"));
        assert_eq!(tag(pick_release(&list, true)).as_deref(), Some("v1.0.0"));
        assert!(newer("10.0.0", "9.9.9"));
        assert!(!newer("2.0.0", "2.0.0"));
        assert!(!newer("garbage", "2.0.0"));
    }
}
