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
    // sized up front when the length is known, so a long file doesn't double the buffer past the cap
    let start = stream.expected_frames().map_or(1 << 22, |f| usize::try_from(f).unwrap_or(cap)).min(cap);
    let mut head: Vec<f32> = Vec::with_capacity(start);
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
        if head.len() + chunk.len() > cap {
            // past the analysed part: the rest only feeds the waveform
            let mut b = rbl_analysis::waveform::Builder::new(rate);
            b.push(&head);
            b.push(chunk);
            head.extend_from_slice(&chunk[..cap - head.len()]);
            builder = Some(b);
            past_head = true;
        } else {
            head.extend_from_slice(chunk);
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
    if !t.source_path.is_file() {
        return None; // a decoded copy (see `decoded_copy`) that has been cleared away
    }
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

/// A FLAC copy of a song the analyser can't decode (Opus, WMA, …), made once with ffmpeg and
/// kept with dubplate's saved analysis. It carries the song's own file name, so the stick shows
/// that name; the library's file is only read.
fn decoded_copy(src: &str) -> Result<String, String> {
    let ff = crate::copy::ffmpeg_path().0.ok_or(
        "this kind of file needs ffmpeg to go on a rekordbox stick: install ffmpeg (on Windows, the Install ffmpeg now button under 💾 Make a USB for the decks)",
    )?;
    let m = std::fs::metadata(src).map_err(|e| e.to_string())?;
    let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    let h = sha1_smol::Sha1::from(format!("decoded-1\n{src}\n{}\n{mtime}", m.len())).digest().to_string();
    let dir = join(&join(&paths::app_dir(), "decoded"), &h[..16]);
    let stem = Path::new(src).file_stem().map_or_else(|| "song".into(), |s| s.to_string_lossy().into_owned());
    let out = join(&dir, &format!("{stem}.flac"));
    if paths::isfile(&out) {
        return Ok(out);
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let part = join(&dir, ".decoding.flac");
    let run = crate::copy::quiet(std::process::Command::new(ff).args(["-v", "error", "-nostdin", "-y", "-i"]))
        .arg(format!("file:{}", paths::longp(src)))
        .args(["-map", "0:a:0", "-map_metadata", "0", "-c:a", "flac", "-sample_fmt", "s16", "-f", "flac"])
        .arg(format!("file:{}", paths::longp(&part)))
        .output();
    match run {
        Ok(o) if o.status.success() => std::fs::rename(&part, &out).map_err(|e| e.to_string()).map(|()| out),
        Ok(o) => {
            let _ = std::fs::remove_file(&part);
            let why = String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("").trim().to_string();
            Err(format!("ffmpeg couldn't read it either ({why})"))
        }
        Err(e) => Err(format!("couldn't run ffmpeg ({e})")),
    }
}

/// what goes on the stick for one song: its details and its analysis files. Analysed once,
/// then taken from dubplate's saved analysis until the file changes.
pub fn prepare(song: &Song) -> Result<SourceTrack, String> {
    let key = cache_key(&song.path);
    if let Some(t) = key.as_deref().and_then(load_cached) {
        return Ok(t);
    }
    let tags = read_tags(&song.path);
    // a format the analyser can't decode goes on the stick from a FLAC copy ffmpeg makes
    let (source, (analysis, waves, duration)) = match analyse_file(Path::new(&song.path)) {
        Ok(a) => (song.path.clone(), a),
        Err(first) => {
            let copy = decoded_copy(&song.path).map_err(|e| format!("{first}; {e}"))?;
            let a = analyse_file(Path::new(&copy))?;
            (copy, a)
        }
    };
    let path = Path::new(&source);
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

/// The stick as typed: `~` expanded, made absolute, and on Windows a bare drive (`E:`) means
/// its top folder (`E:\`), not wherever that drive's current folder happens to be.
pub fn stick_path(raw: &str) -> String {
    let raw = raw.trim();
    let b = raw.as_bytes();
    if paths::WINDOWS && (b.len() == 2 || (b.len() == 3 && (b[2] == b'\\' || b[2] == b'/'))) && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return format!("{}:\\", raw[..1].to_ascii_uppercase());
    }
    paths::abspath(&paths::expanduser(raw))
}

/// A rekordbox stick manages its own files (it removes songs no playlist has any more), so it
/// must never be, hold, or sit inside one of the library's folders.
pub fn not_the_library(stick: &str, library: &[String]) -> Result<(), String> {
    let real = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| Path::new(&paths::abspath(p)).to_path_buf());
    let s = real(stick);
    for root in library.iter().filter(|r| !r.trim().is_empty()) {
        let r = real(root);
        let why = if s.starts_with(&r) {
            format!("{stick} is part of your music library ({root})")
        } else if r.starts_with(&s) {
            format!("{stick} holds your music library ({root})")
        } else {
            continue;
        };
        return Err(format!("{why}. Pick the USB stick itself; dubplate never writes a rekordbox library into your music folders."));
    }
    Ok(())
}

/// dubplate's own folder on a stick, at its top level (never inside the library, so it stays
/// put whatever happens to the library): the stick's id, the lock, an unfinished restore
fn own_dir(stick: &str) -> std::path::PathBuf {
    Path::new(stick).join(".dubplate")
}

/// Only one dubplate writes a stick at a time (the page and a terminal, say). Held until dropped.
struct StickLock(#[allow(dead_code)] std::fs::File);

fn lock_stick(stick: &str) -> Result<StickLock, String> {
    let dir = own_dir(stick);
    std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't write to the stick ({e})"))?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("lock"))
        .map_err(|e| format!("couldn't write to the stick ({e})"))?;
    match f.try_lock() {
        Ok(()) => Ok(StickLock(f)),
        Err(std::fs::TryLockError::WouldBlock) => Err("Another dubplate is writing this stick right now; wait for it to finish.".into()),
        Err(std::fs::TryLockError::Error(e)) => Err(format!("couldn't lock the stick ({e})")),
    }
}

/// rbxport's own lock on the stick (`.rbxport-write.lock`), so dubplate's moves and copies of
/// the library never cross an rbxport export (from dubplate or the rbxport app). Waits a while
/// for one that's running. Never held while calling into rbxport, which takes it itself.
fn rbx_lock(stick: &str, exclusive: bool) -> Result<std::fs::File, String> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(Path::new(stick).join(".rbxport-write.lock"))
        .map_err(|e| format!("couldn't lock the stick's library ({e})"))?;
    let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let r = if exclusive { f.try_lock() } else { f.try_lock_shared() };
        match r {
            Ok(()) => return Ok(f),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err("Another program is writing this stick's library; try again when it's done.".into())
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(format!("couldn't lock the stick's library ({e})")),
        }
    }
}

