//! Copying (never moves, deletes or overwrites), the memory of what was copied where,
//! and deck-ready USB copies (converting what CDJs/XDJs can't play).

use crate::library::MatchResult;
use crate::paths::{self, basename, dirname, join, longp, normcase, splitext, SEP, WINDOWS};
use regex::Regex;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::process::Command;
use std::sync::{LazyLock, Mutex};

static BAD_CHARS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[<>:"|?*\x00-\x1f]"#).unwrap());

/// make a name safe for USB sticks and Windows
pub fn safe(name: &str) -> String {
    BAD_CHARS.replace_all(name, "_").into_owned()
}

#[derive(Clone, Debug)]
pub struct Item {
    pub src: String,
    pub rel: String,
    /// position in the whole list (so numbers stay put when later runs copy the rest)
    pub n: Option<usize>,
}

/// -> [(src, dst)] in list order; never two sources to one destination
pub fn plan_copies(
    items: &[Item],
    dest: &str,
    folders: bool,
    number: bool,
) -> Vec<(String, String)> {
    let mut plan = Vec::new();
    let mut used: HashSet<String> = HashSet::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let biggest = items
        .iter()
        .filter_map(|i| i.n)
        .max()
        .unwrap_or(0)
        .max(items.len());
    let width = biggest.to_string().len().max(3);
    for (k, it) in items.iter().enumerate() {
        if !seen.insert(it.src.as_str()) {
            continue;
        }
        let n = it.n.unwrap_or(k + 1);
        let mut name = basename(&it.src).to_string();
        if number {
            name = format!("{n:0width$} - {name}");
        }
        let name = safe(&name);
        let mut dst = if folders && !it.rel.is_empty() && !paths::is_absolute(&it.rel) {
            let sub: Vec<String> = dirname(&it.rel)
                .split(['/', '\\'])
                .filter(|p| !p.is_empty())
                .map(safe)
                .collect();
            join(&join(dest, &sub.join(&SEP.to_string())), &name)
        } else {
            join(dest, &name)
        };
        let (base, ext) = {
            let (b, e) = splitext(&dst);
            (b.to_string(), e.to_string())
        };
        let mut k2 = 2;
        while used.contains(&dst) {
            dst = format!("{base} ({k2}){ext}");
            k2 += 1;
        }
        used.insert(dst.clone());
        plan.push((it.src.clone(), dst));
    }
    plan
}

fn unique(dst: &str) -> String {
    let (base, ext) = splitext(dst);
    let mut k = 2;
    loop {
        let d = format!("{base} ({k}){ext}");
        if !paths::exists(&d) {
            return d;
        }
        k += 1;
    }
}

/// -> (copied | skipped | error, final destination, message). Never overwrites.
pub fn copy_one(src: &str, dst: &str) -> (&'static str, String, String) {
    let go = || -> std::io::Result<(&'static str, String, String)> {
        std::fs::create_dir_all(dirname(dst))?;
        let mut dst = dst.to_string();
        if paths::exists(&dst) {
            if paths::getsize(&dst)? == paths::getsize(src)? {
                return Ok(("skipped", dst, "already there".into()));
            }
            dst = unique(&dst);
        }
        let mut fi = std::fs::File::open(src)?;
        // create_new: we can never clobber a file that appeared meanwhile
        let mut fo = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dst)?;
        std::io::copy(&mut fi, &mut fo)?;
        fo.flush()?;
        if let Ok(m) = fi.metadata().and_then(|m| m.modified()) {
            let _ = fo.set_modified(m); // e.g. FAT32 USB sticks may refuse; the music is copied fine
        }
        Ok(("copied", dst, String::new()))
    };
    go().unwrap_or_else(|e| ("error", dst.to_string(), e.to_string()))
}

// ---------------------------------------------------------------------------- USB sticks for CDJs / XDJs

/// What every Pioneer/AlphaTheta deck with a USB port plays (CDJ-900/2000, XDJ-1000 MK1 and newer).
/// FLAC/ALAC only arrived with the Nexus 2 generation; Opus/Ogg/WMA never. Hi-res (>48 kHz) isn't safe.
fn deck_safe(codec: &str, ext: &str) -> bool {
    match codec {
        "mp3" => ext == "mp3",
        "aac" => matches!(ext, "m4a" | "mp4" | "aac"),
        "pcm_s16le" | "pcm_s24le" => ext == "wav",
        "pcm_s16be" | "pcm_s24be" => matches!(ext, "aif" | "aiff"),
        _ => false,
    }
}

