//! The web page and its small JSON API (same API as the Python version, plus updates).

use crate::copy::{self, Item, Ledger, Transfer};
use crate::discover;
use crate::library::Library;
use crate::lists;
use crate::paths::{self, human, is_audio, join, nearest_existing, WINDOWS};
use crate::update;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock, Mutex};
use tiny_http::{Header, Method, Response, Server, StatusCode};

const PAGE: &str = include_str!("page.html");

pub struct Options {
    pub music: String,
    pub port: u16,
    pub host: String,
    pub no_browser: bool,
    pub dest: Option<String>,
    /// extra names the page may be opened under (reachable from the network only)
    pub allow_hosts: Vec<String>,
}

#[derive(Serialize, Default, Clone)]
struct Job {
    total: usize,
    n: usize,
    copied: usize,
    converted: usize,
    skipped: usize,
    unchecked: usize,
    errors: Vec<copy::CopyError>,
    current: String,
    finished: bool,
    dest: String,
    playlist: Option<String>,
    done_srcs: Vec<String>,
}

static JOBS: LazyLock<Mutex<HashMap<String, Job>>> = LazyLock::new(Default::default);
static FFMPEG_INSTALL: LazyLock<Mutex<(bool, String)>> = LazyLock::new(Default::default);

struct Ctx {
    lib: Arc<Library>,
    dest: Option<String>,
    local_only: bool,
    allow_hosts: Vec<String>,
}

fn when(t: Option<chrono::NaiveDateTime>) -> Value {
    match t {
        None => Value::Null,
        Some(t) if t.date() == chrono::Local::now().date_naive() => {
            Value::from(t.format("%H:%M").to_string())
        }
        Some(t) => Value::from(t.format("%d %b %H:%M").to_string()),
    }
}

pub fn open_browser(url: &str) -> bool {
    use std::process::Command;
    if WINDOWS {
        return Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()
            .is_ok();
    }
    if cfg!(target_os = "macos") {
        return Command::new("open").arg(url).spawn().is_ok();
    }
    // never a text browser inside this terminal
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return false;
    }
    if let Ok(b) = std::env::var("BROWSER") {
        let parts: Vec<String> = b.split_whitespace().map(|p| p.replace("%s", url)).collect();
        if let Some((prog, rest)) = parts.split_first() {
            let mut c = Command::new(prog);
            c.args(rest);
            if !b.contains("%s") {
                c.arg(url);
            }
            return c.spawn().is_ok();
        }
    }
    Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

pub fn already_running(port: u16) -> bool {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(1))
        .build();
    agent
        .get(&format!("http://127.0.0.1:{port}/api/info"))
        .call()
        .ok()
        .and_then(|r| r.into_json::<Value>().ok())
        .is_some_and(|v| v.get("root").is_some())
}

pub fn serve(o: Options) {
    let url = format!("http://localhost:{}", o.port);
    let restarting = crate::update::is_restart();
    if !paths::isdir(&paths::abspath(&paths::expanduser(&o.music))) {
        eprintln!("Can't open the music folder: {}", o.music);
        std::process::exit(1);
    }
    let addr = format!("{}:{}", o.host, o.port);
    let mut tries = 0;
    let server = loop {
        match Server::http(&addr) {
            Ok(s) => break s,
            Err(e) => {
                if restarting && tries < 60 {
                    // the old version is still letting go of the port
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    continue;
                }
                if already_running(o.port) {
                    println!("Dubplate is already running - opening {url}");
                    if o.no_browser || !open_browser(&url) {
                        println!("Open {url} in your browser.");
                    }
                    return;
                }
                eprintln!(
                    "Can't start on port {}: {e}\nTry:  dubplate serve --port {}",
                    o.port,
                    o.port + 1
                );
                std::process::exit(1);
            }
        }
    };

    let lib = Arc::new(Library::new(&o.music));
    if lib.load_cache() {
        println!(
            "Using saved song list ({} songs); checking for new songs in the background.",
            paths::thousands(lib.len())
        );
    } else {
        println!("Reading your library {} for the first time ...", lib.root);
    }
    {
        let lib = lib.clone();
        std::thread::spawn(move || {
            lib.scan(|_| {});
        });
    }
    println!("\n  Dubplate {} is running at  {url}", update::VERSION);
    if o.host == "0.0.0.0" {
        println!("  (also reachable from other devices on your network at this computer's IP)");
    }
    println!("  Leave this window open while you use it. Press Ctrl+C (or close it) to stop.\n");
    if !o.no_browser && !restarting && !open_browser(&url) {
        println!("  Open {url} in your browser.\n");
    }
    let ctx = Arc::new(Ctx {
        lib,
        dest: o.dest,
        local_only: o.host == "127.0.0.1" || o.host == "localhost",
        allow_hosts: {
            // this computer's own name, plus any the user allowed
            let mut a: Vec<String> = o
                .allow_hosts
                .iter()
                .map(|h| h.trim().trim_end_matches('.').to_ascii_lowercase())
                .collect();
            a.extend(machine_name());
            a
        },
    });
    for req in server.incoming_requests() {
        let ctx = ctx.clone();
        std::thread::spawn(move || handle(&ctx, req));
    }
}