/// Write `songs` to the stick at `stick` as the playlist `name` (replacing a playlist of that
/// name; other playlists on the stick stay). Songs that can't be read are left out and listed.
pub fn export_set(
    stick: &str,
    name: &str,
    songs: &[Song],
    convert: Option<CompatibilityFormat>,
    library: &[String],
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<Report, String> {
    if !paths::isdir(stick) {
        return Err(format!("{stick} isn't there (is the stick plugged in?)"));
    }
    not_the_library(stick, library)?;
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the playlist a name.".into());
    }
    // a song listed twice goes in once (the stick's library holds each file once)
    let mut seen = std::collections::HashSet::new();
    let songs: Vec<&Song> = songs.iter().filter(|s| seen.insert(paths::normcase(&s.path))).collect();
    // analyse (or take the saved analysis), a few songs at a time
    let total = songs.len();
    let next = std::sync::atomic::AtomicUsize::new(0);
    // the results, and how many are in (counted under the same lock so progress never steps back)
    type Results = (Vec<Option<Result<SourceTrack, String>>>, usize);
    let results: std::sync::Mutex<Results> = std::sync::Mutex::new((vec![None; total], 0));
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
                let mut g = results.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                g.0[i] = Some(r);
                g.1 += 1;
                progress(Progress { stage: "analysing".into(), done: g.1, total, current: paths::basename(&songs[i].path).to_string() });
            });
        }
    });
    let mut tracks = Vec::new();
    let mut errors = Vec::new();
    for (song, r) in songs.iter().zip(results.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner).0) {
        match r {
            Some(Ok(t)) => tracks.push(t),
            Some(Err(e)) => errors.push((song.path.clone(), e)),
            None => errors.push((song.path.clone(), "not analysed".into())),
        }
    }
    if tracks.is_empty() {
        return Err("None of the songs could be read, so the stick wasn't changed.".into());
    }
    // "leave them as they are" (FLAC decks) still can't play beyond 96 kHz or 24 bits
    let mut too_fine: Vec<String> = Vec::new();
    if convert.is_none() {
        for t in &tracks {
            if t.sample_rate > 96_000 || t.metadata.bit_depth > 24 {
                too_fine.push(format!(
                    "{}: {} kHz / {}-bit is more than any deck plays; choose AIFF to have it converted",
                    t.title,
                    f64::from(t.sample_rate) / 1000.0,
                    t.metadata.bit_depth
                ));
            }
        }
    }
    let n = tracks.len();
    let playlist = SourcePlaylist { id: playlist_id(name), name: name.to_string(), track_indices: (0..n).collect(), ..Default::default() };
    // add to what the stick holds (other playlists stay), as rekordbox's "Export Playlist" does
    let options = ExportOptions { compatibility: convert, keep_unlisted: true, ..Default::default() };

    let _lock = lock_stick(stick)?;
    settle(stick)?;
    // the stick's library as it is now, kept if the export changes it
    let backup = backup(stick)?;
    let before = files_state(&Path::new(stick).join("Contents"));
    // rbxport waits for its lock without end: make sure no other writer has it (with a limit)
    if let Err(e) = rbx_lock(stick, true).map(drop) {
        if let Some(b) = &backup {
            discard_backup(stick, b);
        }
        return Err(e);
    }
    let result = rbl_export::export_with_options(Path::new(stick), &tracks, &[playlist], &[], &options, &mut |p| {
        let (done, total) = if p.total > 0 { (p.done, p.total) } else { (p.tracks_done, p.tracks_total) };
        progress(Progress { stage: p.stage.to_string(), done, total, current: p.title.clone() })
    });
    let report = match result {
        Ok(r) => r,
        Err(e) => {
            // rbxport can fail after it has written something: the backup goes only if the
            // stick's library is still exactly what it holds
            let kept = match &backup {
                Some(b) if !rbl_export::recovery_pending(Path::new(stick)) && library_matches(stick, b) => {
                    discard_backup(stick, b);
                    None
                }
                b => b.clone(),
            };
            if let Some(b) = &kept {
                keep_backups(stick, &[b]);
            }
            let note = kept.map(|b| format!(" The stick's library from before is backed up ({b})."));
            return Err(format!("couldn't write the stick: {}{}", sentence(&e.to_string()), note.unwrap_or_default()));
        }
    };
    let backup = match backup {
        Some(b) if report.up_to_date => {
            discard_backup(stick, &b);
            None
        }
        Some(b) => {
            keep_backups(stick, &[&b]);
            Some(b)
        }
        None => None,
    };
    progress(Progress { stage: "done".into(), done: n, total: n, current: String::new() });
    let left_out: Vec<(String, String)> =
        report.skipped.iter().chain(&report.failed).map(|f| (String::new(), f.clone())).collect();
    // rbxport counts the whole stick; for this playlist: the songs whose copy on the stick is
    // new or rewritten, and the rest that were there already
    let tracks = n.saturating_sub(left_out.len());
    let after = files_state(&Path::new(stick).join("Contents"));
    let copied = after.iter().filter(|(k, v)| before.get(*k) != Some(*v)).count().min(tracks);
    Ok(Report {
        playlist: name.to_string(),
        tracks,
        copied,
        converted: report.converted.min(copied),
        reused: tracks - copied,
        verified: report.verified,
        backup,
        errors: errors.into_iter().chain(left_out).collect(),
        warnings: too_fine
            .into_iter()
            .chain(report.warnings.iter()
            .map(|w| {
                let why = match w.reason.as_str() {
                    "analysis-stale" => "its analysis on the stick is from an older version of the file",
                    r => r,
                };
                format!("{}: {why}{}", w.title, w.detail.as_deref().map(|d| format!(" ({d})")).unwrap_or_default())
            }))
            .collect(),
    })
}

/// every file under `dir`, with its size and date
fn files_state(dir: &Path) -> std::collections::HashMap<std::path::PathBuf, (u64, Option<std::time::SystemTime>)> {
    let mut m = std::collections::HashMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => todo.push(e.path()),
                Ok(_) => {
                    let md = e.metadata().ok();
                    m.insert(e.path(), (md.as_ref().map_or(0, |m| m.len()), md.and_then(|m| m.modified().ok())));
                }
                Err(_) => {}
            }
        }
    }
    m
}

