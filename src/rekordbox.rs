//! Rekordbox USB sticks: playlists in the deck's library browser, with beat grids, BPM, key and
//! waveforms - no rekordbox needed. Built on rbxport's engine (https://github.com/chrisle/rbxport,
//! GPL-2.0-or-later): rbl-analysis analyses each song, rbl-anlz writes the deck's analysis files,
//! rbl-export writes export.pdb / exportLibrary.db and converts audio older decks can't play.
//! `analyse_file` is adapted from rbxport's `src-tauri/src/analysis.rs` (v1.10.0).

use crate::paths::{self, join};
use rbl_export::{CompatibilityFormat, ExportOptions, SourcePlaylist, SourceTrack};
use serde::Serialize;
use std::path::Path;

/// bump when the analysis or how it's stored changes: old cached analysis is then redone
const CACHE_VERSION: &str = "rbl-1.10.0-a1";
/// analyse at most this much of a song (as rbxport does); longer files get the rest of their
/// waveform drawn but the tempo/key from the first half hour
const DECODE_CAP_SECS: f64 = 1800.0;

/// a song to put on the stick: the file, plus what dubplate already knows (used when the
/// file has no tags)
#[derive(Clone, Debug)]
pub struct Song {
    pub path: String,
    pub artist: String,
    pub title: String,
    pub album: String,
}

#[derive(Default)]
struct Tags {
    title: String,
    artist: String,
    album: String,
    genre: String,
    label: String,
    key: String,
    comment: String,
    year: u16,
    track: u32,
    bitrate: u32,
    sample_rate: u32,
    bit_depth: u16,
}

fn read_tags(path: &str) -> Tags {
    use lofty::prelude::*;
    let Ok(f) = lofty::read_from_path(path) else { return Tags::default() };
    let p = f.properties();
    let mut t = Tags {
        bitrate: p.audio_bitrate().unwrap_or(0),
        sample_rate: p.sample_rate().unwrap_or(0),
        bit_depth: u16::from(p.bit_depth().unwrap_or(0)),
        ..Default::default()
    };
    if let Some(tag) = f.primary_tag().or_else(|| f.first_tag()) {
        let s = |v: Option<std::borrow::Cow<'_, str>>| v.map(|x| x.trim().to_string()).unwrap_or_default();
        t.title = s(tag.title());
        t.artist = s(tag.artist());
        t.album = s(tag.album());
        t.genre = s(tag.genre());
        t.comment = s(tag.comment());
        t.label = tag.get_string(ItemKey::Label).or_else(|| tag.get_string(ItemKey::Publisher)).unwrap_or("").trim().to_string();
        t.key = tag.get_string(ItemKey::InitialKey).unwrap_or("").trim().to_string();
        t.track = tag.track().unwrap_or(0);
        t.year = tag.date().map(|d| d.year).unwrap_or(0);
    }
    t
}

/// decode the file and analyse it the way rekordbox's Normal analysis does (one tempo fitted to
/// the whole song): beat grid, BPM, key, and every waveform the decks draw
fn analyse_file(path: &Path) -> Result<(rbl_analysis::Analysis, rbl_anlz::RekordboxWaves, f64), String> {
    let options = rbl_analysis::AnalysisPreset::Rekordbox.options();
    let mut stream = rbl_audio::MonoStream::open(path).map_err(|e| format!("can't read the audio ({e})"))?;
    let rate = stream.sample_rate();
    let mut creator = rbl_analysis::wave_creator::WaveCreator::new(rate);
    if let Some(frames) = stream.expected_frames() {
        creator.reserve(frames);
    }
    let cap = (DECODE_CAP_SECS * f64::from(rate)) as usize;
    let mut head: Vec<f32> = Vec::with_capacity(cap.min(1 << 24));
    let mut builder: Option<rbl_analysis::waveform::Builder> = None; // only for songs past the cap
    let mut past_head = false;
    while let Some((chunk, frames)) = stream.next_frames() {
        creator.push(frames);
        if past_head {
            if let Some(b) = builder.as_mut() {
                b.push(chunk);
            }
            continue;
        }
        head.extend_from_slice(chunk);
        if head.len() > cap {
            let mut b = rbl_analysis::waveform::Builder::new(rate);
            b.push(&head);
            builder = Some(b);
            head.truncate(cap);
            head.shrink_to_fit();
            past_head = true;
        }
    }
    if head.is_empty() {
        return Err("the file has no audio in it".into());
    }
    let mut analysis = rbl_analysis::analyse_with(&head, rate, options);
    let w = creator.finish();
    let waves = rbl_anlz::RekordboxWaves {
        pwav: w.pwav,
        pwv2: w.pwv2,
        pwvc: w.pwvc,
        pwv3: w.pwv3,
        pwv4: w.pwv4,
        pwv5: w.pwv5,
        pwv6: w.pwv6,
        pwv7: w.pwv7,
    };
    let mut duration = head.len() as f64 / f64::from(rate.max(1));
    if let Some(b) = builder {
        duration = b.samples() as f64 / f64::from(rate.max(1));
        analysis.peak = analysis.peak.max(b.peak());
        analysis.tempo.extend_to(duration);
        analysis.waveform = b.finish();
    }
    Ok((analysis, waves, duration))
}