#[derive(Debug, Clone)]
pub struct Probe {
    pub codec: String,
    pub rate: u32,
    pub bits: u32,
}

fn find_tool(name: &str) -> Option<String> {
    let exe = if WINDOWS {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            let p = d.join(&exe);
            if p.is_file() {
                return Some(p.to_string_lossy().into_owned());
            }
        }
    }
    if WINDOWS {
        // where `winget install Gyan.FFmpeg` puts it
        let la = std::env::var("LOCALAPPDATA").unwrap_or_default();
        let link = join(
            &join(&join(&la, "Microsoft"), "WinGet"),
            &format!("Links\\{exe}"),
        );
        if paths::isfile(&link) {
            return Some(link);
        }
        let pkgs = join(&la, "Microsoft\\WinGet\\Packages");
        if let Ok(rd) = std::fs::read_dir(&pkgs) {
            for p in rd
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("Gyan.FFmpeg"))
            {
                if let Ok(inner) = std::fs::read_dir(p.path()) {
                    for i in inner.flatten() {
                        let cand = i.path().join("bin").join(&exe);
                        if cand.is_file() {
                            return Some(cand.to_string_lossy().into_owned());
                        }
                    }
                }
            }
        }
    }
    None
}

pub fn ffmpeg_path() -> (Option<String>, Option<String>) {
    (find_tool("ffmpeg"), find_tool("ffprobe"))
}

pub fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no console window per file
    }
    cmd
}

pub fn probe(src: &str) -> Option<Probe> {
    let ffprobe = ffmpeg_path().1?;
    let out = quiet(Command::new(ffprobe).args([
        "-v",
        "error",
        "-select_streams",
        "a:0",
        "-show_entries",
        "stream=codec_name,sample_rate,bits_per_raw_sample,bits_per_sample",
        "-of",
        "json",
    ]))
    .arg(format!("file:{}", longp(src)))
    .output()
    .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let s = v.get("streams")?.get(0)?;
    let num = |k: &str| {
        s.get(k).and_then(|x| {
            x.as_str()
                .and_then(|t| t.parse().ok())
                .or(x.as_u64().map(|n| n as u32))
        })
    };
    Some(Probe {
        codec: s
            .get("codec_name")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string(),
        rate: num("sample_rate").unwrap_or(0),
        bits: num("bits_per_raw_sample")
            .filter(|b| *b > 0)
            .or_else(|| num("bits_per_sample"))
            .unwrap_or(0),
    })
}

pub fn deck_playable(src: &str, info: &Probe) -> bool {
    let ext = splitext(src).1.to_lowercase();
    deck_safe(&info.codec, ext.trim_start_matches('.'))
        && matches!(info.rate, 44100 | 48000)
        && matches!(info.bits, 0 | 16 | 24)
}