/// Before touching a stick: finish what an earlier run left half done - dubplate's own restore,
/// then an rbxport export (it journals its writes) - so a backup never copies a half-written
/// library.
fn settle(stick: &str) -> Result<(), String> {
    recover(stick)?;
    if rbl_export::recovery_pending(Path::new(stick)) {
        // rbxport waits for its lock without end; wait here instead, with a limit
        drop(rbx_lock(stick, true)?);
    }
    match rbl_export::recover(Path::new(stick)) {
        Ok(()) => Ok(()),
        // the interrupted export was rolled back: the stick holds the library from before it
        Err(e) if e.to_string().contains("previous generation restored") => Ok(()),
        Err(e) => Err(format!("the stick has an unfinished export that couldn't be settled ({e})")),
    }
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

/// the serial number the stick got when it was formatted, when the system tells it: a stick
/// made by copying another stick's files has the same files but its own serial. The same on
/// Windows and Linux: the last 8 hex digits (Windows gives FAT/exFAT/NTFS sticks' 32-bit serial,
/// Linux shows it as `1A2B-3C4D`, or 16 digits for NTFS).
fn volume_serial(stick: &str) -> Option<String> {
    normal_serial(&volume_serial_raw(stick)?)
}

fn normal_serial(raw: &str) -> Option<String> {
    let s: String = raw.chars().filter(char::is_ascii_hexdigit).collect::<String>().to_ascii_uppercase();
    (s.len() >= 8).then(|| s[s.len() - 8..].to_string())
}

fn volume_serial_raw(stick: &str) -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = std::ffi::OsStr::new(stick).encode_wide().chain([0]).collect();
        let mut root = [0u16; 300];
        let mut serial = 0u32;
        unsafe {
            if GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), root.len() as u32) == 0 {
                return None;
            }
            let ok = GetVolumeInformationW(root.as_ptr(), std::ptr::null_mut(), 0, &mut serial, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0);
            (ok != 0 && serial != 0).then(|| format!("{serial:08X}"))
        }
    }
    #[cfg(target_os = "linux")]
    {
        // the device mounted at the stick, then its UUID
        let real = std::fs::canonicalize(stick).ok()?;
        let src = mount_source(&std::fs::read_to_string("/proc/self/mountinfo").ok()?, &real)?;
        uuid_of(Path::new(&src), Path::new("/dev/disk/by-uuid"))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = stick;
        None
    }
}

/// the device mounted where `path` is (the longest mount point it's under), from mountinfo
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn mount_source(mountinfo: &str, path: &Path) -> Option<String> {
    let unescape = |s: &str| s.replace("\\040", " ").replace("\\011", "\t").replace("\\012", "\n").replace("\\134", "\\");
    let mut best: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let Some(sep) = f.iter().position(|x| *x == "-") else { continue };
        let (Some(mp), Some(src)) = (f.get(4), f.get(sep + 2)) else { continue };
        let mp = unescape(mp);
        if path.starts_with(&mp) && best.as_ref().is_none_or(|(n, _)| mp.len() >= *n) {
            best = Some((mp.len(), unescape(src)));
        }
    }
    best.map(|(_, s)| s)
}

/// the name of the link in `by_uuid` that points at device `dev`
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn uuid_of(dev: &Path, by_uuid: &Path) -> Option<String> {
    let dev = std::fs::canonicalize(dev).ok()?;
    std::fs::read_dir(by_uuid)
        .ok()?
        .flatten()
        .find(|e| std::fs::canonicalize(e.path()).is_ok_and(|p| p == dev))
        .map(|e| e.file_name().to_string_lossy().into_owned())
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetVolumePathNameW(file: *const u16, root: *mut u16, len: u32) -> i32;
    fn GetVolumeInformationW(
        root: *const u16,
        name: *mut u16,
        name_len: u32,
        serial: *mut u32,
        max_component: *mut u32,
        flags: *mut u32,
        fs_name: *mut u16,
        fs_name_len: u32,
    ) -> i32;
}

/// The stick's own name for dubplate, kept on it (`.dubplate/stick-id`, with a second copy in
/// `stick-id.bak`), so its backups follow the stick, not the drive letter or mount point it has
/// today. It carries the stick's serial too: a copy of a stick (same files, different stick)
/// gets a name of its own. Made on first need when `make`.
fn stick_id(stick: &str, make: bool) -> Result<Option<String>, String> {
    let dir = own_dir(stick);
    let serial = volume_serial(stick);
    // (id, serial written with it), from the file or else its second copy
    let read = |name: &str| -> Result<Option<(String, Option<String>)>, String> {
        let text = match std::fs::read(dir.join(name)) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("couldn't read the stick ({e})")),
        };
        let mut lines = text.lines().map(str::trim);
        let id = lines.next().unwrap_or("").to_ascii_lowercase();
        let stored = lines.next().filter(|s| !s.is_empty()).map(str::to_string);
        Ok((id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit())).then_some((id, stored)))
    };
    let found = match read("stick-id")? {
        Some(x) => Some(x),
        None => read("stick-id.bak")?,
    };
    let write = |id: &str| -> Result<(), String> {
        let fail = |e: std::io::Error| format!("couldn't write to the stick ({e}) - nothing was changed");
        rbl_core::durable::create_dir_all(&dir).map_err(fail)?;
        let text = format!("{id}\n{}\n", serial.as_deref().unwrap_or(""));
        // the second copy first: one of the two is always whole
        rbl_core::durable::write(&dir.join("stick-id.bak"), text.as_bytes()).map_err(fail)?;
        rbl_core::durable::write(&dir.join("stick-id"), text.as_bytes()).map_err(fail)
    };
    if let Some((id, stored)) = found {
        let same_stick = match (stored.as_deref(), serial.as_deref()) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => true, // nothing to tell them apart by
        };
        if same_stick {
            if make && (stored.is_none() && serial.is_some() || read("stick-id")?.is_none()) {
                write(&id)?; // learn the serial / mend the first copy
            }
            return Ok(Some(id));
        }
    }
    if !make {
        return Ok(None);
    }
    let seed = format!("{stick}\n{}\n{:?}\n{serial:?}", std::process::id(), std::time::SystemTime::now());
    let id = sha1_smol::Sha1::from(seed).digest().to_string()[..16].to_string();
    write(&id)?;
    Ok(Some(id))
}

fn backups_base(id: &str) -> String {
    join(&join(&data_dir(), "stick-backups"), id)
}

/// rbxport's own safekeeping inside its folder (`rbxport/recovered-*`: files it set aside while
/// settling an interrupted export): not part of the library, so never backed up, compared or
/// restored over - it stays on the stick as it is
fn rbxport_keep(dir: &str, name: &std::ffi::OsStr) -> bool {
    dir == "rbxport" && name.to_string_lossy().starts_with("recovered-")
}

/// copy a folder of the stick's library (`d` in LIBRARY_DIRS); `durable`: every file and folder
/// is on the device before this returns (for copies onto the stick)
fn copy_library_dir(d: &str, from: &Path, to: &Path, durable: bool) -> std::io::Result<()> {
    fn tree(from: &Path, to: &Path, durable: bool, skip: &dyn Fn(&std::ffi::OsStr) -> bool) -> std::io::Result<()> {
        if durable { rbl_core::durable::create_dir_all(to)? } else { std::fs::create_dir_all(to)? }
        for e in std::fs::read_dir(from)? {
            let e = e?;
            if skip(&e.file_name()) {
                continue;
            }
            let (src, dst) = (e.path(), to.join(e.file_name()));
            if e.file_type()?.is_dir() {
                tree(&src, &dst, durable, &|_| false)?;
            } else {
                std::fs::copy(&src, &dst)?;
                if durable {
                    rbl_core::durable::flush(&std::fs::OpenOptions::new().write(true).open(&dst)?)?;
                }
            }
        }
        if durable {
            rbl_core::durable::sync_dir(to)?;
        }
        Ok(())
    }
    tree(from, to, durable, &|n| rbxport_keep(d, n))
}