fn json_response(code: u16, v: &Value) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_data(v.to_string().into_bytes())
        .with_status_code(StatusCode(code))
        .with_header(Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap())
        .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap())
}

fn header<'a>(req: &'a tiny_http::Request, name: &str) -> Option<&'a str> {
    req.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

fn query(url: &str) -> HashMap<String, String> {
    let Some((_, q)) = url.split_once('?') else {
        return HashMap::new();
    };
    q.split('&')
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (decode_url(k), decode_url(v))
        })
        .collect()
}

fn decode_url(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
                {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn handle(ctx: &Ctx, mut req: tiny_http::Request) {
    // only answer the page itself: not other websites open in the same browser
    if !host_allowed(
        header(&req, "Host").unwrap_or(""),
        ctx.local_only,
        &ctx.allow_hosts,
    ) {
        let _ = req.respond(json_response(403, &json!({"error": "forbidden"})));
        return;
    }
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    let method = req.method().clone();
    if method == Method::Get {
        if path == "/" {
            let r = Response::from_data(PAGE.as_bytes().to_vec())
                .with_header(
                    Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap(),
                )
                .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap());
            let _ = req.respond(r);
            return;
        }
        if path == "/api/play" {
            let p = query(&url).remove("path").unwrap_or_default();
            return play(ctx, req, &p);
        }
        let v = get(ctx, &path, &query(&url));
        let _ = req.respond(json_response(
            if v.get("error") == Some(&json!("not found")) {
                404
            } else {
                200
            },
            &v,
        ));
        return;
    }
    if method != Method::Post {
        let _ = req.respond(json_response(405, &json!({"error": "method not allowed"})));
        return;
    }
    // a JSON body can only come from the page itself (other sites can't send it without asking first)
    if !header(&req, "Content-Type").is_some_and(|c| c.starts_with("application/json")) {
        let _ = req.respond(json_response(415, &json!({"error": "expected JSON"})));
        return;
    }
    let mut body = String::new();
    if req
        .as_reader()
        .take(512 << 20)
        .read_to_string(&mut body)
        .is_err()
    {
        let _ = req.respond(json_response(400, &json!({"error": "bad request"})));
        return;
    }
    let b: Value = serde_json::from_str(&body).unwrap_or(json!({}));
    let (v, then_restart) = post(ctx, &path, &b);
    let _ = req.respond(json_response(200, &v));
    if then_restart {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(400));
            update::restart();
        });
    }
}

/// Is this the page's own address? Always: localhost. When reachable from the network
/// (--host 0.0.0.0) also this machine's own addresses and local names ("myserver",
/// "myserver.local"). Never a website's domain: that's how a page elsewhere could
/// trick the browser into talking to dubplate ("DNS rebinding").
fn host_allowed(host: &str, local_only: bool, allowed: &[String]) -> bool {
    let h = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("") // [::1]:8765
    } else {
        host.rsplit_once(':').map(|x| x.0).unwrap_or(host)
    };
    let h = h.trim_end_matches('.').to_ascii_lowercase(); // "myserver.lan." = "myserver.lan"
    if matches!(h.as_str(), "localhost" | "127.0.0.1" | "::1") {
        return true;
    }
    if local_only || h.is_empty() {
        return false;
    }
    // this machine's name in any form ("dahlias-den.fritz.box") and names the user allowed
    if allowed
        .iter()
        .any(|a| !a.is_empty() && (h == *a || h.starts_with(&format!("{a}."))))
    {
        return true;
    }
    h.parse::<std::net::IpAddr>().is_ok()
        || !h.contains('.')
        || [".local", ".lan", ".home", ".internal", ".home.arpa"]
            .iter()
            .any(|t| h.ends_with(t))
}