/// where the analysis of this exact file (path, size, date) is kept
fn cache_key(path: &str) -> Option<String> {
    let m = std::fs::metadata(path).ok()?;
    let mtime = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    let h = sha1_smol::Sha1::from(format!("{CACHE_VERSION}\n{path}\n{}\n{mtime}", m.len())).digest().to_string();
    Some(join(&join(&paths::app_dir(), "analysis"), &h[..2]).to_string() + &format!("{}{}", paths::SEP, &h[2..]))
}

fn load_cached(base: &str) -> Option<SourceTrack> {
    let mut t: SourceTrack = serde_json::from_slice(&std::fs::read(format!("{base}.json")).ok()?).ok()?;
    for ext in ["DAT", "EXT", "2EX"] {
        if let Ok(b) = std::fs::read(format!("{base}.{ext}")) {
            t.analysis.push((ext.to_string(), b));
        }
    }
    (t.analysis.len() == 3).then_some(t)
}

fn save_cached(base: &str, t: &SourceTrack) {
    let mut meta = t.clone();
    meta.analysis.clear(); // the analysis files are stored as they are, next to it
    for (ext, bytes) in &t.analysis {
        let _ = paths::write_atomic(&format!("{base}.{ext}"), bytes);
    }
    if let Ok(j) = serde_json::to_vec(&meta) {
        let _ = paths::write_atomic(&format!("{base}.json"), &j);
    }
}

/// what goes on the stick for one song: its details and its analysis files. Analysed once,
/// then taken from dubplate's saved analysis until the file changes.
pub fn prepare(song: &Song) -> Result<SourceTrack, String> {
    let key = cache_key(&song.path);
    if let Some(t) = key.as_deref().and_then(load_cached) {
        return Ok(t);
    }
    let path = Path::new(&song.path);
    let tags = read_tags(&song.path);
    let (analysis, waves, duration) = analyse_file(path)?;
    let beats: Vec<rbl_anlz::Beat> = analysis
        .tempo
        .beats
        .iter()
        .map(|b| rbl_anlz::Beat { beat_number: b.beat_number, tempo_x100: b.tempo_x100, time_ms: b.time_ms })
        .collect();
    let files = rbl_anlz::author_with_rekordbox_waves(
        "/pending", // rbl-export fills in the real path on the stick
        &beats,
        &waves,
        rbl_anlz::Existing { dat: None, ext: None, two_ex: None },
    );
    let pick = |tag: &str, fallback: &str| if tag.is_empty() { fallback.to_string() } else { tag.to_string() };
    let t = SourceTrack {
        id: 0,
        source_path: path.to_path_buf(),
        title: pick(&tags.title, &song.title),
        artist: pick(&tags.artist, &song.artist),
        album: pick(&tags.album, &song.album),
        genre: tags.genre,
        label: tags.label,
        key: analysis.key.as_ref().map_or(tags.key, |k| k.name.clone()),
        comment: tags.comment,
        duration_sec: duration.round().clamp(0.0, f64::from(u16::MAX)) as u16,
        bpm_x100: (analysis.tempo.bpm * 100.0).round().clamp(0.0, f64::from(u32::MAX)) as u32,
        bitrate: tags.bitrate,
        sample_rate: tags.sample_rate,
        file_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        year: tags.year,
        metadata: rbl_core::ExportMetadata {
            track_number: tags.track,
            bit_depth: tags.bit_depth,
            ..rbl_core::ExportMetadata::default()
        },
        cues: Some(Vec::new()),
        analysis: vec![("DAT".into(), files.dat), ("EXT".into(), files.ext), ("2EX".into(), files.two_ex)],
        ..SourceTrack::default()
    };
    if let Some(k) = key {
        save_cached(&k, &t);
    }
    Ok(t)
}