/// do two copies of a folder of the stick's library hold the same files, byte for byte?
fn same_library_dir(d: &str, a: &Path, b: &Path) -> bool {
    fn same_file(a: &Path, b: &Path) -> bool {
        use std::io::Read;
        let (Ok(mut x), Ok(mut y)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
        if x.metadata().map(|m| m.len()).ok() != y.metadata().map(|m| m.len()).ok() {
            return false;
        }
        let (mut p, mut q) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
        loop {
            let (Ok(n), Ok(m)) = (x.read(&mut p), y.read(&mut q[..])) else { return false };
            if n != m {
                // short reads can differ; fall back to reading the rest whole
                let (mut rest_a, mut rest_b) = (p[..n].to_vec(), q[..m].to_vec());
                return x.read_to_end(&mut rest_a).is_ok() && y.read_to_end(&mut rest_b).is_ok() && rest_a == rest_b;
            }
            if n == 0 {
                return true;
            }
            if p[..n] != q[..n] {
                return false;
            }
        }
    }
    fn tree(a: &Path, b: &Path, skip: &dyn Fn(&std::ffi::OsStr) -> bool) -> bool {
        let list = |d: &Path| -> Option<Vec<(std::ffi::OsString, bool)>> {
            let mut v: Vec<_> = std::fs::read_dir(d)
                .ok()?
                .flatten()
                .filter(|e| !skip(&e.file_name()))
                .map(|e| (e.file_name(), e.file_type().is_ok_and(|t| t.is_dir())))
                .collect();
            v.sort();
            Some(v)
        };
        match (list(a), list(b)) {
            (Some(x), Some(y)) if x == y => x.iter().all(|(name, dir)| {
                if *dir { tree(&a.join(name), &b.join(name), &|_| false) } else { same_file(&a.join(name), &b.join(name)) }
            }),
            _ => false,
        }
    }
    tree(a, b, &|n| rbxport_keep(d, n))
}

/// the folders on a stick that hold its library (not the music): rekordbox's databases, and
/// rbxport's record of what it put there
const LIBRARY_DIRS: [&str; 2] = ["rekordbox", "rbxport"];
/// backups kept per stick: the first one dubplate made (the library from before dubplate ever
/// touched the stick) and the newest ones, this many in all
const KEEP_BACKUPS: usize = 10;

/// does the library under `root` match backup folder `from` exactly?
fn library_is(root: &Path, from: &Path) -> bool {
    LIBRARY_DIRS.iter().all(|d| match (from.join(d).is_dir(), root.join(d).is_dir()) {
        (true, true) => same_library_dir(d, &from.join(d), &root.join(d)),
        (false, false) => true,
        (false, true) => std::fs::read_dir(root.join(d)).is_ok_and(|rd| rd.flatten().all(|e| rbxport_keep(d, &e.file_name()))),
        (true, false) => false,
    })
}

/// does the stick's library still match backup `which` exactly?
fn library_matches(stick: &str, which: &str) -> bool {
    let Ok(Some(id)) = stick_id(stick, false) else { return false };
    library_is(&rbl_export::export_root(Path::new(stick)), &Path::new(&backups_base(&id)).join(which))
}

/// Copy the stick's library (its databases, not the music) into a new backup, under rbxport's
/// lock. Nothing to keep on a stick without one. Callers prune with `keep_backups` once they
/// know the backup stays.
fn backup(stick: &str) -> Result<Option<String>, String> {
    let _rbx = rbx_lock(stick, false)?; // no rbxport export writes it while it's copied
    if rbl_export::recovery_pending(Path::new(stick)) {
        return Err("An export to this stick started meanwhile; try again when it's done.".into());
    }
    backup_locked(stick)
}

/// `backup`, for a caller already holding rbxport's lock
fn backup_locked(stick: &str) -> Result<Option<String>, String> {
    let root = rbl_export::export_root(Path::new(stick));
    if !LIBRARY_DIRS.iter().any(|d| root.join(d).is_dir()) {
        return Ok(None);
    }
    let Some(id) = stick_id(stick, true)? else { return Ok(None) };
    let base = backups_base(&id);
    let stamp = chrono::Local::now().format("%Y-%m-%d %H.%M.%S").to_string();
    let mut when = stamp.clone();
    let mut n = 2;
    while paths::exists(&join(&base, &when)) || paths::exists(&join(&base, &format!(".partial-{when}"))) {
        when = format!("{stamp} ({n})");
        n += 1;
    }
    // copied in under a temporary name and renamed when complete, so a half-made backup (dubplate
    // stopped, the computer off) is never offered for restoring
    let part = join(&base, &format!(".partial-{when}"));
    let made = (|| -> std::io::Result<()> {
        for d in LIBRARY_DIRS {
            if root.join(d).is_dir() {
                copy_library_dir(d, &root.join(d), &Path::new(&part).join(d), false)?;
            }
        }
        // a backup whose library had no rekordbox folder still counts as one
        std::fs::create_dir_all(Path::new(&part).join("rekordbox"))?;
        std::fs::write(Path::new(&part).join("stick.txt"), format!("{stick}\n"))?;
        for e in walk(Path::new(&part)) {
            std::fs::OpenOptions::new().write(true).open(e)?.sync_all()?;
        }
        std::fs::rename(&part, join(&base, &when))
    })();
    if let Err(e) = made {
        let _ = std::fs::remove_dir_all(&part);
        return Err(format!("couldn't back up the stick's library first ({e}) - nothing was changed"));
    }
    Ok(Some(when))
}

/// every file under `dir`
fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) { todo.push(e.path()) } else { v.push(e.path()) }
        }
    }
    v
}

/// keep the stick's first backup, `keep`, and the newest ones; remove the rest (and half-made
/// ones left over from a day ago)
fn keep_backups(stick: &str, keep: &[&str]) {
    let Ok(Some(id)) = stick_id(stick, false) else { return };
    let base = backups_base(&id);
    let all = backups_in(&base); // newest first
    let first = all.last().cloned();
    let mut room = KEEP_BACKUPS.saturating_sub(1 + keep.iter().filter(|k| Some(**k) != first.as_deref()).count());
    for b in &all {
        if keep.contains(&b.as_str()) || Some(b) == first.as_ref() {
            continue;
        }
        if room > 0 {
            room -= 1;
        } else {
            let _ = std::fs::remove_dir_all(join(&base, b));
        }
    }
    for e in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|d| d.as_secs() > 86400);
        if e.file_name().to_string_lossy().starts_with(".partial-") && old {
            let _ = std::fs::remove_dir_all(e.path());
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

/// does the stick have a rekordbox library?
pub fn has_library(stick: &str) -> bool {
    let db = rbl_export::export_root(Path::new(stick)).join("rekordbox");
    db.join("export.pdb").is_file() || db.join("exportLibrary.db").is_file()
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
    /// something didn't go as it should
    pub problem: Option<String>,
}

/// a rename on the stick, on the device before this returns
fn durable_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)?;
    for d in [to.parent(), from.parent()].into_iter().flatten() {
        rbl_core::durable::sync_dir(d)?;
    }
    Ok(())
}