/// this computer's network name ("dahlias-den"), lower case
fn machine_name() -> Option<String> {
    let n = if WINDOWS {
        std::env::var("COMPUTERNAME").ok()
    } else {
        std::fs::read_to_string("/proc/sys/kernel/hostname")
            .or_else(|_| std::fs::read_to_string("/etc/hostname"))
            .ok()
    }?;
    let n = n.trim().to_ascii_lowercase();
    (!n.is_empty()).then_some(n)
}

/// a song file inside the music library. Judged by the path itself with ".." resolved, so it
/// can't step out of the library - while songs in a library folder that is a shortcut
/// (symlink) to another drive still count.
fn in_library(ctx: &Ctx, p: &str) -> bool {
    let root = paths::normcase(&ctx.lib.root);
    let inside = format!("{}{}", root.trim_end_matches(paths::SEP), paths::SEP);
    paths::normcase(&paths::abspath(p)).starts_with(&inside) && paths::isfile(p) && is_audio(p)
}

fn s<'a>(b: &'a Value, k: &str) -> &'a str {
    b.get(k).and_then(Value::as_str).unwrap_or("")
}

fn get(ctx: &Ctx, path: &str, q: &HashMap<String, String>) -> Value {
    let lib = &ctx.lib;
    match path {
        "/api/info" => {
            let (ff, fp) = copy::ffmpeg_path();
            let fi = FFMPEG_INSTALL.lock().unwrap().clone();
            let home = paths::home();
            json!({
                "root": lib.root, "count": lib.len(), "ready": lib.is_ready(),
                "scanning": lib.scanning.load(Ordering::SeqCst), "progress": lib.progress.load(Ordering::Relaxed),
                "scanned": when(lib.scanned_at()), "ffmpeg": ff.is_some() && fp.is_some(), "windows": WINDOWS,
                "version": update::VERSION, "appdir": paths::app_dir(), "ffmpeg_installing": fi.0, "ffmpeg_log": fi.1,
                "home": home, "dest": ctx.dest.clone().unwrap_or_default(),
                "example": join(&join(&home, "Desktop"), "My songs"),
            })
        }
        "/api/ls" => list_dir(q.get("path").map(String::as_str).unwrap_or("~")),
        "/api/update/status" => {
            serde_json::to_value(update::status(q.contains_key("check"))).unwrap_or(json!({}))
        }
        p if p.starts_with("/api/job/") => {
            let id = &p["/api/job/".len()..];
            match JOBS.lock().unwrap().get(id) {
                Some(j) => serde_json::to_value(j).unwrap(),
                None => json!({"error": "not found"}),
            }
        }
        _ => json!({"error": "not found"}),
    }
}

fn list_dir(raw: &str) -> Value {
    if WINDOWS && raw.is_empty() {
        // "This PC": the drives
        let drives: Vec<Value> = ('A'..='Z')
            .filter(|d| logical_drives() & (1 << (*d as u32 - 'A' as u32)) != 0)
            .map(|d| format!("{d}:\\"))
            .map(|d| json!({"name": d, "path": d}))
            .collect();
        return json!({"path": "", "parent": null, "free": "", "dirs": drives});
    }
    let p = paths::abspath(&paths::expanduser(if raw.is_empty() { "~" } else { raw }));
    let ex = nearest_existing(&p);
    let mut names: Vec<String> = match std::fs::read_dir(&ex) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.') && !n.starts_with('$'))
            .collect(),
        Err(e) => return json!({"path": ex, "dirs": [], "error": e.to_string()}),
    };
    names.sort_by_key(|n| n.to_lowercase());
    names.truncate(500);
    let up = paths::dirname(&ex);
    let parent = if up == ex || up.is_empty() {
        if WINDOWS {
            Value::from("")
        } else {
            Value::Null
        } // top of a drive
    } else {
        Value::from(up)
    };
    json!({
        "path": ex, "exists": ex == p, "parent": parent,
        "dirs": names.iter().map(|n| json!({"name": n, "path": join(&ex, n)})).collect::<Vec<_>>(),
        "free": human(free_space(&ex)),
    })
}