/// how far an export has got, for the page / terminal
#[derive(Serialize, Clone, Default, Debug)]
pub struct Progress {
    /// analysing | checking | copying | database | verifying | publishing | done
    pub stage: String,
    pub done: usize,
    pub total: usize,
    pub current: String,
}

#[derive(Serialize, Clone, Default, Debug)]
pub struct Report {
    pub playlist: String,
    pub tracks: usize,
    pub copied: usize,
    pub converted: usize,
    pub reused: usize,
    pub verified: bool,
    pub backup: Option<String>,
    pub errors: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

/// the playlist's id in the stick's record of where it came from: the same name each time, so
/// sending "Friday" again replaces Friday and sending "Saturday" adds a playlist next to it
fn playlist_id(name: &str) -> u64 {
    let d = sha1_smol::Sha1::from(name.to_lowercase()).digest().bytes();
    (u64::from_le_bytes(d[..8].try_into().unwrap()) & 0x7fff_ffff) | 1 // small and never 0
}

pub fn parse_convert(s: &str) -> Option<CompatibilityFormat> {
    match s {
        "aiff" => Some(CompatibilityFormat::Aiff),
        "mp3" => Some(CompatibilityFormat::Mp3),
        "wav" => Some(CompatibilityFormat::Wav),
        _ => None, // "none": the decks play FLAC (CDJ-2000NXS2, CDJ-3000 and newer)
    }
}

/// Write `songs` to the stick at `stick` as the playlist `name` (replacing a playlist of that
/// name; other playlists on the stick stay). Songs that can't be read are left out and listed.
pub fn export_set(
    stick: &str,
    name: &str,
    songs: &[Song],
    convert: Option<CompatibilityFormat>,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<Report, String> {
    if !paths::isdir(stick) {
        return Err(format!("{stick} isn't there (is the stick plugged in?)"));
    }
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the playlist a name.".into());
    }
    recover(stick);
    // a song listed twice goes in once (the stick's library holds each file once)
    let mut seen = std::collections::HashSet::new();
    let songs: Vec<&Song> = songs.iter().filter(|s| seen.insert(paths::normcase(&s.path))).collect();
    // analyse (or take the saved analysis), a few songs at a time
    let total = songs.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: std::sync::Mutex<Vec<Option<Result<SourceTrack, String>>>> = std::sync::Mutex::new(vec![None; total]);
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 4);
    progress(Progress { stage: "analysing".into(), done: 0, total, current: String::new() });
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if i >= total {
                    break;
                }
                // a file that trips up the decoder leaves out that song, not the whole set
                let r = std::panic::catch_unwind(|| prepare(songs[i]))
                    .unwrap_or_else(|_| Err("dubplate couldn't analyse this file (it may be damaged)".into()));
                results.lock().unwrap_or_else(std::sync::PoisonError::into_inner)[i] = Some(r);
                let d = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                progress(Progress { stage: "analysing".into(), done: d, total, current: paths::basename(&songs[i].path).to_string() });
            });
        }
    });
    let mut tracks = Vec::new();
    let mut errors = Vec::new();
    for (song, r) in songs.iter().zip(results.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner)) {
        match r {
            Some(Ok(t)) => tracks.push(t),
            Some(Err(e)) => errors.push((song.path.clone(), e)),
            None => errors.push((song.path.clone(), "not analysed".into())),
        }
    }
    if tracks.is_empty() {
        return Err("None of the songs could be read, so the stick wasn't changed.".into());
    }
    let n = tracks.len();
    let playlist = SourcePlaylist { id: playlist_id(name), name: name.to_string(), track_indices: (0..n).collect(), ..Default::default() };
    // add to what the stick holds (other playlists stay), as rekordbox's "Export Playlist" does
    let options = ExportOptions { compatibility: convert, keep_unlisted: true, ..Default::default() };
    // the stick's library as it is now, kept until we know the export changed it
    let backup = backup(stick, &[])?;
    let before = count_files(&Path::new(stick).join("Contents"));
    let result = rbl_export::export_with_options(Path::new(stick), &tracks, &[playlist], &[], &options, &mut |p| {
        let (done, total) = if p.total > 0 { (p.done, p.total) } else { (p.tracks_done, p.tracks_total) };
        progress(Progress { stage: p.stage.to_string(), done, total, current: p.title.clone() })
    });
    let report = match result {
        Ok(r) => r,
        Err(e) => {
            if let Some(b) = &backup {
                discard_backup(stick, b); // rbxport changes nothing when an export fails
            }
            return Err(format!("couldn't write the stick: {e}"));
        }
    };
    let backup = match backup {
        Some(b) if report.up_to_date => {
            discard_backup(stick, &b);
            None
        }
        b => b,
    };
    progress(Progress { stage: "done".into(), done: n, total: n, current: String::new() });
    let left_out: Vec<(String, String)> =
        report.skipped.iter().chain(&report.failed).map(|f| (String::new(), f.clone())).collect();
    // rbxport counts the whole stick; for this playlist: the songs newly on the stick, and the
    // rest that were there already
    let tracks = n.saturating_sub(left_out.len());
    let copied = count_files(&Path::new(stick).join("Contents")).saturating_sub(before).min(tracks);
    Ok(Report {
        playlist: name.to_string(),
        tracks,
        copied,
        converted: report.converted.min(copied),
        reused: tracks - copied,
        verified: report.verified,
        backup,
        errors: errors.into_iter().chain(left_out).collect(),
        warnings: report
            .warnings
            .iter()
            .map(|w| {
                let why = match w.reason.as_str() {
                    "analysis-stale" => "its analysis on the stick is from an older version of the file",
                    r => r,
                };
                format!("{}: {why}{}", w.title, w.detail.as_deref().map(|d| format!(" ({d})")).unwrap_or_default())
            })
            .collect(),
    })
}

