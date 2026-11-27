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

/// ask GitHub for the newest release (and remember it)
pub fn check_now() -> Result<(), String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => "no releases published yet".to_string(),
            ureq::Error::Status(c, _) => format!("GitHub answered {c}"),
            e => format!("no connection to GitHub ({})", e.kind()),
        })?;
    let v: Value = resp.into_json().map_err(|e| e.to_string())?;
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

/// the version kept for "Go back", if any
pub fn previous() -> Option<(String, String)> {
    let rd = std::fs::read_dir(backup_dir()).ok()?;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(v) = name
            .strip_prefix("dubplate-")
            .map(|v| v.trim_end_matches(".exe").to_string())
        {
            if v != VERSION {
                return Some((v, e.path().to_string_lossy().into_owned()));
            }
        }
    }
    None
}

fn can_replace_self() -> Result<(), String> {
    let exe = exe_path().map_err(|e| e.to_string())?;
    let dir = exe
        .parent()
        .ok_or("can't find where dubplate is installed")?;
    let probe = dir.join(format!(".dubplate-write-test-{}", std::process::id()));
    std::fs::write(&probe, b"")
        .map_err(|_| format!("dubplate's folder ({}) isn't writable", dir.display()))?;
    let _ = std::fs::remove_file(probe);
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
    let exe = exe_path().map_err(|e| e.to_string())?;
    let bdir = backup_dir();
    let _ = std::fs::remove_dir_all(&bdir);
    std::fs::create_dir_all(&bdir).map_err(|e| e.to_string())?;
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    std::fs::copy(&exe, join(&bdir, &format!("dubplate-{VERSION}{suffix}")))
        .map_err(|e| format!("couldn't keep a backup ({e})"))?;
    let tmp = join(
        &paths::app_dir(),
        &format!("dubplate-{version}-download{suffix}"),
    );
    std::fs::write(&tmp, new_exe).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    let r =
        self_replace::self_replace(&tmp).map_err(|e| format!("couldn't replace dubplate ({e})"));
    let _ = std::fs::remove_file(&tmp);
    r
}

/// go back to the version that was kept when updating
pub fn rollback() -> Result<String, String> {
    let (v, path) = previous().ok_or("there's no earlier version to go back to")?;
    can_replace_self()?;
    let data = std::fs::read(&path).map_err(|e| e.to_string())?;
    let exe = exe_path().map_err(|e| e.to_string())?;
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let keep = join(&backup_dir(), &format!("dubplate-{VERSION}{suffix}"));
    let _ = std::fs::copy(&exe, &keep);
    let tmp = join(&paths::app_dir(), &format!("dubplate-{v}-rollback{suffix}"));
    std::fs::write(&tmp, &data).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    let r =
        self_replace::self_replace(&tmp).map_err(|e| format!("couldn't replace dubplate ({e})"));
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(&path);
    r.map(|_| v)
}

/// start the (new) program again in place of this one, keeping the window
pub fn restart() -> ! {
    let exe = exe_path().expect("current exe");
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "--no-browser") {
        args.push("--no-browser".into());
    }
    let mut cmd = std::process::Command::new(exe);
    cmd.args(&args).env("DUBPLATE_RESTART", "1");
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
        assert!(newer("10.0.0", "9.9.9"));
        assert!(!newer("2.0.0", "2.0.0"));
        assert!(!newer("garbage", "2.0.0"));
    }
}