/// an empty marker file on the stick, on the device before this returns
fn durable_mark(path: &Path) -> std::io::Result<()> {
    rbl_core::durable::write(path, b"")
}

/// remove finished restores' leftovers (`restore-done-*`); one that can't go yet is left for
/// next time (a fresh name is used each time, so it never blocks a restore)
fn clear_done(stick: &str, own: &Path) {
    for e in std::fs::read_dir(own).into_iter().flatten().flatten() {
        if !e.file_name().to_string_lossy().starts_with("restore-done") {
            continue;
        }
        // rbxport's safekeeping that couldn't be moved back last time goes back first
        let root_name = std::fs::read_to_string(e.path().join("root")).unwrap_or_default();
        let root_name = if root_name.trim() == ".PIONEER" { ".PIONEER" } else { "PIONEER" };
        let rbx = Path::new(stick).join(root_name).join("rbxport");
        let mut kept = true;
        for k in std::fs::read_dir(e.path().join("old-rbxport")).into_iter().flatten().flatten() {
            if rbxport_keep("rbxport", &k.file_name()) {
                let to = rbx.join(k.file_name());
                let moved = !to.exists() && rbl_core::durable::create_dir_all(&rbx).and_then(|()| durable_rename(&k.path(), &to)).is_ok();
                kept &= moved;
            }
        }
        if kept {
            let _ = rbl_core::durable::remove_tree(&e.path());
        }
    }
}

/// A restore cut off (dubplate stopped, the stick pulled) in the middle of swapping the library
/// puts back the library it was replacing. A restore whose swap had finished stands (its work
/// folder was renamed to `restore-done-*` in one step), and only the leftovers are cleared away.
/// Whatever is in the library's place and isn't exactly the backup's copy (say rekordbox wrote
/// the stick since) is backed up before it's set aside. Fails - and nothing else may then touch
/// the stick - if the library can't be put back.
fn recover(stick: &str) -> Result<(), String> {
    let own = own_dir(stick);
    clear_done(stick, &own);
    let work = own.join("restore");
    if !work.is_dir() {
        return Ok(());
    }
    let root_name = std::fs::read_to_string(work.join("root")).unwrap_or_default();
    let root_name = match root_name.trim() {
        n @ (".PIONEER" | "PIONEER") => n.to_string(),
        _ => "PIONEER".to_string(),
    };
    let root = Path::new(stick).join(&root_name);
    let was_restoring = std::fs::read_to_string(work.join("backup")).ok().map(|s| s.trim().to_string());
    let backup_copy = was_restoring
        .filter(|b| valid_name(b))
        .and_then(|b| stick_id(stick, false).ok().flatten().map(|id| Path::new(&backups_base(&id)).join(b)));
    let fail = |e: String| format!("a restore of this stick was cut off, and its library couldn't be put back ({e}). It's kept in {} on the stick.", work.display());
    let _rbx = rbx_lock(stick, true)?;
    let set_aside: Vec<&str> = LIBRARY_DIRS
        .iter()
        .copied()
        .filter(|d| (work.join(format!("old-{d}")).is_dir() || work.join(format!("added-{d}")).exists()) && root.join(d).exists())
        .collect();
    // keep a copy of anything that's about to be set aside unless it's just the backup's copy
    let all_backup = set_aside
        .iter()
        .all(|d| backup_copy.as_ref().is_some_and(|b| b.join(d).is_dir() && same_library_dir(d, &b.join(d), &root.join(d))));
    if !set_aside.is_empty() && !all_backup {
        let kept = backup_locked(stick).map_err(fail)?;
        if let Some(k) = kept {
            keep_backups(stick, &[&k]);
        }
    }
    for d in LIBRARY_DIRS {
        let old = work.join(format!("old-{d}"));
        if !old.is_dir() && !work.join(format!("added-{d}")).exists() {
            continue;
        }
        let back = || -> std::io::Result<()> {
            if root.join(d).exists() {
                let half = work.join(format!("half-{d}"));
                if half.exists() {
                    rbl_core::durable::remove_tree(&half)?;
                }
                durable_rename(&root.join(d), &half)?;
            }
            if old.is_dir() {
                durable_rename(&old, &root.join(d))?;
            }
            Ok(())
        };
        back().map_err(|e| fail(e.to_string()))?;
    }
    let _ = rbl_core::durable::remove_tree(&work);
    Ok(())
}

/// "message." - one full stop, whatever the message ended with
pub fn sentence(s: &str) -> String {
    format!("{}.", s.trim_end().trim_end_matches('.'))
}