/// files under `dir`, all the way down
fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| if e.file_type().is_ok_and(|t| t.is_dir()) { count_files(&e.path()) } else { 1 })
                .sum()
        })
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------- backups

/// where dubplate keeps what you rely on, not a cache: %LOCALAPPDATA%\dubplate on Windows,
/// ~/.local/share/dubplate (XDG_DATA_HOME) elsewhere
fn data_dir() -> String {
    if paths::WINDOWS || std::env::var("LOCALAPPDATA").is_ok_and(|v| !v.is_empty()) {
        return paths::app_dir();
    }
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|v| v.starts_with('/'))
        .unwrap_or_else(|| join(&join(&paths::home(), ".local"), "share"));
    join(&base, "dubplate")
}

/// the stick's own name for dubplate, kept on the stick (PIONEER/dubplate/stick-id), so its
/// backups follow the stick, not the drive letter or mount point it happens to have today.
/// Made on first need when `make`.
fn stick_id(stick: &str, make: bool) -> Result<Option<String>, String> {
    let f = rbl_export::export_root(Path::new(stick)).join("dubplate").join("stick-id");
    if let Ok(s) = std::fs::read_to_string(&f) {
        let id = s.trim();
        if id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(Some(id.to_ascii_lowercase()));
        }
    }
    if !make {
        return Ok(None);
    }
    let seed = format!("{stick}\n{}\n{:?}", std::process::id(), std::time::SystemTime::now());
    let id = sha1_smol::Sha1::from(seed).digest().to_string()[..16].to_string();
    let fail = |e: std::io::Error| format!("couldn't write to the stick ({e}) - nothing was changed");
    std::fs::create_dir_all(f.parent().unwrap()).map_err(fail)?;
    std::fs::write(&f, format!("{id}\n")).map_err(fail)?;
    Ok(Some(id))
}

fn backups_base(id: &str) -> String {
    join(&join(&data_dir(), "stick-backups"), id)
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<u64> {
    std::fs::create_dir_all(to)?;
    let mut n = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        if e.file_type()?.is_dir() {
            n += copy_tree(&p, &to.join(e.file_name()))?;
        } else {
            n += std::fs::copy(&p, to.join(e.file_name()))?;
        }
    }
    Ok(n)
}