/// free space on the drive holding p
pub fn free_space(p: &str) -> u64 {
    #[cfg(unix)]
    {
        let Ok(c) = std::ffi::CString::new(p) else {
            return u64::MAX;
        };
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: c is a valid C string and st a properly sized, writable statvfs
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0 {
            return st.f_bavail as u64 * st.f_frsize as u64;
        }
        u64::MAX
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let w: Vec<u16> = std::ffi::OsStr::new(p)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut free: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                w.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok != 0 {
            free
        } else {
            u64::MAX
        }
    }
}

/// which drive letters exist, without touching the drives (an empty card reader would
/// otherwise make Windows complain)
fn logical_drives() -> u32 {
    #[cfg(windows)]
    {
        unsafe { GetLogicalDrives() }
    }
    #[cfg(not(windows))]
    {
        0
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetLogicalDrives() -> u32;
    fn GetDiskFreeSpaceExW(
        dir: *const u16,
        free_to_caller: *mut u64,
        total: *mut u64,
        total_free: *mut u64,
    ) -> i32;
}

#[derive(Debug, PartialEq)]
enum Range {
    Full,
    Part(u64, u64),
    Unsatisfiable,
}

/// one "Range: bytes=..." (RFC 9110). Anything we don't understand -> the whole file.
fn parse_range(h: Option<&str>, size: u64) -> Range {
    let Some(r) = h.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return Range::Full;
    };
    if r.contains(',') {
        return Range::Full; // several ranges: sending everything is allowed
    }
    let Some((a, b)) = r.split_once('-') else {
        return Range::Full;
    };
    let (a, b) = (a.trim(), b.trim());
    let num = |x: &str| {
        x.parse::<u64>()
            .ok()
            .filter(|_| x.bytes().all(|c| c.is_ascii_digit()))
    };
    let (start, end) = if a.is_empty() {
        // the last n bytes
        let Some(n) = num(b) else { return Range::Full };
        if n == 0 || size == 0 {
            return Range::Unsatisfiable;
        }
        (size - n.min(size), size - 1)
    } else {
        let Some(start) = num(a) else {
            return Range::Full;
        };
        let end = if b.is_empty() {
            u64::MAX
        } else {
            match num(b) {
                Some(e) if e >= start => e,
                _ => return Range::Full, // invalid: ignore it
            }
        };
        if start >= size {
            return Range::Unsatisfiable;
        }
        (start, end.min(size - 1))
    };
    Range::Part(start, end)
}

fn play(ctx: &Ctx, req: tiny_http::Request, p: &str) {
    if !in_library(ctx, p) {
        let _ = req.respond(json_response(404, &json!({"error": "not found"})));
        return;
    }
    let Ok(mut f) = std::fs::File::open(p) else {
        let _ = req.respond(json_response(404, &json!({"error": "not found"})));
        return;
    };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let ctype = match paths::splitext(p).1.to_lowercase().as_str() {
        ".mp3" => "audio/mpeg",
        ".flac" => "audio/flac",
        ".m4a" => "audio/mp4",
        ".opus" | ".ogg" => "audio/ogg",
        ".wav" => "audio/wav",
        _ => "application/octet-stream",
    };
    let (start, end, code) = match parse_range(header(&req, "Range"), size) {
        Range::Full => (0, size.saturating_sub(1), 200),
        Range::Part(a, b) => (a, b, 206),
        Range::Unsatisfiable => {
            let r = Response::empty(StatusCode(416)).with_header(
                Header::from_bytes("Content-Range", format!("bytes */{size}")).unwrap(),
            );
            let _ = req.respond(r);
            return;
        }
    };
    let len = if size == 0 { 0 } else { end - start + 1 };
    let _ = f.seek(SeekFrom::Start(start));
    let mut headers = vec![
        Header::from_bytes("Content-Type", ctype).unwrap(),
        Header::from_bytes("Accept-Ranges", "bytes").unwrap(),
    ];
    if code == 206 {
        headers.push(
            Header::from_bytes("Content-Range", format!("bytes {start}-{end}/{size}")).unwrap(),
        );
    }
    let resp = Response::new(
        StatusCode(code),
        headers,
        f.take(len),
        Some(len as usize),
        None,
    );
    let _ = req.respond(resp);
}