/// Put a backup of the stick's library back (what it holds now is backed up first). The music
/// on the stick is left alone. The backup is copied onto the stick first and swapped in with
/// renames, so a stick pulled out midway keeps a whole library.
pub fn restore(stick: &str, which: &str, library: &[String]) -> Result<Restored, String> {
    not_the_library(stick, library)?;
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
    let _lock = lock_stick(stick)?;
    settle(stick)?;
    // rbxport's lock from the backup of what's there now until the backup is swapped in, so no
    // rbxport export can slip in between
    let rbx = rbx_lock(stick, true)?;
    if rbl_export::recovery_pending(Path::new(stick)) {
        return Err("An export to this stick started meanwhile; try again when it's done.".into());
    }
    let before = backup_locked(stick)?;
    let unused = |b: &Option<String>| {
        if let Some(b) = b {
            discard_backup(stick, b);
        }
    };
    let root_name = rbl_export::export_root_name(Path::new(stick)).unwrap_or("PIONEER");
    let root = Path::new(stick).join(root_name);
    let own = own_dir(stick);
    let work = own.join("restore");
    let staged = |d: &str| work.join(format!("new-{d}"));
    let old = |d: &str| work.join(format!("old-{d}"));
    // 1. copy the backup onto the stick, beside the library
    let staging = (|| -> std::io::Result<()> {
        let _ = rbl_core::durable::remove_tree(&work);
        rbl_core::durable::create_dir_all(&work)?;
        rbl_core::durable::write(&work.join("root"), root_name.as_bytes())?;
        rbl_core::durable::write(&work.join("backup"), which.as_bytes())?;
        for d in LIBRARY_DIRS {
            if from.join(d).is_dir() {
                copy_library_dir(d, &from.join(d), &staged(d), true)?;
            }
        }
        Ok(())
    })();
    if let Err(e) = staging {
        let _ = rbl_core::durable::remove_tree(&work);
        unused(&before);
        return Err(format!("couldn't put the backup back ({e}); the stick wasn't changed"));
    }
    // 2. swap: the library out of the way, the backup in
    let mut done: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
    let finished = own.join(format!("restore-done-{}", std::process::id() as u128 * 1_000_000 + std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() % 1_000_000)));
    let mut swap = || -> std::io::Result<()> {
        rbl_core::durable::create_dir_all(&root)?;
        for d in LIBRARY_DIRS {
            if root.join(d).exists() {
                durable_rename(&root.join(d), &old(d))?;
                done.push((old(d), root.join(d)));
            }
            if staged(d).is_dir() {
                if !old(d).exists() {
                    durable_mark(&work.join(format!("added-{d}")))?; // so `recover` can undo it
                }
                durable_rename(&staged(d), &root.join(d))?;
                done.push((root.join(d), staged(d)));
            }
        }
        // the restore stands from here on: one rename marks it finished
        durable_rename(&work, &finished)
    };
    if let Err(e) = swap() {
        let undone = done.iter().rev().all(|(now, was)| durable_rename(now, was).is_ok());
        drop(rbx);
        if undone {
            let _ = rbl_core::durable::remove_tree(&work);
            unused(&before);
            return Err(format!("couldn't put the backup back ({e}); the stick wasn't changed"));
        }
        // leave the work folder: the next run puts the parked library back (`recover`)
        return Err(format!("couldn't put the backup back ({e}). Run the restore (or an export) again with the stick plugged in to finish putting the stick right."));
    }
    // 3. rbxport's own safekeeping from the library just replaced stays on the stick
    let mut problem = None;
    let mut carried = true;
    if let Ok(rd) = std::fs::read_dir(finished.join("old-rbxport")) {
        for e in rd.flatten() {
            if rbxport_keep("rbxport", &e.file_name()) {
                let to = root.join("rbxport").join(e.file_name());
                let ok = rbl_core::durable::create_dir_all(root.join("rbxport")).and_then(|()| durable_rename(&e.path(), &to));
                if ok.is_err() {
                    carried = false;
                }
            }
        }
    }
    if carried {
        let _ = rbl_core::durable::remove_tree(&finished);
    } else {
        problem = Some(format!("rbxport's recovery files from the old library couldn't be moved back; they're in {}", finished.display()));
    }
    drop(rbx);
    if let Some(b) = &before {
        keep_backups(stick, &[b, which]);
    }
    // 4. check: the stick's databases are now byte for byte the backup's
    if problem.is_none() && !library_is(&root, &from) {
        problem = Some("what's on the stick now doesn't match the backup".into());
    }
    // songs a later export removed can't come back from a backup of the library
    let missing = rbl_export::verify(Path::new(stick)).map(|v| v.missing_audio).unwrap_or_default();
    Ok(Restored { backup: before, missing, problem })
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
        [(120.0, "First"), (128.0, "Second"), (100.0, "Third")]
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
        let songs = &songs[..2];
        let twice = vec![songs[0].clone(), songs[1].clone(), songs[0].clone()];
        let r = export_set(&st, "Friday", &twice, None, &[], &|_| {}).unwrap();
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
        let r2 = export_set(&st, "Saturday", &songs[..1], None, &[], &|_| {}).unwrap();
        assert!(r2.backup.is_some());
        let files: Vec<_> = walk_files(&stick.join("Contents"));
        assert_eq!(files.len(), 2, "{files:?}");
        assert_eq!((r2.tracks, r2.copied, r2.reused), (1, 0, 1), "{r2:?}");
        let v = rbl_export::verify(&stick).unwrap();
        assert_eq!(v.playlists, 2, "{v:?}");
        assert_eq!(list_backups(&st).len(), 1);
        // the same set again changes nothing, and uses up no backup
        let r3 = export_set(&st, "Saturday", &songs[..1], None, &[], &|_| {}).unwrap();
        assert!(r3.backup.is_none(), "{r3:?}");
        assert_eq!(list_backups(&st).len(), 1);
        // sending Friday again (any case) replaces it rather than adding a third
        export_set(&st, "friday", &songs[1..], None, &[], &|_| {}).unwrap();
        let v = rbl_export::verify(&stick).unwrap();
        assert!(v.is_ok() && v.playlists == 2, "{v:?}");
        let all = list_backups(&st);
        assert_eq!(all.len(), 2);
        let first = all.last().unwrap().clone(); // oldest: before Saturday
        // the backups follow the stick, not where it's plugged in
        let moved = tmp.path().join("stick-on-another-day");
        std::fs::rename(&stick, &moved).unwrap();
        assert!(list_backups(&st).is_empty());
        assert!(restore(&st, &first, &[]).is_err()); // not plugged in: an error, nothing written
        assert!(!stick.exists());
        let mv = moved.to_string_lossy().into_owned();
        assert_eq!(list_backups(&mv), all);
        // the backup can be put back
        let back = restore(&mv, &first, &[]).unwrap();
        assert!(back.backup.is_some() && back.problem.is_none(), "{back:?}");
        let v = rbl_export::verify(&moved).unwrap();
        assert_eq!(v.playlists, 1);
        assert!(back.missing.is_empty(), "{back:?}"); // First stayed: Saturday holds it
        assert!(rbl_export::verify(&moved).unwrap().is_ok());
        assert!(!moved.join(".dubplate/restore").exists() && !moved.join(".dubplate/restore-done").exists());
        assert!(restore(&mv, "../x", &[]).is_err() && restore(&mv, "nope", &[]).is_err() && restore(&mv, ".partial-x", &[]).is_err());
        // and the stick takes more sets after a restore
        export_set(&mv, "Sunday", songs, None, &[], &|_| {}).unwrap();
        let v = rbl_export::verify(&moved).unwrap();
        assert!(v.is_ok() && v.playlists == 2 && v.tracks == 2, "{v:?}");
    }

    #[test]
    fn a_copied_stick_gets_its_own_name() {
        // on a real disk (not a RAM disk), so the system can say which volume it is
        let tmp = tempfile::tempdir_in(concat!(env!("CARGO_MANIFEST_DIR"), "/target")).unwrap();
        let st = tmp.path().to_string_lossy().into_owned();
        let serial = volume_serial(&st); // None where the system can't tell (some containers / CI)
        let id = stick_id(&st, true).unwrap().unwrap();
        assert_eq!(stick_id(&st, false).unwrap().as_ref(), Some(&id)); // stable
        let text = std::fs::read_to_string(tmp.path().join(".dubplate/stick-id")).unwrap();
        assert_eq!(text.lines().nth(1).filter(|s| !s.is_empty()), serial.as_deref());
        if serial.is_some() {
            std::fs::write(tmp.path().join(".dubplate/stick-id"), format!("{id}\nOTHER-STICK\n")).unwrap();
            assert_eq!(stick_id(&st, false).unwrap(), None);
            let new = stick_id(&st, true).unwrap().unwrap();
            assert_ne!(new, id);
        }
        // the same stick seen by Windows (1A2B3C4D) and Linux (1A2B-3C4D, or 16 digits for NTFS)
        assert_eq!(normal_serial("1A2B-3C4D"), normal_serial("1a2b3c4d"));
        assert_eq!(normal_serial("01D9F2C41A2B3C4D").as_deref(), Some("1A2B3C4D"));
        assert_eq!(normal_serial("12"), None);
        // written by an older dubplate without the serial: learnt, same name
        let id = stick_id(&st, true).unwrap().unwrap();
        std::fs::write(tmp.path().join(".dubplate/stick-id"), format!("{id}\n")).unwrap();
        assert_eq!(stick_id(&st, true).unwrap().as_ref(), Some(&id));
        // a damaged first copy (a stick pulled mid-write): the second copy keeps the name, and the
        // first is mended
        std::fs::write(tmp.path().join(".dubplate/stick-id"), b"").unwrap();
        assert_eq!(stick_id(&st, false).unwrap().as_ref(), Some(&id));
        assert_eq!(stick_id(&st, true).unwrap().as_ref(), Some(&id));
        assert!(std::fs::read_to_string(tmp.path().join(".dubplate/stick-id")).unwrap().starts_with(&id));
    }

    #[test]
    fn a_stick_is_never_the_library() {
        let tmp = tempfile::tempdir().unwrap();
        let music = tmp.path().join("Music");
        let stick = tmp.path().join("stick");
        std::fs::create_dir_all(music.join("Artist")).unwrap();
        std::fs::create_dir_all(&stick).unwrap();
        let lib = vec![music.to_string_lossy().into_owned()];
        let p = |x: &Path| x.to_string_lossy().into_owned();
        assert!(not_the_library(&p(&stick), &lib).is_ok());
        assert!(not_the_library(&p(&music), &lib).is_err()); // the library itself
        assert!(not_the_library(&p(&music.join("Artist")), &lib).is_err()); // inside it
        assert!(not_the_library(&p(tmp.path()), &lib).is_err()); // holding it
        assert!(not_the_library(&p(&stick), &[String::new()]).is_ok());
        let songs = vec![Song { path: p(&music.join("x.wav")), artist: String::new(), title: String::new(), album: String::new() }];
        assert!(export_set(&p(&music), "Set", &songs, None, &lib, &|_| {}).unwrap_err().contains("music library"));
        assert!(!music.join("PIONEER").exists() && !music.join(".dubplate").exists());
    }

    /// a test sound made with ffmpeg (None when ffmpeg isn't installed)
    fn ffmpeg_song(path: &Path, args: &[&str]) -> Option<String> {
        let ff = crate::copy::ffmpeg_path().0?;
        let ok = std::process::Command::new(ff)
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=6"])
            .args(args)
            .arg(path)
            .status()
            .ok()?
            .success();
        ok.then(|| path.to_string_lossy().into_owned())
    }

    #[test]
    fn every_kind_of_song_goes_on_the_stick() {
        let _g = crate::paths::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("LOCALAPPDATA", tmp.path().join("app"));
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let mk = |name: &str, args: &[&str]| ffmpeg_song(&src.join(name), args);
        let Some(ogg) = mk("A - Vorbis.ogg", &["-c:a", "libvorbis"]) else {
            eprintln!("ffmpeg isn't installed: skipped");
            return;
        };
        let opus = mk("A - Opus.opus", &["-c:a", "libopus"]).unwrap();
        let aac = mk("A - Aac.m4a", &["-c:a", "aac"]).unwrap();
        let hires = mk("A - Hires.flac", &["-ar", "192000", "-sample_fmt", "s32"]).unwrap();
        let song = |p: &String| Song { path: p.clone(), artist: "A".into(), title: String::new(), album: String::new() };
        let stick = tmp.path().join("stick");
        std::fs::create_dir_all(&stick).unwrap();
        let st = stick.to_string_lossy().into_owned();
        let set: Vec<Song> = [&ogg, &opus, &aac].into_iter().map(song).collect();
        let r = export_set(&st, "Formats", &set, parse_convert("aiff"), &[], &|_| {}).unwrap();
        assert!(r.errors.is_empty(), "{r:?}");
        assert_eq!((r.tracks, r.converted), (3, 3), "{r:?}");
        let v = rbl_export::verify(&stick).unwrap();
        assert!(v.is_ok() && v.tracks == 3 && v.analysis_present == 3, "{v:?}");
        // the Opus song is on the stick under its own name
        let names: Vec<String> = walk(&stick.join("Contents")).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert!(names.iter().any(|n| n.starts_with("A - Opus")), "{names:?}");
        // the library's files are as they were
        assert!(Path::new(&opus).is_file() && Path::new(&ogg).is_file());
        // left as they are: a file no deck plays is pointed out
        let r = export_set(&st, "Hires", &[song(&hires)], None, &[], &|_| {}).unwrap();
        assert!(r.warnings.iter().any(|w| w.contains("192 kHz")), "{r:?}");
    }

    #[test]
    fn which_device_a_folder_is_on() {
        let info = "22 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw\n\
                    90 22 8:17 / /media/me/DJ\\040STICK rw,nosuid shared:50 - vfat /dev/sdb1 rw,uid=1000\n\
                    91 22 0:5 / /media/me/DJ rw shared:51 - tmpfs tmpfs rw\n";
        assert_eq!(mount_source(info, Path::new("/media/me/DJ STICK/PIONEER")).as_deref(), Some("/dev/sdb1"));
        assert_eq!(mount_source(info, Path::new("/media/me/DJ")).as_deref(), Some("tmpfs"));
        assert_eq!(mount_source(info, Path::new("/home/me")).as_deref(), Some("/dev/sda2"));
        // /media/me/DJX is not under /media/me/DJ
        assert_eq!(mount_source(info, Path::new("/media/me/DJX")).as_deref(), Some("/dev/sda2"));
        // the link pointing at the device names its UUID
        let tmp = tempfile::tempdir().unwrap();
        let dev = tmp.path().join("sdb1");
        std::fs::write(&dev, b"").unwrap();
        let by = tmp.path().join("by-uuid");
        std::fs::create_dir_all(&by).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&dev, by.join("1A2B-3C4D")).unwrap();
            std::os::unix::fs::symlink(tmp.path().join("other"), by.join("FFFF-0000")).unwrap();
            assert_eq!(uuid_of(&dev, &by).as_deref(), Some("1A2B-3C4D"));
        }
    }

    #[test]
    fn backups_keep_the_first_and_never_lose_the_one_being_restored() {
        let _g = crate::paths::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let songs = setup(tmp.path());
        let stick = tmp.path().join("stick");
        std::fs::create_dir_all(&stick).unwrap();
        let st = stick.to_string_lossy().into_owned();
        export_set(&st, "One", &songs[..1], None, &[], &|_| {}).unwrap();
        let mut made = Vec::new();
        for _ in 0..14 {
            let b = backup(&st).unwrap().unwrap();
            keep_backups(&st, &[&b]);
            made.push(b);
        }
        let all = list_backups(&st);
        assert_eq!(all.len(), KEEP_BACKUPS);
        assert_eq!(all.last(), made.first(), "the first backup is kept");
        assert_eq!(all[0], *made.last().unwrap());
        // restoring the oldest while the list is full: the backup made first doesn't push it out
        let oldest = all.last().unwrap().clone();
        restore(&st, &oldest, &[]).unwrap();
        assert!(list_backups(&st).contains(&oldest));
        assert!(list_backups(&st).len() <= KEEP_BACKUPS + 1);
        // an export that changes nothing leaves the backups as they were
        let now = list_backups(&st);
        export_set(&st, "One", &songs[..1], None, &[], &|_| {}).unwrap();
        assert_eq!(list_backups(&st), now);
        assert!(stick.join("PIONEER/rekordbox/export.pdb").is_file());
        // a song that can't be read is left out and listed; the rest go on
        let bad = tmp.path().join("src/Broken - Song.wav");
        std::fs::write(&bad, b"RIFF....WAVEfmt nonsense").unwrap();
        let mut set = songs[..2].to_vec();
        set.push(Song { path: bad.to_string_lossy().into_owned(), artist: "Broken".into(), title: "Song".into(), album: String::new() });
        let r = export_set(&st, "Two", &set, None, &[], &|_| {}).unwrap();
        assert_eq!(r.tracks, 2);
        assert_eq!(r.errors.len(), 1, "{r:?}");
        // a restore cut off between its renames: the next run puts the old library back first
        let pioneer = stick.join("PIONEER");
        let work = stick.join(".dubplate/restore");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("root"), "PIONEER").unwrap();
        std::fs::rename(pioneer.join("rekordbox"), work.join("old-rekordbox")).unwrap();
        copy_library_dir("rekordbox", &work.join("old-rekordbox"), &work.join("new-rbxport"), false).unwrap(); // a staged leftover
        copy_library_dir("rekordbox", &work.join("old-rekordbox"), &pioneer.join("rekordbox"), false).unwrap(); // half put in place
        std::fs::write(pioneer.join("rekordbox/marker"), b"half").unwrap();
        let before_recover = list_backups(&st);
        recover(&st).unwrap();
        assert!(pioneer.join("rekordbox/export.pdb").is_file() && !pioneer.join("rekordbox/marker").exists() && !work.exists());
        // what was in the library's place wasn't a backup's copy, so it was kept before being set aside
        let newest = list_backups(&st)[0].clone();
        assert!(!before_recover.contains(&newest));
        let api = Path::new(&backups_base(&stick_id(&st, false).unwrap().unwrap())).join(&newest);
        assert!(api.join("rekordbox/marker").is_file());
        assert!(rbl_export::verify(&stick).unwrap().is_ok());
        // a library folder the cut-off restore added where the stick had none is taken out again
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("root"), "PIONEER").unwrap();
        std::fs::write(work.join("added-rbxport"), b"").unwrap();
        std::fs::rename(pioneer.join("rbxport"), stick.join("keep-rbxport")).unwrap();
        std::fs::create_dir_all(pioneer.join("rbxport")).unwrap(); // the backup's, put in place
        std::fs::create_dir_all(work.join("half-rbxport")).unwrap(); // left by an earlier attempt
        recover(&st).unwrap();
        assert!(!pioneer.join("rbxport").exists() && !work.exists());
        std::fs::rename(stick.join("keep-rbxport"), pioneer.join("rbxport")).unwrap();
        // rbxport's safekeeping left in a finished restore's folder goes back to the stick
        let left = stick.join(".dubplate/restore-done-9/old-rbxport/recovered-1");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::write(left.join("theirs.bin"), b"keep me").unwrap();
        recover(&st).unwrap();
        assert_eq!(std::fs::read(pioneer.join("rbxport/recovered-1/theirs.bin")).unwrap(), b"keep me");
        assert!(!stick.join(".dubplate/restore-done-9").exists());
        rbl_core::durable::remove_tree(&pioneer.join("rbxport/recovered-1")).unwrap();
        // ...and one whose swap had finished stands (only its leftovers go)
        let done = stick.join(".dubplate/restore-done-123");
        std::fs::create_dir_all(done.join("old-rekordbox")).unwrap();
        std::fs::write(pioneer.join("rekordbox/marker"), b"restored").unwrap();
        recover(&st).unwrap();
        assert!(pioneer.join("rekordbox/marker").is_file() && !done.exists());
        std::fs::remove_file(pioneer.join("rekordbox/marker")).unwrap();
        // a set re-sent without a song takes it off the stick (as rekordbox does); restoring the
        // set from before says it's gone rather than pretending
        let other = tmp.path().join("stick2");
        std::fs::create_dir_all(&other).unwrap();
        let o = other.to_string_lossy().into_owned();
        export_set(&o, "A", &songs[..2], None, &[], &|_| {}).unwrap();
        // replacing a set: the new song counts as copied even though another left the stick
        let r = export_set(&o, "A", &songs[1..], None, &[], &|_| {}).unwrap();
        assert_eq!((r.tracks, r.copied, r.reused), (2, 1, 1), "{r:?}");
        let b = r.backup.unwrap();
        assert_eq!(walk_files(&other.join("Contents")).len(), 2);
        let back = restore(&o, &b, &[]).unwrap();
        assert_eq!(back.missing.len(), 1, "{back:?}");
        assert!(back.problem.is_none(), "removed songs are reported as missing, not as a problem: {back:?}");
        // two sticks keep their own backups
        assert_ne!(stick_id(&st, false).unwrap(), stick_id(&o, false).unwrap());
        assert_eq!(list_backups(&o).len(), 2); // before the second A, and before the restore
        // a copy of a stick (same files, another stick's serial) is a stick of its own
        let idf = other.join(".dubplate/stick-id");
        let id = stick_id(&o, false).unwrap().unwrap();
        std::fs::write(&idf, format!("{id}\nNOT-THIS-STICK\n")).unwrap();
        if volume_serial(&o).is_some() {
            assert_eq!(stick_id(&o, false).unwrap(), None);
            assert!(list_backups(&o).is_empty());
            assert_ne!(stick_id(&o, true).unwrap().unwrap(), id);
        }
        // a stick written by older rekordbox (export.pdb only): dubplate adds to it, and putting
        // the first backup back is fine, not "a problem"
        let old = tmp.path().join("stick3");
        std::fs::create_dir_all(&old).unwrap();
        let ol = old.to_string_lossy().into_owned();
        export_set(&ol, "Old", &songs[..1], None, &[], &|_| {}).unwrap();
        std::fs::remove_file(old.join("PIONEER/rekordbox/exportLibrary.db")).unwrap();
        let _ = std::fs::remove_dir_all(old.join("PIONEER/rbxport"));
        assert!(has_library(&ol));
        let r = export_set(&ol, "New", &songs[1..2], None, &[], &|_| {}).unwrap();
        let first = r.backup.expect("the old library was backed up");
        let back = restore(&ol, &first, &[]).unwrap();
        assert!(back.problem.is_none(), "{back:?}");
        assert!(!old.join("PIONEER/rekordbox/exportLibrary.db").exists());
        // nothing readable: an error, and no backup used up
        let n = list_backups(&st).len();
        assert!(export_set(&st, "Three", &set[2..], None, &[], &|_| {}).is_err());
        assert_eq!(list_backups(&st).len(), n);
    }
}