/// the folders on a stick that hold its library (not the music): rekordbox's databases, and
/// rbxport's record of what it put there
const LIBRARY_DIRS: [&str; 2] = ["rekordbox", "rbxport"];
/// backups kept per stick: the first one dubplate made (the library from before dubplate ever
/// touched the stick) and the newest ones
const KEEP_BACKUPS: usize = 10;

/// Keep a copy of the stick's library (its databases, not the music), so an export can be
/// undone. Nothing to keep on a stick without one. `keep` are backups pruning must not remove.
pub fn backup(stick: &str, keep: &[&str]) -> Result<Option<String>, String> {
    let root = rbl_export::export_root(Path::new(stick));
    if !root.join("rekordbox").is_dir() {
        return Ok(None);
    }
    let Some(id) = stick_id(stick, true)? else { return Ok(None) };
    let base = backups_base(&id);
    let mut when = chrono::Local::now().format("%Y-%m-%d %H.%M.%S").to_string();
    let stamp = when.clone();
    let mut n = 2;
    while paths::exists(&join(&base, &when)) {
        when = format!("{stamp} ({n})");
        n += 1;
    }
    // copied in under a temporary name and renamed when complete, so a half-made backup (dubplate
    // stopped, the stick pulled) is never offered for restoring
    let part = join(&base, &format!(".partial-{when}"));
    let made = (|| -> std::io::Result<()> {
        let _ = std::fs::remove_dir_all(&part);
        for d in LIBRARY_DIRS {
            if root.join(d).is_dir() {
                copy_tree(&root.join(d), &Path::new(&part).join(d))?;
            }
        }
        std::fs::write(Path::new(&part).join("stick.txt"), format!("{stick}\n"))?;
        std::fs::rename(&part, join(&base, &when))
    })();
    if let Err(e) = made {
        let _ = std::fs::remove_dir_all(&part);
        return Err(format!("couldn't back up the stick's library first ({e}) - nothing was changed"));
    }
    let mut keep: Vec<&str> = keep.to_vec();
    keep.push(&when);
    prune(&base, &keep);
    Ok(Some(when))
}