fn decode_b64(s: &str) -> Vec<u8> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut val = [255u8; 256];
    for (i, c) in T.iter().enumerate() {
        val[*c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = val[c as usize];
        if v == 255 {
            continue;
        }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

fn post(ctx: &Ctx, path: &str, b: &Value) -> (Value, bool) {
    let lib = &ctx.lib;
    let v = match path {
        "/api/rescan" => {
            if !lib.scanning.load(Ordering::SeqCst) {
                let lib = lib.clone();
                std::thread::spawn(move || {
                    lib.scan(|_| {});
                });
            }
            json!({"started": true})
        }
        "/api/match" => {
            let reqs = if let Some(t) = b.get("text").and_then(Value::as_str) {
                let f = Some(s(b, "filename"))
                    .filter(|f| !f.is_empty())
                    .unwrap_or("pasted.txt");
                lists::read_rows(f, t.as_bytes())
            } else {
                let f = Some(s(b, "filename"))
                    .filter(|f| !f.is_empty())
                    .unwrap_or("list.csv");
                lists::read_rows(f, &decode_b64(s(b, "data")))
            };
            match reqs {
                Err(e) => json!({"error": format!("Couldn't read that file: {e}")}),
                Ok(r) if r.is_empty() => json!({"error": "No songs found in that list."}),
                Ok(reqs) => {
                    let prefer = Some(s(b, "prefer"))
                        .filter(|p| !p.is_empty())
                        .unwrap_or("flac");
                    lib.wait_ready(); // first-ever run: wait until the library has been read
                    let view = lib.view();
                    let results: Vec<_> = reqs.iter().map(|r| view.match_one(r, prefer)).collect();
                    json!({"results": results})
                }
            }
        }
        "/api/search" => {
            lib.wait_ready();
            let prefer = Some(s(b, "prefer"))
                .filter(|p| !p.is_empty())
                .unwrap_or("flac");
            json!({"files": lib.view().search(s(b, "q"), prefer)})
        }
        "/api/copy" => start_copy(ctx, b),
        "/api/copied" => {
            let srcs: Vec<String> = b
                .get("srcs")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let found = if s(b, "mode") == "usb" {
                // a stick remembers per playlist folder
                let stick = paths::abspath(&paths::expanduser(s(b, "dest")));
                let folder = copy::usb_folder(&stick, s(b, "usb_name"));
                if s(b, "dest").trim().is_empty() {
                    Default::default()
                } else {
                    Ledger::copied_usb(&stick, &folder, &srcs)
                }
            } else {
                Ledger::copied(s(b, "dest"), &srcs)
            };
            json!({"copied": found.into_keys().collect::<Vec<_>>()})
        }
        "/api/install_ffmpeg" if WINDOWS => {
            let mut st = FFMPEG_INSTALL.lock().unwrap();
            let (ff, fp) = copy::ffmpeg_path();
            if !st.0 && !(ff.is_some() && fp.is_some()) {
                st.0 = true;
                std::thread::spawn(install_ffmpeg);
            }
            json!({"started": true})
        }
        "/api/update/settings" => {
            if let Some(a) = b.get("auto").and_then(Value::as_bool) {
                discover::save_setting("update_auto", Value::from(a));
            }
            json!({"ok": true})
        }
        "/api/update/apply" => {
            return match update::apply() {
                Ok(v) => (json!({"ok": true, "version": v}), true),
                Err(e) => (json!({"error": e}), false),
            }
        }
        "/api/update/rollback" => {
            return match update::rollback() {
                Ok(v) => (json!({"ok": true, "version": v}), true),
                Err(e) => (json!({"error": e}), false),
            }
        }
        _ => json!({"error": "not found"}),
    };
    (v, false)
}

fn install_ffmpeg() {
    let r = copy::quiet(std::process::Command::new("winget").args([
        "install",
        "-e",
        "--id",
        "Gyan.FFmpeg",
        "--scope",
        "user",
        "--accept-package-agreements",
        "--accept-source-agreements",
        "--disable-interactivity",
    ]))
    .output();
    let log = match r {
        Ok(o) => {
            let t = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            t.chars()
                .rev()
                .take(600)
                .collect::<String>()
                .chars()
                .rev()
                .collect()
        }
        Err(e) => format!("couldn't run winget: {e}"),
    };
    *FFMPEG_INSTALL.lock().unwrap() = (false, log);
}

fn start_copy(ctx: &Ctx, b: &Value) -> Value {
    let dest_raw = s(b, "dest").trim();
    if dest_raw.is_empty() {
        return json!({"error": "Choose a destination folder first."});
    }
    let dest = paths::abspath(&paths::expanduser(dest_raw));
    let items: Vec<Item> = b
        .get("items")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|it| {
                    let src = it.get("src")?.as_str()?.to_string();
                    // reachable from the network: only songs from the library can be copied
                    let ok = paths::isfile(&src) && (ctx.local_only || in_library(ctx, &src));
                    ok.then(|| Item {
                        src,
                        rel: it
                            .get("rel")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        n: it.get("n").and_then(Value::as_u64).map(|n| n as usize),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if items.is_empty() {
        return json!({"error": "Nothing selected to copy."});
    }
    let usb_format = (s(b, "mode") == "usb").then(|| s(b, "usb_format").to_string());
    if let Some(f) = &usb_format {
        if f != "aiff" && f != "mp3" {
            return json!({"error": "Unknown USB format."});
        }
    }
    let (target, plan) = match &usb_format {
        // USB for decks: one playlist folder, numbered in list order
        Some(_) => {
            let t = copy::usb_folder(&dest, s(b, "usb_name"));
            let p = copy::plan_copies(&items, &t, false, true);
            (t, p)
        }
        None => {
            let flag = |k: &str| b.get(k).and_then(Value::as_bool).unwrap_or(false);
            (
                dest.clone(),
                copy::plan_copies(&items, &dest, flag("folders"), flag("number")),
            )
        }
    };
    let total = copy::estimate_size(&plan, usb_format.as_deref());
    let free = free_space(&nearest_existing(&dest));
    if total > free {
        return json!({"error": format!("Not enough space: need about {}, only {} free there.", human(total), human(free))});
    }
    let id = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            & 0xff_ffff_ffff
    );
    JOBS.lock().unwrap().insert(
        id.clone(),
        Job {
            total: plan.len(),
            dest: target.clone(),
            ..Default::default()
        },
    );
    let playlist =
        usb_format.is_none() && b.get("playlist").and_then(Value::as_bool).unwrap_or(false);
    let jid = id.clone();
    let count = plan.len();
    std::thread::spawn(move || {
        // what was copied is remembered per destination: the folder, or the stick's playlist folder
        let key = if usb_format.is_some() {
            target.clone()
        } else {
            dest.clone()
        };
        let r: Transfer = copy::transfer(&plan, &key, usb_format.as_deref(), |k, src| {
            if let Some(j) = JOBS.lock().unwrap().get_mut(&jid) {
                j.n = k - 1;
                j.current = paths::basename(src).to_string();
            }
        });
        let pl = if playlist && !r.done.is_empty() {
            copy::write_playlist(&target, &r.done).map_err(|e| e.to_string())
        } else {
            Err(String::new())
        };
        if let Some(j) = JOBS.lock().unwrap().get_mut(&jid) {
            j.copied = r.copied;
            j.converted = r.converted;
            j.skipped = r.skipped;
            j.unchecked = r.unchecked;
            j.errors = r.errors.clone();
            j.done_srcs = r.pairs.iter().map(|(s, _)| s.clone()).collect();
            match pl {
                Ok(p) => j.playlist = Some(p),
                Err(e) if !e.is_empty() => j.errors.push(copy::CopyError {
                    src: "playlist.m3u8".into(),
                    msg: e,
                }),
                _ => {}
            }
            j.n = j.total;
            j.current.clear();
            j.finished = true;
        }
    });
    json!({"job": id, "count": count, "size": human(total)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_follow_the_standard() {
        assert_eq!(parse_range(None, 100), Range::Full);
        assert_eq!(parse_range(Some("bytes=0-9"), 100), Range::Part(0, 9));
        assert_eq!(parse_range(Some("bytes=90-"), 100), Range::Part(90, 99));
        assert_eq!(parse_range(Some("bytes=90-500"), 100), Range::Part(90, 99));
        assert_eq!(parse_range(Some("bytes=-10"), 100), Range::Part(90, 99));
        assert_eq!(parse_range(Some("bytes=-500"), 100), Range::Part(0, 99));
        assert_eq!(parse_range(Some("bytes=100-"), 100), Range::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=-0"), 100), Range::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=50-10"), 100), Range::Full);
        assert_eq!(parse_range(Some("bytes=abc"), 100), Range::Full);
        assert_eq!(parse_range(Some("bytes=0-1,5-6"), 100), Range::Full);
        assert_eq!(parse_range(Some("bytes=+5-9"), 100), Range::Full);
        assert_eq!(parse_range(Some("bytes=0-"), 0), Range::Unsatisfiable);
    }

    #[allow(non_snake_case)]
    fn HA(host: &str, local_only: bool) -> bool {
        host_allowed(
            host,
            local_only,
            &[
                "dahlias-den".to_string(),
                "dubplate.example.com".to_string(),
            ],
        )
    }

    #[test]
    fn only_the_pages_own_address() {
        assert!(HA("dahlias-den.fritz.box:8765", false)); // the machine's own name, any suffix
        assert!(HA("dubplate.example.com", false)); // allowed with --allow-host
        assert!(HA("myserver.lan.:8765", false)); // trailing dot
        assert!(!HA("dahlias-den.fritz.box:8765", true)); // not reachable from the network: localhost only
        assert!(!HA("dahlias-denx.evil.example", false));
        assert!(HA("localhost:8765", true));
        assert!(HA("127.0.0.1:8765", true));
        assert!(HA("[::1]:8765", true));
        assert!(!HA("evil.example:8765", true));
        assert!(!HA("192.168.0.26:8765", true));
        assert!(!HA("", true));
        // reachable from the network: this machine's names and addresses, never a website's
        assert!(HA("192.168.0.26:8765", false));
        assert!(HA("dahlias-den:8765", false));
        assert!(HA("dahlias-den.local:8765", false));
        assert!(HA("[fe80::1]:8765", false));
        assert!(!HA("evil.example:8765", false));
        assert!(!HA("127.0.0.1.evil.example", false));
        assert!(!HA("", false));
    }

    #[test]
    fn preview_and_copies_stay_inside_the_library() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Music");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("in.mp3"), b"x").unwrap();
        std::fs::write(tmp.path().join("out.mp3"), b"x").unwrap();
        let ctx = Ctx {
            lib: Arc::new(Library::new(&root.to_string_lossy())),
            dest: None,
            local_only: true,
            allow_hosts: vec![],
        };
        let r = root.to_string_lossy().into_owned();
        assert!(in_library(&ctx, &paths::join(&r, "in.mp3")));
        assert!(!in_library(
            &ctx,
            &paths::join(&r, &format!("..{}out.mp3", paths::SEP))
        ));
        assert!(!in_library(
            &ctx,
            &tmp.path().join("out.mp3").to_string_lossy()
        ));
        assert!(!in_library(&ctx, &paths::join(&r, "nope.mp3")));
    }

    #[test]
    fn url_and_base64_decoding() {
        assert_eq!(decode_url("a%20b+c%2Fd"), "a b c/d");
        assert_eq!(decode_url("100%"), "100%");
        assert_eq!(decode_b64("aGVsbG8gd29ybGQ="), b"hello world");
    }
}