/// deck-ready copy of src at dst (extension decided here). Never overwrites, never touches src.
pub fn convert_one(src: &str, dst: &str, fmt: &str) -> (&'static str, String, String) {
    let info = probe(src);
    if let Some(i) = &info {
        if deck_playable(src, i) {
            return copy_one(src, dst);
        }
    }
    let Some(ffmpeg) = ffmpeg_path().0 else {
        return (
            "error",
            dst.into(),
            "this file needs converting for the decks, but ffmpeg isn't installed".into(),
        );
    };
    let mut dst = format!(
        "{}{}",
        splitext(dst).0,
        if fmt == "mp3" { ".mp3" } else { ".aiff" }
    );
    if let Err(e) = std::fs::create_dir_all(dirname(&dst)) {
        return ("error", dst, e.to_string());
    }
    if paths::exists(&dst) {
        dst = unique(&dst);
    }
    let rate = info
        .as_ref()
        .map(|i| i.rate)
        .filter(|r| *r > 0)
        .unwrap_or(44100);
    let rate = if matches!(rate, 44100 | 48000) {
        rate
    } else if rate % 48000 == 0 {
        48000
    } else {
        44100
    };
    let codec: Vec<&str> = if fmt == "mp3" {
        vec![
            "-c:a",
            "libmp3lame",
            "-b:a",
            "320k",
            "-id3v2_version",
            "3",
            "-f",
            "mp3",
        ]
    } else {
        let pcm = if info.as_ref().map(|i| i.bits).unwrap_or(16) > 16 {
            "pcm_s24be"
        } else {
            "pcm_s16be"
        };
        vec!["-c:a", pcm, "-write_id3v2", "1", "-f", "aiff"]
    };
    let part = format!("{dst}.part");
    let mut last_err = String::new();
    let mut ok = false;
    for art in [
        &[
            "-map",
            "0:v:0?",
            "-c:v",
            "copy",
            "-disposition:v",
            "attached_pic",
        ][..],
        &[][..],
    ] {
        // second try without cover art
        let out = quiet(Command::new(&ffmpeg).args(["-v", "error", "-nostdin", "-y", "-i"]))
            .arg(format!("file:{}", longp(src)))
            .args(["-map", "0:a:0"])
            .args(art)
            .args(["-ar", &rate.to_string(), "-ac", "2", "-map_metadata", "0"])
            .args(&codec)
            .arg(format!("file:{}", longp(&part)))
            .output();
        match out {
            Ok(o) if o.status.success() => {
                ok = true;
                break;
            }
            Ok(o) => {
                last_err = String::from_utf8_lossy(&o.stderr)
                    .trim()
                    .chars()
                    .rev()
                    .take(300)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect()
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    if !ok {
        let _ = std::fs::remove_file(&part);
        return ("error", dst, format!("conversion failed: {last_err}"));
    }
    if paths::exists(&dst) {
        let _ = std::fs::remove_file(&part);
        return (
            "error",
            dst,
            "a file with that name appeared while converting".into(),
        );
    }
    match std::fs::rename(&part, &dst) {
        Ok(()) => {
            let from = info.map(|i| i.codec).unwrap_or_else(|| "?".into());
            ("converted", dst, format!("{from} → {}", fmt.to_uppercase()))
        }
        Err(e) => ("error", dst, e.to_string()),
    }
}

pub fn usb_folder(dest: &str, name: &str) -> String {
    let n = safe(name.trim());
    let n = n.trim_matches(|c| c == '.' || c == ' ');
    let n: String = if n.is_empty() {
        "dubplate".into()
    } else {
        n.chars().take(80).collect()
    };
    join(dest, &n)
}

/// bytes needed; FLAC -> AIFF is roughly 1.7x, lossy -> AIFF a lot more
pub fn estimate_size(plan: &[(String, String)], usb_format: Option<&str>) -> u64 {
    plan.iter()
        .map(|(s, _)| {
            let size = paths::getsize(s).unwrap_or(0) as f64;
            let ext = splitext(s).1.to_lowercase();
            let f = match (usb_format, ext.as_str()) {
                (Some("aiff"), ".flac" | ".alac" | ".ape" | ".wv") => 1.8,
                (Some("aiff"), ".opus" | ".ogg" | ".wma") => 10.0,
                _ => 1.0,
            };
            (size * f) as u64
        })
        .sum()
}

// ---------------------------------------------------------------------------- remembering what was copied

/// What dubplate copied to each destination, so a re-run of the same list (after you've added the
/// missing songs to your library) only offers what's still to do. Kept in dubplate's own folder -
/// nothing extra is written into the destination. A song counts as copied only while its copy exists.
/// Same file as the Python version: copied.json = {destination: {source: copy}}.
pub struct Ledger;

static LEDGER_LOCK: Mutex<()> = Mutex::new(());

impl Ledger {
    fn file() -> String {
        join(&paths::app_dir(), "copied.json")
    }

    fn key(dest: &str) -> String {
        normcase(&paths::abspath(&paths::expanduser(dest)))
    }

    fn load() -> HashMap<String, HashMap<String, String>> {
        std::fs::read_to_string(Self::file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn record(dest: &str, pairs: &[(String, String)]) {
        if pairs.is_empty() {
            return;
        }
        let _g = LEDGER_LOCK.lock().unwrap();
        let mut d = Self::load();
        let m = d.entry(Self::key(dest)).or_default();
        for (s, t) in pairs {
            m.insert(s.clone(), t.clone());
        }
        if let Ok(j) = serde_json::to_vec(&d) {
            let _ = paths::write_atomic(&Self::file(), &j);
        }
    }

    /// -> {source: copy} for the sources already copied to dest (and still there)
    pub fn copied(dest: &str, srcs: &[String]) -> HashMap<String, String> {
        let mut out = HashMap::new();
        if dest.trim().is_empty() {
            return out;
        }
        let dest = paths::abspath(&paths::expanduser(dest));
        let all = Self::load();
        let m = all.get(&Self::key(&dest));
        for s in srcs {
            if let Some(d) = m.and_then(|m| m.get(s)) {
                if paths::exists(d) {
                    out.insert(s.clone(), d.clone());
                    continue;
                }
            }
            // copied before dubplate kept a record (or by hand): same name + size right in dest
            let d = join(&dest, &safe(basename(s)));
            if paths::exists(&d) && paths::getsize(&d).ok() == paths::getsize(s).ok() {
                out.insert(s.clone(), d);
            }
        }
        out
    }
}

#[derive(Default, Serialize, Clone)]
pub struct Transfer {
    pub copied: usize,
    pub converted: usize,
    pub skipped: usize,
    pub errors: Vec<CopyError>,
    pub done: Vec<String>,
    #[serde(skip)]
    pub pairs: Vec<(String, String)>,
}

#[derive(Serialize, Clone)]
pub struct CopyError {
    pub src: String,
    pub msg: String,
}

/// copy (or for a USB, copy/convert) every (src, dst). Saves to the memory as it goes,
/// so an interrupted copy still counts.
pub fn transfer(
    plan: &[(String, String)],
    dest_key: &str,
    usb_format: Option<&str>,
    mut on_progress: impl FnMut(usize, &str),
) -> Transfer {
    let mut r = Transfer::default();
    let mut saved = 0;
    for (k, (s, d)) in plan.iter().enumerate() {
        on_progress(k + 1, s);
        let (st, fin, msg) = match usb_format {
            Some(f) => convert_one(s, d, f),
            None => copy_one(s, d),
        };
        match st {
            "error" => {
                r.errors.push(CopyError {
                    src: s.clone(),
                    msg,
                });
                continue;
            }
            "copied" => r.copied += 1,
            "converted" => r.converted += 1,
            _ => r.skipped += 1,
        }
        r.done.push(fin.clone());
        r.pairs.push((s.clone(), fin));
        if r.pairs.len() - saved >= 20 {
            Ledger::record(dest_key, &r.pairs[saved..]);
            saved = r.pairs.len();
        }
    }
    Ledger::record(dest_key, &r.pairs[saved..]);
    r
}

pub fn write_playlist(dest: &str, done: &[String]) -> std::io::Result<String> {
    let mut path = join(dest, "playlist.m3u8");
    let mut k = 2;
    while paths::exists(&path) {
        path = join(dest, &format!("playlist ({k}).m3u8"));
        k += 1;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    writeln!(f, "#EXTM3U")?;
    for d in done {
        writeln!(f, "{}", paths::relpath(d, dest))?;
    }
    Ok(path)
}

pub fn write_missing_csv(path: &str, results: &[MatchResult]) -> std::io::Result<()> {
    let mut w = csv::WriterBuilder::new()
        .terminator(csv::Terminator::CRLF)
        .from_path(path)?;
    w.write_record(["Row", "Song", "Problem", "Closest thing in your library"])?;
    for r in results {
        let row = match &r.row {
            serde_json::Value::Null => String::new(),
            serde_json::Value::String(s) => s.clone(),
            v => v.to_string(),
        };
        match r.status.as_str() {
            "missing" => {
                let s = r
                    .suggestions
                    .first()
                    .map(|s| s.rel.clone())
                    .unwrap_or_default();
                w.write_record([row.as_str(), &r.label, "not found in library", &s])?;
            }
            "version" => {
                let s = r.file.as_ref().map(|f| f.rel.clone()).unwrap_or_default();
                w.write_record([
                    row.as_str(),
                    &r.label,
                    "only a different version in library",
                    &s,
                ])?;
            }
            _ => {}
        }
    }
    w.flush()
}