/// keep the first backup, the newest ones and `keep`; remove the rest (and old half-made ones)
fn prune(base: &str, keep: &[&str]) {
    let all = backups_in(base); // newest first
    let first = all.last().cloned();
    let mut kept = 0;
    for b in &all {
        if keep.contains(&b.as_str()) || Some(b) == first.as_ref() {
            continue;
        }
        kept += 1;
        if kept > KEEP_BACKUPS - 2 {
            let _ = std::fs::remove_dir_all(join(base, b));
        }
    }
    if let Ok(rd) = std::fs::read_dir(base) {
        for e in rd.flatten() {
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|d| d.as_secs() > 86400);
            if e.file_name().to_string_lossy().starts_with(".partial-") && old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

fn discard_backup(stick: &str, which: &str) {
    if let Ok(Some(id)) = stick_id(stick, false) {
        if valid_name(which) {
            let _ = std::fs::remove_dir_all(join(&backups_base(&id), which));
        }
    }
}

fn backups_in(base: &str) -> Vec<String> {
    let mut v: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(base)
        .map(|rd| {
            rd.flatten()
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.') && e.path().join("rekordbox").is_dir())
                .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), e.file_name().to_string_lossy().into_owned()))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v.into_iter().rev().map(|(_, n)| n).collect()
}

/// the stick's backups, newest first
pub fn list_backups(stick: &str) -> Vec<String> {
    match stick_id(stick, false) {
        Ok(Some(id)) => backups_in(&backups_base(&id)),
        _ => Vec::new(),
    }
}

fn valid_name(which: &str) -> bool {
    !which.is_empty() && !which.starts_with('.') && !which.contains(['/', '\\', ':']) && !which.contains("..")
}

#[derive(Serialize, Clone, Default, Debug)]
pub struct Restored {
    /// the backup of what the stick held just before
    pub backup: Option<String>,
    /// songs the restored playlists list that a later export took off the stick
    pub missing: Vec<String>,
}

/// A restore that was cut off (dubplate stopped, the stick pulled) in the middle of swapping
/// the library: put the library it was replacing back where it was. Once the swap had finished
/// the restore stands, and only the leftovers are cleared away.
fn recover(stick: &str) {
    let root = rbl_export::export_root(Path::new(stick));
    let work = root.join(".dubplate-restore");
    if !work.is_dir() {
        return;
    }
    if !work.join("swapped").exists() {
        for d in LIBRARY_DIRS {
            let old = work.join(format!("old-{d}"));
            if old.is_dir() {
                if root.join(d).is_dir() {
                    // the backup's copy, half put in place
                    if std::fs::rename(root.join(d), work.join(format!("half-{d}"))).is_err() {
                        continue;
                    }
                }
                let _ = std::fs::rename(&old, root.join(d));
            }
        }
    }
    if LIBRARY_DIRS.iter().all(|d| !work.join(format!("old-{d}")).exists()) || work.join("swapped").exists() {
        let _ = std::fs::remove_dir_all(&work);
    }
}

/// Put a backup of the stick's library back (what it holds now is backed up first). The music
/// on the stick is left alone. The backup is copied onto the stick first and swapped in with
/// renames, so a stick pulled out midway keeps a whole library.
pub fn restore(stick: &str, which: &str) -> Result<Restored, String> {
    if !valid_name(which) {
        return Err("no such backup".into());
    }
    if !paths::isdir(stick) {
        return Err(format!("{stick} isn't there (is the stick plugged in?)"));
    }
    let Some(id) = stick_id(stick, false)? else {
        return Err("dubplate has no backups of this stick (is it the right stick?)".into());
    };
    let from = Path::new(&backups_base(&id)).join(which);
    if !from.join("rekordbox").is_dir() {
        return Err(format!("this stick has no backup called {which}"));
    }
    recover(stick);
    let before = backup(stick, &[which])?;
    let root = rbl_export::export_root(Path::new(stick));
    let work = root.join(".dubplate-restore");
    let staged = |d: &str| work.join(format!("new-{d}"));
    let old = |d: &str| work.join(format!("old-{d}"));
    let fail = |e: std::io::Error| {
        let _ = std::fs::remove_dir_all(&work);
        format!("couldn't put the backup back ({e}); the stick wasn't changed")
    };
    // 1. copy the backup onto the stick, beside the library
    let _ = std::fs::remove_dir_all(&work);
    for d in LIBRARY_DIRS {
        if from.join(d).is_dir() {
            copy_tree(&from.join(d), &staged(d)).map_err(fail)?;
        }
    }
    // 2. swap: the library out of the way, the backup in (undone if a rename fails)
    let mut done: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
    let mut swap = || -> std::io::Result<()> {
        for d in LIBRARY_DIRS {
            if root.join(d).is_dir() {
                std::fs::rename(root.join(d), old(d))?;
                done.push((old(d), root.join(d)));
            }
            if staged(d).is_dir() {
                std::fs::rename(staged(d), root.join(d))?;
                done.push((root.join(d), staged(d)));
            }
        }
        Ok(())
    };
    if let Err(e) = swap() {
        for (now, was) in done.iter().rev() {
            let _ = std::fs::rename(now, was);
        }
        return Err(fail(e));
    }
    let _ = std::fs::write(work.join("swapped"), b""); // from here on, the restore stands
    let _ = std::fs::remove_dir_all(&work);
    // songs a later export removed from the stick can't come back from a backup of the library
    let missing = rbl_export::verify(Path::new(stick)).map(|v| v.missing_audio).unwrap_or_default();
    Ok(Restored { backup: before, missing })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a click track: a short burst every beat at `bpm`, as a WAV file
    pub fn click_wav(path: &str, bpm: f64, seconds: u32) {
        let rate = 44100u32;
        let n = rate * seconds;
        let beat = (60.0 / bpm * f64::from(rate)) as u32;
        let mut data = Vec::with_capacity(n as usize * 2);
        for i in 0..n {
            let s: i16 = if i % beat < 400 { if (i / 20) % 2 == 0 { 20000 } else { -20000 } } else { 0 };
            data.extend_from_slice(&s.to_le_bytes());
        }
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&1u16.to_le_bytes()); // mono
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * 2).to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(data.len() as u32).to_le_bytes());
        w.extend_from_slice(&data);
        std::fs::write(path, w).unwrap();
    }

    fn walk_files(d: &Path) -> Vec<std::path::PathBuf> {
        let mut v = Vec::new();
        for e in std::fs::read_dir(d).unwrap().flatten() {
            if e.path().is_dir() { v.extend(walk_files(&e.path())) } else { v.push(e.path()) }
        }
        v
    }

    fn setup(tmp: &Path) -> Vec<Song> {
        std::env::set_var("LOCALAPPDATA", tmp.join("app"));
        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        [(120.0, "First"), (128.0, "Second")]
            .iter()
            .map(|(bpm, t)| {
                let p = src.join(format!("Artist - {t}.wav")).to_string_lossy().into_owned();
                click_wav(&p, *bpm, 20);
                Song { path: p, artist: "Artist".into(), title: t.to_string(), album: String::new() }
            })
            .collect()
    }

    #[test]
    fn a_rekordbox_stick_from_loose_files() {
        let _g = crate::paths::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let songs = setup(tmp.path());
        let stick = tmp.path().join("stick");
        std::fs::create_dir_all(&stick).unwrap();
        let st = stick.to_string_lossy().into_owned();
        // a song listed twice goes in once instead of failing the export
        let twice = vec![songs[0].clone(), songs[1].clone(), songs[0].clone()];
        let r = export_set(&st, "Friday", &twice, None, &|_| {}).unwrap();
        assert_eq!((r.tracks, r.copied, r.reused), (2, 2, 0), "{r:?}");
        assert!(r.errors.is_empty(), "{r:?}");
        assert!(r.backup.is_none()); // nothing was on the stick yet
        assert!(stick.join("PIONEER/rekordbox/export.pdb").is_file());
        let v = rbl_export::verify(&stick).unwrap();
        assert!(v.is_ok() && v.tracks == 2 && v.playlists == 1 && v.analysis_present == 2, "{v:?}");
        // the analysis found the tempo
        let t = prepare(&songs[1]).unwrap();
        assert!((t.bpm_x100 as i32 - 12800).abs() <= 100, "bpm {}", t.bpm_x100);
        // a second set goes next to the first; the stick is backed up first
        let r2 = export_set(&st, "Saturday", &songs[..1], None, &|_| {}).unwrap();
        assert!(r2.backup.is_some());
        let files: Vec<_> = walk_files(&stick.join("Contents"));
        assert_eq!(files.len(), 2, "{files:?}");
        assert_eq!((r2.tracks, r2.copied, r2.reused), (1, 0, 1), "{r2:?}");
        let v = rbl_export::verify(&stick).unwrap();
        assert_eq!(v.playlists, 2, "{v:?}");
        assert_eq!(list_backups(&st).len(), 1);
        // the same set again changes nothing, and uses up no backup
        let r3 = export_set(&st, "Saturday", &songs[..1], None, &|_| {}).unwrap();
        assert!(r3.backup.is_none(), "{r3:?}");
        assert_eq!(list_backups(&st).len(), 1);
        // sending Friday again (any case) replaces it rather than adding a third
        export_set(&st, "friday", &songs[1..], None, &|_| {}).unwrap();
        let v = rbl_export::verify(&stick).unwrap();
        assert!(v.is_ok() && v.playlists == 2, "{v:?}");
        let all = list_backups(&st);
        assert_eq!(all.len(), 2);
        let first = all.last().unwrap().clone(); // oldest: before Saturday
        // the backups follow the stick, not where it's plugged in
        let moved = tmp.path().join("stick-on-another-day");
        std::fs::rename(&stick, &moved).unwrap();
        assert!(list_backups(&st).is_empty());
        assert!(restore(&st, &first).is_err()); // not plugged in: an error, nothing written
        assert!(!stick.exists());
        let mv = moved.to_string_lossy().into_owned();
        assert_eq!(list_backups(&mv), all);
        // the backup can be put back
        let back = restore(&mv, &first).unwrap();
        assert!(back.backup.is_some());
        let v = rbl_export::verify(&moved).unwrap();
        assert_eq!(v.playlists, 1);
        assert!(back.missing.is_empty(), "{back:?}"); // First stayed: Saturday holds it
        assert!(rbl_export::verify(&moved).unwrap().is_ok());
        assert!(!moved.join("PIONEER/.dubplate-restore").exists());
        assert!(restore(&mv, "../x").is_err() && restore(&mv, "nope").is_err() && restore(&mv, ".partial-x").is_err());
        // and the stick takes more sets after a restore
        export_set(&mv, "Sunday", &songs, None, &|_| {}).unwrap();
        let v = rbl_export::verify(&moved).unwrap();
        assert!(v.is_ok() && v.playlists == 2 && v.tracks == 2, "{v:?}");
    }

    #[test]
    fn backups_keep_the_first_and_never_lose_the_one_being_restored() {
        let _g = crate::paths::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let songs = setup(tmp.path());
        let stick = tmp.path().join("stick");
        std::fs::create_dir_all(&stick).unwrap();
        let st = stick.to_string_lossy().into_owned();
        export_set(&st, "One", &songs[..1], None, &|_| {}).unwrap();
        let mut made = Vec::new();
        for _ in 0..14 {
            made.push(backup(&st, &[]).unwrap().unwrap());
        }
        let all = list_backups(&st);
        assert_eq!(all.len(), KEEP_BACKUPS);
        assert_eq!(all.last(), made.first(), "the first backup is kept");
        assert_eq!(all[0], *made.last().unwrap());
        // restoring the oldest while the list is full: the backup made first doesn't push it out
        let oldest = all.last().unwrap().clone();
        restore(&st, &oldest).unwrap();
        assert!(list_backups(&st).contains(&oldest));
        assert!(stick.join("PIONEER/rekordbox/export.pdb").is_file());
        // a song that can't be read is left out and listed; the rest go on
        let bad = tmp.path().join("src/Broken - Song.wav");
        std::fs::write(&bad, b"RIFF....WAVEfmt nonsense").unwrap();
        let mut set = songs.clone();
        set.push(Song { path: bad.to_string_lossy().into_owned(), artist: "Broken".into(), title: "Song".into(), album: String::new() });
        let r = export_set(&st, "Two", &set, None, &|_| {}).unwrap();
        assert_eq!(r.tracks, 2);
        assert_eq!(r.errors.len(), 1, "{r:?}");
        // a restore cut off between its renames: the next export puts the old library back first
        let pioneer = stick.join("PIONEER");
        let work = pioneer.join(".dubplate-restore");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::rename(pioneer.join("rekordbox"), work.join("old-rekordbox")).unwrap();
        copy_tree(&work.join("old-rekordbox"), &work.join("new-rbxport")).unwrap(); // a staged leftover
        recover(&st);
        assert!(pioneer.join("rekordbox/export.pdb").is_file() && !work.exists());
        assert!(rbl_export::verify(&stick).unwrap().is_ok());
        // ...and one cut off after both swaps were done stands
        std::fs::create_dir_all(&work).unwrap();
        copy_tree(&pioneer.join("rekordbox"), &work.join("old-rekordbox")).unwrap();
        std::fs::write(pioneer.join("rekordbox/marker"), b"restored").unwrap();
        std::fs::write(work.join("swapped"), b"").unwrap();
        recover(&st);
        assert!(pioneer.join("rekordbox/marker").is_file() && !work.exists());
        std::fs::remove_file(pioneer.join("rekordbox/marker")).unwrap();
        // a set re-sent without a song takes it off the stick (as rekordbox does); restoring the
        // set from before says it's gone rather than pretending
        let other = tmp.path().join("stick2");
        std::fs::create_dir_all(&other).unwrap();
        let o = other.to_string_lossy().into_owned();
        export_set(&o, "A", &songs, None, &|_| {}).unwrap();
        let b = export_set(&o, "A", &songs[1..], None, &|_| {}).unwrap().backup.unwrap();
        assert_eq!(walk_files(&other.join("Contents")).len(), 1);
        let back = restore(&o, &b).unwrap();
        assert_eq!(back.missing.len(), 1, "{back:?}");
        // two sticks keep their own backups
        assert_ne!(stick_id(&st, false).unwrap(), stick_id(&o, false).unwrap());
        assert_eq!(list_backups(&o).len(), 2); // before the second A, and before the restore
        // nothing readable: an error, and no backup used up
        let n = list_backups(&st).len();
        assert!(export_set(&st, "Three", &set[2..], None, &|_| {}).is_err());
        assert_eq!(list_backups(&st).len(), n);
    }
}
